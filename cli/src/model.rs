//! Recordings: loading, naming elements, which element owns each drawn node, and per-element state.
use crate::fail;
use crate::py::{self, f, obj, truthy};
use crate::state::{ctx, failure, local_time, now_ms, write_file, Result, KEPT_RECORDINGS, OWNER_TOLERANCE_PT};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct Frame {
    /// The frame as recorded, plus each element's normalized `label` and `key` and each node's `owner`.
    pub v: Value,
    pub t: f64,
    /// Element index by key.
    pub by_key: HashMap<String, usize>,
    /// Node indices by owner key, in draw order. Built once per frame: queries ask for one element in every frame.
    pub by_owner: HashMap<Option<String>, Vec<usize>>,
}

impl Frame {
    pub fn from(mut v: Value, tolerance: f64) -> Frame {
        name_elements(&mut v);
        let (by_key, by_owner) = assign_owners(&mut v, tolerance);
        let t = f(&v["t"]);
        Frame { v, t, by_key, by_owner }
    }

    pub fn elements(&self) -> &[Value] {
        self.v["elements"].as_array().map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn nodes(&self) -> &[Value] {
        self.v["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn t_value(&self) -> &Value {
        &self.v["t"]
    }

    pub fn element(&self, key: &str) -> Option<&Value> {
        self.by_key.get(key).map(|&i| &self.elements()[i])
    }

    pub fn owned(&self, key: Option<&str>) -> Vec<&Value> {
        let nodes = self.nodes();
        self.by_owner
            .get(&key.map(String::from))
            .map(|list| list.iter().map(|&i| &nodes[i]).collect())
            .unwrap_or_default()
    }

    pub fn keys(&self) -> Vec<String> {
        self.elements().iter().map(element_key).collect()
    }
}

/// The DDP protocol this CLI reads (docs/protocol.md). A header without `protocol` comes from a library
/// older than the field, which reads as 0 and is still understood. Raise it when a field changes meaning
/// or goes away; an added optional field needs no new version.
pub const PROTOCOL: u64 = 1;

/// A library newer than this CLI may send frames it would misread, so it stops instead of guessing.
pub fn check_protocol(header: &Value) -> Result<()> {
    let spoken = header.get("protocol").and_then(Value::as_u64).unwrap_or(0);
    if spoken > PROTOCOL {
        fail!(
            format!("the app's library speaks DDP protocol {spoken}; this ddp reads up to {PROTOCOL}"),
            "protocol_too_new",
            "npm i -g agent-ddp@latest"
        );
    }
    Ok(())
}

pub struct Recording {
    pub header: Value,
    pub end: Value,
    pub frames: Vec<Frame>,
    pub tolerance: f64,
    pub origin: f64,
}

impl Recording {
    /// pt per recorded unit.
    pub fn to_pt(&self) -> f64 {
        to_pt(&self.header)
    }

    pub fn unit(&self) -> String {
        py::str(&self.header["unit"])
    }
}

pub fn to_pt(header: &Value) -> f64 {
    if header["unit"] == "px" {
        1.0 / header.get("scale").map(f).unwrap_or(1.0)
    } else {
        1.0
    }
}

pub fn tolerance_for(header: &Value) -> f64 {
    OWNER_TOLERANCE_PT * if header["unit"] == "px" { header.get("scale").map(f).unwrap_or(1.0) } else { 1.0 }
}

/// The app's data container. Cached per session: each `simctl` call costs about 0.2 s.
pub fn app_container(udid: &str, app: &str) -> Result<String> {
    let cache_file = ctx().dir.join("cache.json");
    let mut cache = crate::state::read_json(&cache_file).unwrap_or_else(|| json!({}));
    let key = format!("{udid}/{app}");
    if let Some(path) = cache[&key].as_str() {
        if Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }
    let result = crate::state::capture("xcrun", &["simctl", "get_app_container", udid, app, "data"]);
    if result.code != 0 {
        fail!(
            format!("no app {app} on simulator {udid}: {}", result.stderr.trim()),
            "app_not_installed",
            format!("ddp doctor {udid} --app {app}")
        );
    }
    let path = result.stdout.trim().to_string();
    cache[&key] = path.clone().into();
    write_file(&cache_file, py::dumps(&cache))?;
    Ok(path)
}

/// Writes the recording to last.jsonl and keeps a gzipped copy: the last KEPT_RECORDINGS in recordings/,
/// or under saved/<name> when named (never pruned). Gzip makes them 25-55x smaller.
pub fn save_recording(text: &str, name: Option<&str>) -> Result<PathBuf> {
    let context = ctx();
    write_file(&context.last, text)?;
    let target = match name {
        Some(name) => context.dir.join("saved").join(format!("{name}.jsonl.gz")),
        None => {
            let now = now_ms();
            context.dir.join("recordings").join(format!(
                "{}-{:03}.jsonl.gz",
                local_time("%Y%m%d-%H%M%S", now / 1000.0),
                (now as i64) % 1000
            ))
        }
    };
    write_file(&target, gzip(text.as_bytes()))?;
    let kept = history();
    if kept.len() > KEPT_RECORDINGS {
        for old in &kept[..kept.len() - KEPT_RECORDINGS] {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(target)
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// Gzipped or plain bytes as text.
pub fn decode(data: &[u8]) -> String {
    if data.starts_with(&[0x1f, 0x8b]) {
        let mut text = String::new();
        let mut decoder = flate2::read::MultiGzDecoder::new(data);
        let mut bytes = Vec::new();
        // A file cut short (the app died while writing) still gives what it holds.
        let _ = decoder.read_to_end(&mut bytes);
        text.push_str(&String::from_utf8_lossy(&bytes));
        text
    } else {
        String::from_utf8_lossy(data).into_owned()
    }
}

/// Kept recordings, oldest first.
pub fn history() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(ctx().dir.join("recordings"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.to_string_lossy().ends_with(".jsonl.gz"))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// A path, or the name of a saved recording.
pub fn recording_path(text: &str) -> PathBuf {
    let path = PathBuf::from(text);
    let saved = ctx().dir.join("saved").join(format!("{text}.jsonl.gz"));
    if !path.exists() && saved.exists() {
        saved
    } else {
        path
    }
}

pub fn read_text(path: &Path) -> Result<String> {
    let data = std::fs::read(path).map_err(|e| failure(format!("{}: {e}", path.display()), "file_not_found", None))?;
    Ok(decode(&data))
}

pub fn normalize_label(label: &str) -> String {
    label
        .chars()
        .filter(|c| !matches!(c, '\u{feff}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}'))
        .map(|c| if c == '\u{a0}' { ' ' } else { c })
        .collect()
}

pub fn parse_lines(text: &str, path: &Path) -> Result<Vec<Value>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|e| {
                failure(
                    format!("{}: not a recording line ({e})", path.display()),
                    "bad_recording",
                    Some("ddp record again"),
                )
            })
        })
        .collect()
}

pub fn load(path: &Path) -> Result<Recording> {
    let path = recording_path(&path.to_string_lossy());
    if !path.exists() {
        fail!(format!("{} not found", path.display()), "file_not_found", "ddp record, or pass --file");
    }
    let lines = parse_lines(&read_text(&path)?, &path)?;
    from_lines(lines, path)
}

pub fn from_lines(lines: Vec<Value>, path: PathBuf) -> Result<Recording> {
    let Some(header) = lines.iter().find(|l| l["type"] == "header").cloned() else {
        fail!(
            format!("{} has no recording in it (no header line)", path.display()),
            "bad_recording",
            "ddp record again"
        );
    };
    check_protocol(&header)?;
    let end = lines.iter().find(|l| l["type"] == "end").cloned().unwrap_or_else(|| json!({}));
    let tolerance = tolerance_for(&header);
    let frames = lines.into_iter().filter(|l| l["type"] == "frame").map(|l| Frame::from(l, tolerance)).collect();
    Ok(Recording { header, end, frames, tolerance, origin: 0.0 })
}

/// Normalized labels, and an index for elements that share a name in one frame: `Open`, `Open [2]`.
fn name_elements(frame: &mut Value) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    if let Some(elements) = frame["elements"].as_array_mut() {
        for element in elements {
            element["label"] = match element.get("label") {
                Some(Value::String(label)) if !label.is_empty() => normalize_label(label).into(),
                Some(other) => other.clone(),
                None => Value::Null,
            };
            let name = [element.get("id"), element.get("label")]
                .into_iter()
                .find(|v| truthy(*v))
                .map(|v| py::str(v.unwrap()))
                .unwrap_or_else(|| "?".into());
            let count = seen.entry(name.clone()).or_insert(0);
            *count += 1;
            element["key"] = if *count == 1 { name.into() } else { format!("{name} [{count}]").into() };
        }
    }
}

pub fn element_key(element: &Value) -> String {
    [element.get("key"), element.get("id"), element.get("label")]
        .into_iter()
        .find(|v| truthy(*v))
        .map(|v| py::str(v.unwrap()))
        .unwrap_or_else(|| "?".into())
}

pub fn rect(value: &Value) -> [f64; 4] {
    let mut out = [0.0; 4];
    if let Some(items) = value.as_array() {
        for (slot, item) in out.iter_mut().zip(items) {
            *slot = f(item);
        }
    }
    out
}

pub fn contains(outer: [f64; 4], inner: [f64; 4], tolerance: f64) -> bool {
    let [ox, oy, ow, oh] = outer;
    let [ix, iy, iw, ih] = inner;
    ix >= ox - tolerance && iy >= oy - tolerance && ix + iw <= ox + ow + tolerance && iy + ih <= oy + oh + tolerance
}

pub fn area(r: [f64; 4]) -> f64 {
    r[2] * r[3]
}

/// A node belongs to the element whose subtree drew it, when the recorder knows (`own`, Android).
/// Otherwise to the smallest element whose frame holds it. Others stay unowned.
#[allow(clippy::type_complexity)]
fn assign_owners(frame: &mut Value, tolerance: f64) -> (HashMap<String, usize>, HashMap<Option<String>, Vec<usize>>) {
    let elements: Vec<Value> = frame["elements"].as_array().cloned().unwrap_or_default();
    let keys: Vec<String> = elements.iter().map(element_key).collect();
    let mut by_area: Vec<usize> = (0..elements.len()).collect();
    by_area
        .sort_by(|&a, &b| area(rect(&elements[a]["frame"])).partial_cmp(&area(rect(&elements[b]["frame"]))).unwrap());
    let mut by_owner: HashMap<Option<String>, Vec<usize>> = HashMap::new();
    if let Some(nodes) = frame["nodes"].as_array_mut() {
        for (index, node) in nodes.iter_mut().enumerate() {
            let owner = if let Some(own) = node.get("own") {
                let own = f(own) as i64;
                (0..keys.len() as i64).contains(&own).then(|| keys[own as usize].clone())
            } else {
                let frame = rect(&node["frame"]);
                by_area
                    .iter()
                    .find(|&&e| contains(rect(&elements[e]["frame"]), frame, tolerance))
                    .map(|&e| keys[e].clone())
            };
            node["owner"] = owner.clone().map(Value::from).unwrap_or(Value::Null);
            by_owner.entry(owner).or_default().push(index);
        }
    }
    let by_key = keys.into_iter().enumerate().map(|(i, k)| (k, i)).collect();
    (by_key, by_owner)
}

pub fn element_state(frame: &Frame, key: &str) -> Option<Value> {
    let element = frame.element(key)?;
    let owned = frame.owned(Some(key));
    let filled: Vec<&Value> = owned.iter().copied().filter(|n| truthy(n.get("fill"))).collect();
    let bordered: Vec<&Value> = owned.iter().copied().filter(|n| truthy(n.get("border"))).collect();
    let widest = py::max_first(bordered.iter().copied(), |n| f(&n["border"]["width"]));
    let drawn = union(&owned.iter().map(|n| n.get("visible").unwrap_or(&n["frame"]).clone()).collect::<Vec<_>>());
    let mut kinds: Vec<String> = owned.iter().map(|n| py::str(&n["kind"])).collect();
    kinds.sort();
    kinds.dedup();
    let pixels = owned.iter().find(|n| truthy(n.get("pixels"))).map(|n| {
        let colors = &n["pixels"]["colors"];
        let first = if truthy(Some(colors)) { colors[0]["color"].clone() } else { Value::Null };
        json!([first, n["pixels"]["peakAlpha"].clone()])
    });
    Some(obj(vec![
        ("frame", element["frame"].clone()),
        ("drawn", drawn),
        ("label", element.get("label").cloned().unwrap_or(Value::Null)),
        (
            "fill",
            py::max_first(filled.iter().copied(), |n| area(rect(&n["frame"])))
                .map(|n| n["fill"].clone())
                .unwrap_or(Value::Null),
        ),
        ("border", widest.map(|n| n["border"]["width"].clone()).unwrap_or(json!(0))),
        ("borderColor", widest.map(|n| n["border"]["color"].clone()).unwrap_or(Value::Null)),
        (
            "opacity",
            py::min_first(owned.iter().map(|n| n.get("opacity").cloned().unwrap_or(json!(1))), f).unwrap_or(json!(1)),
        ),
        ("kinds", json!(kinds)),
        ("draws", json!(owned.iter().filter(|n| n["kind"] == "draw").count())),
        ("pixels", pixels.unwrap_or(Value::Null)),
    ]))
}

pub fn union(rects: &[Value]) -> Value {
    if rects.is_empty() {
        return Value::Null;
    }
    let pick = |i: usize| rects.iter().map(move |r| r[i].clone());
    let left = py::min_first(pick(0), f).unwrap();
    let top = py::min_first(pick(1), f).unwrap();
    let right = py::max_first(rects.iter().map(|r| py::add(&r[0], &r[2])), f).unwrap();
    let bottom = py::max_first(rects.iter().map(|r| py::add(&r[1], &r[3])), f).unwrap();
    json!([
        py::round(&left, 2),
        py::round(&top, 2),
        py::round(&py::sub(&right, &left), 2),
        py::round(&py::sub(&bottom, &top), 2)
    ])
}

pub fn element_keys(frames: &[Frame]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for frame in frames {
        for element in frame.elements() {
            let key = element_key(element);
            if seen.insert(key.clone()) {
                keys.push(key);
            }
        }
    }
    keys
}

const PAINTS_INTERIOR: [&str; 3] = ["bitmap", "gradient", "opaque-drawing"]; // "draw" ops count only when filled

pub fn paints_at(node: &Value, point: (f64, f64)) -> bool {
    let paints = PAINTS_INTERIOR.contains(&node["kind"].as_str().unwrap_or("")) || truthy(node.get("fill"));
    let [x, y, w, h] = rect(node.get("visible").unwrap_or(&node["frame"]));
    paints && x <= point.0 && point.0 <= x + w && y <= point.1 && point.1 <= y + h
}

pub fn color_alpha(color: &Value) -> Value {
    match color.as_str() {
        Some(text) if !text.is_empty() => {
            py::fnum(u8::from_str_radix(text.get(7..9).unwrap_or("FF"), 16).unwrap_or(255) as f64 / 255.0)
        }
        _ => json!(0),
    }
}

/// How strongly a node shows: its opacity times the strongest color it paints.
pub fn node_alpha(node: &Value) -> Value {
    let mut colors = vec![
        node.get("fill").cloned().unwrap_or(Value::Null),
        node.get("border").and_then(|b| b.get("color")).cloned().unwrap_or(Value::Null),
    ];
    if let Some(gradient) = node.get("gradient").and_then(Value::as_array) {
        colors.extend(gradient.iter().cloned());
    }
    let mut strongest = py::max_first(colors.iter().map(color_alpha), f).unwrap_or(json!(0));
    if truthy(node.get("pixels")) {
        strongest = node["pixels"]["peakAlpha"].clone();
    } else if PAINTS_INTERIOR.contains(&node["kind"].as_str().unwrap_or("")) && !truthy(node.get("fill")) {
        strongest = json!(1); // bitmaps and unread drawings: color unknown, count them as solid
    }
    py::mul(&node.get("opacity").cloned().unwrap_or(json!(1)), &strongest)
}

/// The first change anywhere in a recording stands for the action, so two recordings line up on it.
pub fn with_origin(recording: &mut Recording) {
    let keys = element_keys(&recording.frames);
    recording.origin = 0.0;
    for i in 1..recording.frames.len() {
        let changed = keys.iter().any(|k| {
            !option_equal(&element_state(&recording.frames[i], k), &element_state(&recording.frames[i - 1], k))
        });
        if changed {
            recording.origin = recording.frames[i].t;
            break;
        }
    }
}

pub fn option_equal(a: &Option<Value>, b: &Option<Value>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => py::equal(x, y),
        (None, None) => true,
        _ => false,
    }
}

pub fn element_alpha(frame: &Frame, key: &str) -> Value {
    py::max_first(frame.owned(Some(key)).into_iter().map(node_alpha), f).unwrap_or(json!(0))
}

pub fn frame_at(frames: &[Frame], t: f64) -> &Frame {
    py::min_first(frames.iter(), |frame| (frame.t - t).abs()).unwrap()
}

pub fn frames_between(frames: &[Frame], start: Option<f64>, end: Option<f64>) -> Vec<&Frame> {
    frames.iter().filter(|f| start.is_none_or(|s| f.t >= s) && end.is_none_or(|e| f.t <= e)).collect()
}

const QUERY_OPERATORS: [&str; 7] = ["!=", ">=", "<=", "=", ">", "<", "~"];

pub type Clause = (String, String, String);

/// 'kind=draw and alpha>0' to [(field, operator, value)].
pub fn parse_where(text: &str) -> Result<Vec<Clause>> {
    text.split(" and ")
        .map(|clause| {
            let Some(operator) = QUERY_OPERATORS.iter().find(|op| clause.contains(*op)) else {
                fail!(
                    format!("can't read '{clause}'"),
                    "bad_where",
                    format!("use field{}value joined by ' and '", QUERY_OPERATORS.join("|"))
                );
            };
            let (field, value) = clause.split_once(operator).unwrap();
            Ok((field.trim().to_string(), operator.to_string(), value.trim().to_string()))
        })
        .collect()
}

fn as_float(value: &Value) -> Option<f64> {
    match value {
        Value::Null => Some(f64::NAN),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Number(_) => Some(f(value)),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

pub fn matches(fields: &serde_json::Map<String, Value>, clauses: &[Clause]) -> Result<bool> {
    for (field, operator, expected) in clauses {
        let Some(actual) = fields.get(field) else {
            fail!(
                format!("unknown field '{field}'. Fields: {}", fields.keys().cloned().collect::<Vec<_>>().join(", ")),
                "unknown_field"
            );
        };
        let ok = if operator == "~" {
            py::str(actual).to_lowercase().contains(&expected.to_lowercase())
        } else if let (Some(a), Ok(e)) = (as_float(actual), expected.trim().parse::<f64>()) {
            compare(operator, a.partial_cmp(&e), a == e)
        } else {
            let a = if actual.is_null() { "none".to_string() } else { py::str(actual).to_lowercase() };
            let e = expected.to_lowercase();
            compare(operator, Some(a.cmp(&e)), a == e)
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

fn compare(operator: &str, order: Option<std::cmp::Ordering>, equal: bool) -> bool {
    use std::cmp::Ordering::*;
    match operator {
        "=" => equal,
        "!=" => !equal,
        ">" => order == Some(Greater),
        "<" => order == Some(Less),
        ">=" => matches!(order, Some(Greater | Equal)),
        "<=" => matches!(order, Some(Less | Equal)),
        _ => false,
    }
}

/// Prints a one-line warning when a recording has frames but no elements in any.
pub fn warn_if_empty(recording: &Recording) {
    if !recording.frames.is_empty() && recording.frames.iter().all(|f| f.elements().is_empty()) {
        crate::warn!("ddp: warning: no elements in this recording\nnext: ddp doctor");
    }
}
