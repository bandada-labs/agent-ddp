//! tree, get, find, diff, and the @eN refs they hand out.
use crate::args::Args;
use crate::checks::{comparable_state, state_differences};
use crate::live::{live_session, record_live, recording_for, snapshot_live};
use crate::model::{self, area, contains, element_alpha, element_key, element_keys, element_state, frame_at, frames_between,
                   load, node_alpha, normalize_label, read_text, recording_path, rect, with_origin, Frame, Recording};
use crate::py::{self, obj};
use crate::state::{ctx, parse_ms, write_file, Result};
use crate::{fail, say};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub fn describe_node(node: &Value) -> String {
    let mut parts = vec![format!("z={}", py::str(&node["z"])),
                         format!("{}{}", py::str(&node["kind"]),
                                 if py::truthy(node.get("op")) { format!("/{}", py::str(&node["op"])) } else { String::new() })];
    if py::truthy(node.get("fill")) {
        parts.push(format!("fill {}", py::str(&node["fill"])));
    }
    if py::truthy(node.get("border")) {
        parts.push(format!("border {} {}", py::str(&node["border"]["width"]), py::str(&node["border"]["color"])));
    }
    for field in ["corner", "opacity", "source"] {
        if py::truthy(node.get(field)) {
            parts.push(format!("{field} {}", py::str(&node[field])));
        }
    }
    if py::truthy(node.get("pixels")) {
        let colors: Vec<Value> = node["pixels"]["colors"].as_array().map(|c| c.iter().map(|c| c["color"].clone()).collect())
            .unwrap_or_default();
        parts.push(format!("pixels {} peakAlpha {}", py::repr(&json!(colors)), py::str(&node["pixels"]["peakAlpha"])));
    }
    parts.push(py::str(node.get("visible").unwrap_or(&node["frame"])));
    parts.join(" ")
}

/// @e1, @e2 ... in order of first appearance. A recording never changes, so its refs stay valid;
/// live, they go stale as soon as the screen changes.
pub fn save_refs(frames: &[Frame]) -> Result<HashMap<String, String>> {
    let keys = element_keys(frames);
    let mut refs = Map::new();
    for (index, key) in keys.iter().enumerate() {
        refs.insert(format!("@e{}", index + 1), key.clone().into());
    }
    write_file(&ctx().dir.join("refs.json"), py::dumps(&Value::Object(refs)))?;
    Ok(keys.into_iter().enumerate().map(|(i, k)| (k, format!("@e{}", i + 1))).collect())
}

pub fn resolve_ref(text: &str) -> Result<String> {
    if !text.starts_with("@e") {
        return Ok(text.to_string());
    }
    let refs = crate::state::read_json(&ctx().dir.join("refs.json")).unwrap_or(json!({}));
    match refs.get(text) {
        Some(key) => Ok(py::str(key)),
        None => fail!(format!("unknown ref {text}"), "unknown_ref", "ddp tree (refs come from the last tree)"),
    }
}

/// Each element's parent: the smallest element listed before it whose frame holds it. Recorders list
/// elements in tree order, so a parent always comes first; a popup listed later never adopts the screen below.
pub fn parents_by_containment(frame: &Frame, tolerance: f64) -> HashMap<String, Option<String>> {
    let elements = frame.elements();
    elements.iter().enumerate().map(|(index, element)| {
        let inner = rect(&element["frame"]);
        let holder = py::min_first(elements[..index].iter().filter(|o| contains(rect(&o["frame"]), inner, tolerance)),
                                   |o| area(rect(&o["frame"])));
        (element_key(element), holder.map(element_key))
    }).collect()
}

pub fn changing_keys(frames: &[Frame]) -> Vec<String> {
    element_keys(frames).into_iter().filter(|key| {
        frames.iter().map(|f| py::dumps_sorted(&element_state(f, key).unwrap_or(Value::Null))).collect::<HashSet<_>>().len() > 1
    }).collect()
}

/// A node as the recorder wrote it, plus the owner the CLI worked out.
fn public_node(node: &Value) -> Value {
    let mut copy = node.clone();
    if let Some(map) = copy.as_object_mut() {
        map.shift_remove("own");
    }
    copy
}

pub fn tree(args: &mut Args) -> Result<i32> {
    let recording = match live_session(args) {
        Some(session) => if args.changed { record_live(args)? } else { snapshot_live(&session)? },
        None => load(args.file.as_deref().unwrap_or(&ctx().last))?,
    };
    let frame = match &args.at {
        Some(at) => frame_at(&recording.frames, parse_ms(at)?),
        None => recording.frames.last().ok_or_else(|| crate::state::failure("no frames in the recording", "bad_recording", None))?,
    };
    let refs = save_refs(&recording.frames)?;
    let parents = parents_by_containment(frame, recording.tolerance);
    let mut children: HashMap<Option<String>, Vec<&Value>> = HashMap::new();
    for element in frame.elements() {
        children.entry(parents[&element_key(element)].clone()).or_default().push(element);
    }
    let mut shown: Option<HashSet<String>> = args.changed.then(|| changing_keys(&recording.frames).into_iter().collect());
    if let Some(shown) = &mut shown {
        // Keep the ancestors of what changes, so its place in the screen stays readable.
        for key in shown.clone() {
            let mut key = key;
            while let Some(Some(parent)) = parents.get(&key) {
                key = parent.clone();
                shown.insert(key.clone());
            }
        }
    }
    let root_id = match args.within.as_ref().or(args.id.as_ref()) {
        Some(id) => Some(resolve_ref(id)?),
        None => None,
    };
    let mut rows: Vec<(&Value, usize)> = Vec::new();
    if let Some(root_id) = &root_id {
        let Some(root) = frame.elements().iter().find(|e| &element_key(e) == root_id) else {
            fail!(format!("no element '{root_id}' at t={}ms", py::str(frame.t_value())), "unknown_element",
                  "ddp tree (without --within)");
        };
        if args.id.is_some() && args.within.is_none() {
            children.clear(); // --id: that element alone
        }
        collect(root, 0, &children, &shown, args.depth, &mut rows);
    } else {
        for element in children.get(&None).cloned().unwrap_or_default() {
            collect(element, 0, &children, &shown, args.depth, &mut rows);
        }
    }
    let unowned: Vec<&Value> = if args.interactive || shown.is_some() || root_id.is_some() { Vec::new() } else { frame.owned(None) };

    if ctx().json {
        let elements: Vec<Value> = rows.iter().map(|(e, depth)| {
            let key = element_key(e);
            let mut row = obj(vec![("ref", refs[&key].clone().into()), ("key", key.clone().into()), ("depth", json!(depth)),
                                   ("parent", parents[&key].clone().map(Value::from).unwrap_or(Value::Null))]);
            for field in ["id", "label", "frame", "tap", "touch"] {
                if let Some(value) = e.get(field).filter(|v| !v.is_null()) {
                    row[field] = value.clone();
                }
            }
            if !args.interactive {
                row["nodes"] = json!(frame.owned(Some(&key)).into_iter().map(public_node).collect::<Vec<_>>());
            }
            row
        }).collect();
        let mut out = obj(vec![("t", frame.t_value().clone()), ("unit", recording.header["unit"].clone()), ("elements", json!(elements))]);
        if !unowned.is_empty() {
            out["unowned"] = json!(unowned.into_iter().map(public_node).collect::<Vec<_>>());
        }
        say!("{}", py::dumps(&out));
        return Ok(0);
    }
    say!("t={}ms (frame {} of {}), unit {}", py::str(frame.t_value()), py::str(&frame.v["i"]), recording.frames.len(),
         recording.unit());
    for (element, depth) in &rows {
        let key = element_key(element);
        let label = if py::truthy(element.get("label")) && py::truthy(element.get("id")) {
            format!(" \"{}\"", py::str(&element["label"]))
        } else {
            String::new()
        };
        let indent = "  ".repeat(*depth);
        say!("{indent}{} {key}{label} {}", refs[&key], py::str(&element["frame"]));
        if !args.interactive {
            for node in frame.owned(Some(&key)) {
                say!("{indent}  {}", describe_node(node));
            }
        }
    }
    if !unowned.is_empty() {
        say!("(owned by no element)");
        for node in unowned {
            say!("  {}", describe_node(node));
        }
    }
    Ok(0)
}

fn subtree_has(element: &Value, children: &HashMap<Option<String>, Vec<&Value>>, shown: &Option<HashSet<String>>) -> bool {
    let key = element_key(element);
    match shown {
        None => true,
        Some(set) => set.contains(&key)
            || children.get(&Some(key)).is_some_and(|list| list.iter().any(|c| subtree_has(c, children, shown))),
    }
}

fn collect<'a>(element: &'a Value, depth: usize, children: &HashMap<Option<String>, Vec<&'a Value>>,
               shown: &Option<HashSet<String>>, max_depth: Option<i64>, rows: &mut Vec<(&'a Value, usize)>) {
    if !subtree_has(element, children, shown) || max_depth.is_some_and(|d| depth as i64 > d) {
        return;
    }
    rows.push((element, depth));
    if let Some(list) = children.get(&Some(element_key(element))) {
        for child in list {
            collect(child, depth + 1, children, shown, max_depth, rows);
        }
    }
}

pub fn get(args: &mut Args) -> Result<i32> {
    let recording = recording_for(args)?;
    let start = args.start.as_deref().map(parse_ms).transpose()?;
    let end = args.end.as_deref().map(parse_ms).transpose()?;
    let frames = frames_between(&recording.frames, start, end);
    let id = args.id.clone().unwrap_or_default();
    let known = element_keys(&recording.frames);
    if !known.contains(&id) {
        fail!(format!("no element '{id}'. Known: {}", known.join(", ")), "unknown_element",
              "ddp tree, or ddp find text <part of a label>");
    }
    if args.curve {
        crate::summary::print_curves(&recording, &frames, &id);
        return Ok(0);
    }
    if ctx().json {
        for frame in &frames {
            let state = element_state(frame, &id);
            let mut row = obj(vec![("t", frame.t_value().clone()), ("present", json!(state.is_some()))]);
            if let Some(state) = state {
                for (k, v) in state.as_object().unwrap() {
                    if k != "kinds" {
                        row[k] = v.clone();
                    }
                }
                row["alpha"] = py::round(&element_alpha(frame, &id), 3);
            }
            say!("{}", py::dumps(&row));
        }
        return Ok(0);
    }
    let states: Vec<Option<Value>> = frames.iter().map(|f| element_state(f, &id)).collect();
    crate::summary::print_timeline(&id, &frames, &states);
    Ok(0)
}

fn node_fields(node: &Value, frame: &Frame) -> Map<String, Value> {
    let r = &node["frame"];
    let border = node.get("border").filter(|b| b.is_object()).cloned().unwrap_or(json!({}));
    let get = |field: &str, default: Value| node.get(field).cloned().unwrap_or(default);
    obj(vec![("t", frame.t_value().clone()), ("z", node["z"].clone()), ("kind", node["kind"].clone()),
             ("op", get("op", Value::Null)), ("owner", node["owner"].clone()), ("source", get("source", Value::Null)),
             ("fill", get("fill", Value::Null)), ("border", border.get("width").cloned().unwrap_or(json!(0))),
             ("borderColor", border.get("color").cloned().unwrap_or(Value::Null)), ("corner", get("corner", json!(0))),
             ("opacity", get("opacity", json!(1))), ("alpha", py::round(&node_alpha(node), 2)),
             ("x", r[0].clone()), ("y", r[1].clone()), ("w", r[2].clone()), ("h", r[3].clone())])
        .as_object().unwrap().clone()
}

/// Elements whose id is `value` (by id) or whose label holds it, any case (by text).
fn find_elements(recording: &Recording, by: &str, value: &str) -> Result<()> {
    let mut found: Vec<(String, Vec<(&Value, &Value)>)> = Vec::new();
    let wanted = normalize_label(value).to_lowercase();
    for frame in &recording.frames {
        for element in frame.elements() {
            let key = element_key(element);
            let hit = if by == "id" {
                key == value || element.get("id").and_then(Value::as_str) == Some(value)
            } else {
                element.get("label").and_then(Value::as_str).unwrap_or("").to_lowercase().contains(&wanted)
            };
            if hit {
                match found.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, seen)) => seen.push((frame.t_value(), element)),
                    None => found.push((key, vec![(frame.t_value(), element)])),
                }
            }
        }
    }
    if found.is_empty() {
        say!("{}", if ctx().json { "[]" } else { "no match" });
        return Ok(());
    }
    let refs = save_refs(&recording.frames)?;
    if ctx().json {
        let rows: Vec<Value> = found.iter().map(|(key, seen)| {
            let last = seen[seen.len() - 1].1;
            obj(vec![("ref", refs[key].clone().into()), ("key", key.clone().into()),
                     ("label", last.get("label").cloned().unwrap_or(Value::Null)), ("frame", last["frame"].clone()),
                     ("first", seen[0].0.clone()), ("last", seen[seen.len() - 1].0.clone()), ("frames", json!(seen.len()))])
        }).collect();
        say!("{}", py::dumps(&json!(rows)));
        return Ok(());
    }
    for (key, seen) in &found {
        let last = seen[seen.len() - 1].1;
        let label = if py::truthy(last.get("label")) && py::truthy(last.get("id")) {
            format!(" \"{}\"", py::str(&last["label"]))
        } else {
            String::new()
        };
        say!("{} {key}{label} {} present {}..{}ms ({} of {} frames)", refs[key], py::str(&last["frame"]),
             py::str(seen[0].0), py::str(seen[seen.len() - 1].0), seen.len(), recording.frames.len());
    }
    Ok(())
}

pub fn find(args: &mut Args) -> Result<i32> {
    let recording = recording_for(args)?;
    if let Some(by) = &args.by {
        let Some(value) = args.value.as_ref().filter(|_| by == "text" || by == "id") else {
            fail!("find takes `text <part of a label>`, `id <id>` or --where", "bad_find");
        };
        find_elements(&recording, by, value)?;
        return Ok(0);
    }
    let Some(where_text) = &args.where_ else {
        fail!("find needs `text <label>`, `id <id>` or --where", "missing_option", "ddp find --where 'kind=draw and alpha>0'");
    };
    let (start, end) = match &args.between {
        Some((a, b)) => (Some(parse_ms(a)?), Some(parse_ms(b)?)),
        None => (None, None),
    };
    let clauses = model::parse_where(where_text)?;
    let mut found: Vec<(&Frame, &Value)> = Vec::new();
    for frame in frames_between(&recording.frames, start, end) {
        for node in frame.nodes() {
            if model::matches(&node_fields(node, frame), &clauses)? {
                found.push((frame, node));
            }
        }
    }
    let limit = args.limit.unwrap_or(20);
    if ctx().json {
        let rows: Vec<Value> = found.iter().take(limit).map(|(f, n)| Value::Object(node_fields(n, f))).collect();
        say!("{}", py::dumps(&json!({"matches": found.len(), "rows": rows})));
        return Ok(0);
    }
    if found.is_empty() {
        say!("no match");
        return Ok(0);
    }
    let mut times: Vec<&Value> = Vec::new();
    for (frame, _) in &found {
        if !times.iter().any(|t| py::equal(t, frame.t_value())) {
            times.push(frame.t_value());
        }
    }
    times.sort_by(|a, b| py::compare(a, b));
    say!("{} matches in {} frames, t={}..{}ms", found.len(), times.len(), py::str(times[0]), py::str(times[times.len() - 1]));
    for (frame, node) in found.iter().take(limit) {
        say!("  t={} owner={} {}", py::str(frame.t_value()), py::str(&node["owner"]), describe_node(node));
    }
    if found.len() > limit {
        say!("  ... {} more (narrow --where or --between, or raise --limit)", found.len() - limit);
    }
    Ok(0)
}

fn tree_text(recording: &Recording) -> Vec<String> {
    let frame = recording.frames.last().unwrap();
    frame.elements().iter().map(|e| {
        let key = element_key(e);
        let nodes: Vec<String> = frame.nodes().iter().filter(|n| n["owner"].as_str() == Some(&key)).map(describe_node).collect();
        format!("{key} {} {}", py::str(&e["frame"]), nodes.join(" "))
    }).collect()
}

/// Two recordings; one against the last one; none: the last two takes in the history.
pub fn diff(args: &mut Args) -> Result<i32> {
    if args.recordings.len() > 2 {
        fail!("diff takes at most two recordings", "bad_diff");
    }
    let mut paths: Vec<PathBuf> = args.recordings.clone();
    if paths.len() < 2 {
        paths = if paths.is_empty() {
            let kept = model::history();
            kept[kept.len().saturating_sub(2)..].to_vec()
        } else {
            vec![paths[0].clone(), ctx().last]
        };
        if paths.len() < 2 {
            fail!("no two recordings to compare", "file_not_found", "record twice, or pass two files");
        }
    }
    let (a, b) = (paths[0].display().to_string(), paths[1].display().to_string());
    if read_text(&recording_path(&a))? == read_text(&recording_path(&b))? {
        say!("diff: {a} and {b} are the same");
        return Ok(0);
    }
    let (mut left, mut right) = (load(&paths[0])?, load(&paths[1])?);
    with_origin(&mut left);
    with_origin(&mut right);
    let Some(id) = &args.id else {
        // Like a snapshot diff: the screen at the end of each take, one line per element.
        let lines = unified_diff(&tree_text(&left), &tree_text(&right), &a, &b);
        say!("{}", if lines.is_empty() { "same screen at the end of both".to_string() } else { lines.join("\n") });
        return Ok(0);
    };
    say!("diff {id}: {a} vs {b}, t from each recording's first change, sizes in pt (repeated rows skipped)");
    let (mut previous, mut rows) = (None::<String>, 0);
    for frame in &left.frames {
        let t = frame.t - left.origin;
        let other = frame_at(&right.frames, t + right.origin);
        let differences = state_differences(&comparable_state(&left, frame, id), &comparable_state(&right, other, id), &args.ignore);
        let row = if differences.is_empty() { "same".to_string() } else { differences.join("; ") };
        if previous.as_ref() != Some(&row) {
            say!("  t={t:.0}ms {row}");
            rows += 1;
        }
        previous = Some(row);
    }
    if rows == 1 && previous.as_deref() == Some("same") {
        say!("  no differences");
    }
    Ok(0)
}

/// difflib.unified_diff with 3 lines of context, on a longest common subsequence.
pub fn unified_diff(a: &[String], b: &[String], from: &str, to: &str) -> Vec<String> {
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    // (tag, line index in a, line index in b): ' ' both, '-' a only, '+' b only.
    let mut ops: Vec<(char, usize, usize)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((' ', i, j));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            ops.push(('-', i, j));
            i += 1;
        } else {
            ops.push(('+', i, j));
            j += 1;
        }
    }
    let changed: Vec<usize> = (0..ops.len()).filter(|&k| ops[k].0 != ' ').collect();
    if changed.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!("--- {from}"), format!("+++ {to}")];
    let mut k = 0;
    while k < changed.len() {
        let start = changed[k].saturating_sub(3);
        let mut end = changed[k] + 3;
        while k + 1 < changed.len() && changed[k + 1] <= end + 3 {
            k += 1;
            end = changed[k] + 3;
        }
        let end = end.min(ops.len() - 1);
        let hunk = &ops[start..=end];
        let a_count = hunk.iter().filter(|o| o.0 != '+').count();
        let b_count = hunk.iter().filter(|o| o.0 != '-').count();
        let range = |first: usize, count: usize| match count {
            0 => format!("{first},0"),
            1 => format!("{}", first + 1),
            _ => format!("{},{count}", first + 1),
        };
        out.push(format!("@@ -{} +{} @@", range(hunk[0].1, a_count), range(hunk[0].2, b_count)));
        for &(tag, ai, bj) in hunk {
            out.push(format!("{tag}{}", if tag == '+' { &b[bj] } else { &a[ai] }));
        }
        k += 1;
    }
    out
}
