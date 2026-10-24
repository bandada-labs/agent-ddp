//! What the platform itself answers at the end of a live take, for the checks that must not guess (ARCHITECTURE.md):
//! the app's hit test for every control, with how far a touch reaches it, and what the app's windows show, from the
//! render server (`screen.pixels`) between two frames. Only what is the same in both frames is judged from the
//! pixels. The answer is kept in the take as an `inspect` line, so the checks read it offline too.
use crate::checks::{corner_breaks, inside_rounded, radii};
use crate::live::open;
use crate::model::{element_key, rect, tolerance_for, Frame};
use crate::py::{self, f};
use crate::state::{Result, OWNER_TOLERANCE_PT};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Apple's and Material's smallest touch target: 44 pt, 48 dp.
pub const IOS_TOUCH_PT: f64 = 44.0;
pub const ANDROID_TOUCH_DP: f64 = 48.0;
/// Two colors this close (largest channel difference, 0..255) are the same color on screen: antialiasing and color
/// management move a drawn color by less.
pub const SAME: i32 = 32;
/// A text color is on screen when some pixel of the text's frame is this close to it. Generous on purpose: a text
/// is called covered only when its color is nowhere near.
pub const VISIBLE: i32 = 48;
/// More pixels than this in a text's frame are sampled on a grid.
const MAX_PIXELS: usize = 250_000;

/// The `inspect` line for the screen as it is now.
pub fn inspect(session: &Value) -> Result<Value> {
    let mut control = open(session)?;
    let info = control.call("app.info", json!({}))?;
    let android = info["platform"] == "android";
    let scale = info.get("scale").map(f).filter(|s| *s > 0.0).unwrap_or(1.0);
    let minimum = if android { ANDROID_TOUCH_DP * scale } else { IOS_TOUCH_PT };
    let hit = control.call("screen.hit", json!({"touch": minimum}))?;
    let before = control.call("screen.frame", json!({}))?;
    let header = json!({"type": "header", "platform": info["platform"], "unit": info["unit"], "scale": scale});
    let tolerance = tolerance_for(&header);
    let frame = Frame::from(before.clone(), tolerance);
    let (texts, places) = (text_nodes(&frame), corner_places(&frame, tolerance / OWNER_TOLERANCE_PT));
    // The rectangles to look at: each text, and a little square around each corner place and right outside it.
    let mut rects: Vec<[f64; 4]> = texts.iter().map(|node| area(node)).collect();
    // Three screen pixels, in recorded units.
    let margin = 3.0 / if android { 1.0 } else { scale };
    for place in &places {
        for (x, y) in [place.point, place.outside] {
            rects.push([x - margin, y - margin, 2.0 * margin, 2.0 * margin]);
        }
    }
    let pixels = control.call("screen.pixels", json!({"rects": rects}));
    let after = control.call("screen.frame", json!({}))?;
    control.close();
    let mut line = json!({"type": "inspect", "minimum": minimum, "hit": hit["elements"], "frame": before});
    let Ok(pixels) = pixels else {
        line["pixels"] = json!("the app does not answer screen.pixels");
        return Ok(line);
    };
    let screen = Screen::from(&pixels);
    let moving = moved(&line["frame"], &after);
    line["moving"] = json!(moving.len());
    // Some(...) when the platform answers per text (iOS), even with no text on screen.
    let inks = pixels.get("texts").and_then(Value::as_array).cloned();
    line["texts"] = json!(judge_texts(&frame, &texts, &screen, inks.as_deref(), &moving));
    line["corners"] = json!(judge_corners(&places, &screen, &moving));
    Ok(line)
}

type Rgb = [i32; 3];

/// The pixels the app answered, by region, in recorded units times `px`.
pub struct Screen {
    regions: Vec<Region>,
    px: f64,
}

struct Region {
    x: f64,
    y: f64,
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Screen {
    fn from(answer: &Value) -> Screen {
        let regions = answer["regions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                Some(Region {
                    x: f(&r["x"]),
                    y: f(&r["y"]),
                    width: r["width"].as_u64()? as usize,
                    height: r["height"].as_u64()? as usize,
                    rgba: base64(r["rgba"].as_str()?)?,
                })
            })
            .collect();
        Screen { regions, px: answer.get("scale").map(f).filter(|s| *s > 0.0).unwrap_or(1.0) }
    }

    fn pixel(&self, x: f64, y: f64) -> Option<Rgb> {
        self.regions.iter().find_map(|r| {
            let (column, row) = ((x - r.x).floor(), (y - r.y).floor());
            if column < 0.0 || row < 0.0 || column as usize >= r.width || row as usize >= r.height {
                return None;
            }
            let at = (row as usize * r.width + column as usize) * 4;
            let p = r.rgba.get(at..at + 4)?;
            // Not drawn by any window, or see-through: nothing to judge there.
            (p[3] == 255).then_some([p[0] as i32, p[1] as i32, p[2] as i32])
        })
    }

    /// The pixel under a point in recorded units.
    fn at(&self, point: (f64, f64)) -> Option<Rgb> {
        self.pixel(point.0 * self.px, point.1 * self.px)
    }

    /// Every pixel inside a rectangle in recorded units, on a grid when there are too many.
    fn inside(&self, r: [f64; 4]) -> Vec<Rgb> {
        let [x, y, w, h] = r.map(|v| v * self.px);
        if w <= 0.0 || h <= 0.0 {
            return Vec::new();
        }
        let step = ((w * h) / MAX_PIXELS as f64).sqrt().max(1.0);
        let mut found = Vec::new();
        let mut row = y.floor() + 0.5;
        while row < y + h {
            let mut column = x.floor() + 0.5;
            while column < x + w {
                found.extend(self.pixel(column, row));
                column += step;
            }
            row += step;
        }
        found
    }
}

/// Standard base64 with padding, as both libraries write it.
fn base64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let bytes = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let sextets: Vec<u8> = chunk.iter().map(|&c| value(c)).collect::<Option<_>>()?;
        let bits = sextets.iter().enumerate().fold(0u32, |all, (i, s)| all | (*s as u32) << (18 - 6 * i));
        out.extend(bits.to_be_bytes()[1..].iter().take(sextets.len() - 1));
    }
    Some(out)
}

pub fn parse(color: &str) -> Option<[i32; 4]> {
    let hex = |at: usize| color.get(at..at + 2).and_then(|c| i32::from_str_radix(c, 16).ok());
    Some([hex(1)?, hex(3)?, hex(5)?, hex(7).unwrap_or(255)])
}

fn hex(color: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}FF", color[0], color[1], color[2])
}

fn distance(a: Rgb, b: Rgb) -> i32 {
    (0..3).map(|i| (a[i] - b[i]).abs()).max().unwrap()
}

/// How far a color is from the mix of two others: an antialiased edge between a text and its background is on it.
fn off_mix(p: Rgb, a: Rgb, b: Rgb) -> i32 {
    let d: Vec<f64> = (0..3).map(|i| (b[i] - a[i]) as f64).collect();
    let length = d.iter().map(|v| v * v).sum::<f64>();
    let t = if length == 0.0 {
        0.0
    } else {
        ((0..3).map(|i| (p[i] - a[i]) as f64 * d[i]).sum::<f64>() / length).clamp(0.0, 1.0)
    };
    (0..3).map(|i| (p[i] as f64 - (a[i] as f64 + t * d[i])).abs().round() as i32).max().unwrap()
}

/// The most common color among pixels (16 levels a channel, averaged inside), and its share.
fn dominant(pixels: &[Rgb]) -> Option<(Rgb, f64)> {
    let mut buckets: HashMap<(i32, i32, i32), (usize, [i64; 3])> = HashMap::new();
    for p in pixels {
        let bucket = buckets.entry((p[0] >> 4, p[1] >> 4, p[2] >> 4)).or_insert((0, [0; 3]));
        bucket.0 += 1;
        (0..3).for_each(|i| bucket.1[i] += p[i] as i64);
    }
    let (count, sums) = buckets.into_values().max_by_key(|(count, _)| *count)?;
    Some((sums.map(|s| (s / count as i64) as i32), count as f64 / pixels.len() as f64))
}

/// Rectangles where the two frames differ: a node or element that moved, changed, came or went.
fn moved(before: &Value, after: &Value) -> Vec<[f64; 4]> {
    let items = |frame: &Value, field: &str| -> Vec<Value> {
        frame[field]
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| {
                let mut same = item.clone();
                if let Some(map) = same.as_object_mut() {
                    map.remove("z");
                    map.remove("own");
                }
                same
            })
            .collect()
    };
    let mut rects = Vec::new();
    for field in ["nodes", "elements"] {
        // Value equality ignores key order: the libraries do not keep one.
        let (a, b) = (items(before, field), items(after, field));
        let gone = a.iter().filter(|item| !b.contains(item));
        let new = b.iter().filter(|item| !a.contains(item));
        rects.extend(gone.chain(new).map(|item| rect(item.get("visible").unwrap_or(&item["frame"]))));
    }
    rects
}

fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// The rectangle a node shows in: its visible part when something clips it.
fn area(node: &Value) -> [f64; 4] {
    rect(node.get("visible").unwrap_or(&node["frame"]))
}

/// The texts the recorder saw that can be judged from pixels. Left out: a see-through text (opacity), one under an
/// effect that changes its pixels (a filter, a blend mode, a mask), one in a text field (its text and placeholder
/// draw in parts), or one whose color is not known for sure.
fn text_nodes(frame: &Frame) -> Vec<&Value> {
    frame
        .nodes()
        .iter()
        .filter(|node| {
            let Some(text) = node.get("text").filter(|t| t.is_object()) else { return false };
            let in_field =
                node["owner"].as_str().and_then(|key| frame.element(key)).is_some_and(|e| e["role"] == "textbox");
            node.get("opacity").map(f).unwrap_or(1.0) >= 0.999
                && !py::truthy(node.get("effects"))
                && !py::truthy(text.get("unknownColor"))
                && !in_field
        })
        .collect()
}

/// The main color of some samples and the share of them close to it.
fn main_color(samples: &[Rgb]) -> Option<(Rgb, f64)> {
    let (color, _) = dominant(samples)?;
    Some((color, samples.iter().filter(|p| distance(**p, color) <= SAME).count() as f64 / samples.len() as f64))
}

/// Per text, what the screen shows. iOS answers per text layer with the screen's colors at its ink and clear points
/// (`ink`, `clear`): the text's own shape, the screen's colors, so a color the render server gives a text counts.
/// Otherwise: how near each of the text's colors comes on screen inside its frame, the screen's main color there,
/// and the background behind it when that is one plain color. A text that changed between the two frames is left out.
fn judge_texts(
    frame: &Frame,
    nodes: &[&Value],
    screen: &Screen,
    inks: Option<&[Value]>,
    moving: &[[f64; 4]],
) -> Vec<Value> {
    let mut found = Vec::new();
    for node in nodes {
        let text = &node["text"];
        let box_ = area(node);
        if moving.iter().any(|m| overlaps(*m, box_)) {
            continue;
        }
        let owner = node["owner"].as_str().filter(|key| frame.element(key).is_some());
        let key = owner.map(String::from).unwrap_or_else(|| format!("text at {}", py::str(&node["frame"])));
        let mut evidence = json!({"key": key, "z": node["z"], "rect": box_,
            "size": text.get("size").cloned().unwrap_or(Value::Null), "weight": text.get("weight").cloned().unwrap_or(Value::Null)});
        if py::truthy(text.get("estimated")) {
            evidence["estimated"] = json!(true);
        }
        let colors_of = |field: &str, ink: &Value| -> Vec<Rgb> {
            ink[field]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c.as_str().and_then(parse))
                .map(|c| [c[0], c[1], c[2]])
                .collect()
        };
        let ink = inks.and_then(|inks| inks.iter().find(|ink| ink["frame"] == node["frame"]));
        // A platform that answers per text (iOS) is judged only that way: its texts' own colors are not reliable.
        if ink.is_none() && inks.is_some() {
            continue;
        }
        if let Some(ink) = ink {
            let (cores, clear) = (colors_of("ink", ink), colors_of("clear", ink));
            let (Some((ink_color, ink_share)), Some((clear_color, clear_share))) =
                (main_color(&cores), main_color(&clear))
            else {
                continue;
            };
            let like_clear =
                cores.iter().filter(|p| distance(**p, clear_color) <= SAME).count() as f64 / cores.len() as f64;
            evidence["ink"] = json!(hex(ink_color));
            evidence["inkShare"] = json!(py::round_f(ink_share, 3));
            evidence["clear"] = json!(hex(clear_color));
            evidence["clearShare"] = json!(py::round_f(clear_share, 3));
            evidence["inkLikeClear"] = json!(py::round_f(like_clear, 3));
            found.push(evidence);
            continue;
        }
        let colors: Vec<String> = match text.get("colors").and_then(Value::as_array) {
            Some(all) => all.iter().map(py::str).collect(),
            None => text.get("color").map(py::str).into_iter().collect(),
        };
        let parsed: Vec<[i32; 4]> = colors.iter().filter_map(|c| parse(c)).collect();
        if parsed.is_empty() || parsed.iter().all(|c| c[3] < 0xF0) {
            continue;
        }
        let pixels = screen.inside(box_);
        let Some((main, share)) = dominant(&pixels) else { continue };
        let seen: Vec<i32> = parsed
            .iter()
            .map(|c| pixels.iter().map(|p| distance(*p, [c[0], c[1], c[2]])).min().unwrap_or(255))
            .collect();
        evidence["colors"] = json!(colors);
        evidence["seen"] = json!(seen);
        evidence["screen"] = json!(hex(main));
        evidence["share"] = json!(py::round_f(share, 3));
        if let [fg] = parsed[..] {
            let fg = [fg[0], fg[1], fg[2]];
            // Not the text's own color: the background, and edges between the two.
            let far: Vec<Rgb> = pixels.iter().copied().filter(|p| distance(*p, fg) > SAME / 2).collect();
            if let Some((background, _)) = dominant(&far) {
                let plain =
                    far.iter().filter(|p| distance(**p, background) <= SAME || off_mix(**p, fg, background) <= SAME);
                evidence["background"] = json!(hex(background));
                evidence["uniform"] = json!(py::round_f(plain.count() as f64 / far.len() as f64, 3));
            }
        }
        found.push(evidence);
    }
    found
}

/// Where the recorded geometry says a node draws past an element's rounded corners: the place, and a point right
/// outside the rounded shape's box past the corner nearest to it.
struct CornerPlace {
    key: String,
    z: Value,
    color: Rgb,
    shape: [f64; 4],
    radii: [f64; 4],
    point: (f64, f64),
    outside: (f64, f64),
}

fn corner_places(frame: &Frame, tolerance: f64) -> Vec<CornerPlace> {
    frame
        .elements()
        .iter()
        .filter_map(|element| {
            // One place per element is enough to show it.
            corner_breaks(frame, element, tolerance).into_iter().find_map(|(shape, node, point)| {
                let color = node
                    .get("fill")
                    .or_else(|| node["border"].get("color"))
                    .and_then(Value::as_str)
                    .and_then(parse)
                    .filter(|c| c[3] >= 0xF0)?;
                let [x, y, w, h] = rect(&shape["frame"]);
                let (right, bottom) = (point.0 >= x + w / 2.0, point.1 >= y + h / 2.0);
                Some(CornerPlace {
                    key: element_key(element),
                    z: node["z"].clone(),
                    color: [color[0], color[1], color[2]],
                    shape: [x, y, w, h],
                    radii: radii(shape),
                    point,
                    outside: (if right { x + w + 2.0 } else { x - 2.0 }, if bottom { y + h + 2.0 } else { y - 2.0 }),
                })
            })
        })
        .collect()
}

/// Per place: how near the node's color comes on screen there, still outside the rounded shape, and what the screen
/// shows right outside the corner. A shape that changed between the two frames is left out.
fn judge_corners(places: &[CornerPlace], screen: &Screen, moving: &[[f64; 4]]) -> Vec<Value> {
    let unit = 1.0 / screen.px;
    places
        .iter()
        .filter(|place| !moving.iter().any(|m| overlaps(*m, place.shape)))
        .filter_map(|place| {
            let (x, y) = place.point;
            let near = (-1..=1)
                .flat_map(|dx| (-1..=1).map(move |dy| (x + dx as f64 * unit, y + dy as f64 * unit)))
                .filter(|p| !inside_rounded(*p, place.shape, place.radii, unit))
                .filter_map(|p| screen.at(p))
                .min_by_key(|p| distance(*p, place.color))?;
            let behind = screen.at(place.outside)?;
            Some(json!({
                "key": place.key, "z": place.z, "point": [py::round_f(x, 1), py::round_f(y, 1)], "shape": place.shape,
                "radius": place.radii.iter().cloned().fold(0.0, f64::max), "color": hex(place.color),
                "near": distance(near, place.color), "screen": hex(near), "behind": hex(behind),
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(width: usize, height: usize, paint: impl Fn(usize, usize) -> [u8; 3]) -> Screen {
        let rgba = (0..height).flat_map(|y| (0..width).map(move |x| (x, y))).flat_map(|(x, y)| {
            let [r, g, b] = paint(x, y);
            [r, g, b, 255]
        });
        Screen { regions: vec![Region { x: 0.0, y: 0.0, width, height, rgba: rgba.collect() }], px: 1.0 }
    }

    #[test]
    fn base64_reads_what_the_libraries_write() {
        assert_eq!(base64("AAEC/w=="), Some(vec![0, 1, 2, 255]));
        assert_eq!(base64("TWFu"), Some(b"Man".to_vec()));
    }

    #[test]
    fn an_antialiased_edge_is_a_mix_not_another_color() {
        assert_eq!(off_mix([128, 128, 128], [0, 0, 0], [255, 255, 255]), 0);
        assert!(off_mix([255, 0, 0], [0, 0, 0], [255, 255, 255]) > SAME);
    }

    #[test]
    fn a_text_under_a_box_shows_its_color_nowhere() {
        let frame = Frame::from(
            json!({"type": "frame", "t": 0,
                   "elements": [{"id": "label", "frame": [10, 10, 40, 10]}],
                   "nodes": [{"z": 0, "kind": "bitmap", "frame": [10, 10, 40, 10], "text": {"color": "#000000FF", "size": 14}}]}),
            1.0,
        );
        let nodes = text_nodes(&frame);
        let covered = screen(60, 30, |_, _| [158, 158, 158]);
        assert_eq!(judge_texts(&frame, &nodes, &covered, None, &[])[0]["seen"][0], 158);
        let plain = screen(60, 30, |x, _| if x % 4 == 0 { [0, 0, 0] } else { [255, 255, 255] });
        let shown = judge_texts(&frame, &nodes, &plain, None, &[]);
        assert_eq!((shown[0]["seen"][0].as_i64(), shown[0]["background"].as_str()), (Some(0), Some("#FFFFFFFF")));
        assert_eq!(shown[0]["uniform"], 1.0);
        // While the text moves, the pixels may show another moment: not judged.
        assert!(judge_texts(&frame, &nodes, &covered, None, &[[0.0, 0.0, 20.0, 20.0]]).is_empty());
        // iOS answers with the screen at the text's own ink and clear points: the same color at both is covered.
        let gray = vec!["#8E8E93FF"; 8];
        let ink = json!([{"frame": [10, 10, 40, 10], "ink": gray, "clear": gray}]);
        let shown = judge_texts(&frame, &nodes, &covered, ink.as_array().map(Vec::as_slice), &[]);
        assert_eq!((shown[0]["inkLikeClear"].as_f64(), shown[0].get("seen")), (Some(1.0), None));
    }
}
