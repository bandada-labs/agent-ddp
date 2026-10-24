//! `check a11y`: touch targets, labels and text contrast. Labels come from the accessibility tree; how far a touch
//! reaches a control is the app's hit test, and a text's background is what the screen shows (inspect).
use super::overlap::{not_inspected, COVERED_INK};
use crate::args::Args;
use crate::inspect::{parse, VISIBLE};
use crate::model::{element_key, Recording};
use crate::py::{self, f, truthy};
use crate::say;
use crate::state::Result;
use serde_json::Value;

/// A background is one plain color when this much of what is not the text is that color or an edge between the two.
const PLAIN: f64 = 0.97;
/// A text draws in one color when this much of its ink shows that color.
const ONE_COLOR: f64 = 0.8;

fn luminance(color: [i32; 4]) -> f64 {
    let linear: Vec<f64> = color[..3]
        .iter()
        .map(|&c| {
            let c = c as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
        .collect();
    0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
}

/// A translucent color composited over an opaque background.
fn over(color: [i32; 4], background: [i32; 4]) -> [i32; 4] {
    let alpha = color[3] as f64 / 255.0;
    let mix = |i: usize| (color[i] as f64 * alpha + background[i] as f64 * (1.0 - alpha)).round_ties_even() as i32;
    [mix(0), mix(1), mix(2), 255]
}

fn contrast(text: [i32; 4], background: [i32; 4]) -> f64 {
    let (a, b) = (luminance(text), luminance(background));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn hex(color: [i32; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}FF", color[0], color[1], color[2])
}

/// A text's color, its background and how to name the text's color, when the pixels give one of each. Covered text
/// is overlap's; two colors, or a busy background, have no single ratio.
fn text_colors(text: &Value) -> Option<([i32; 4], [i32; 4], String)> {
    if text.get("ink").is_some() {
        // iOS: what the screen shows at the text's ink and clear points.
        let plain = text["inkShare"].as_f64()? >= ONE_COLOR && text["clearShare"].as_f64()? >= PLAIN;
        let seen = text["inkLikeClear"].as_f64()? < COVERED_INK;
        let (fg, background) = (parse(text["ink"].as_str()?)?, parse(text["clear"].as_str()?)?);
        return (plain && seen).then(|| (fg, background, hex(fg)));
    }
    let colors = text["colors"].as_array()?;
    let visible = text["seen"].get(0)?.as_i64()? <= VISIBLE as i64;
    let plain = text.get("uniform").map(f).unwrap_or(0.0) >= PLAIN;
    let fg = parse(colors.first()?.as_str()?)?;
    let background = parse(text["background"].as_str()?)?;
    (colors.len() == 1 && visible && plain).then(|| (fg, background, py::str(&colors[0])))
}

/// Small touch targets, controls nobody can name, and text without enough contrast (WCAG AA).
pub fn check_a11y(args: &Args, recording: &Recording) -> Result<usize> {
    let to_pt = recording.to_pt();
    let minimum = if recording.header["platform"] == "android" { 48.0 } else { 44.0 }; // pt / dp: Apple HIG and Material
    let mut problems: Vec<String> = Vec::new();
    let unlabeled =
        recording.frames.last().unwrap().elements().iter().filter(|e| truthy(e.get("tap")) && !truthy(e.get("label")));
    problems.extend(unlabeled.map(|e| format!("NO LABEL {}: a control a screen reader can't name", element_key(e))));
    let inspected = not_inspected(recording).is_none();
    // A control no touch reaches, or a disabled one, is check tappable's to report.
    for hit in recording.inspect["hit"].as_array().into_iter().flatten().filter(|h| !truthy(h.get("reason"))) {
        let Some(reach) = hit.get("reach").and_then(Value::as_array) else { continue };
        let (w, h) = (f(&reach[0]) * to_pt, f(&reach[1]) * to_pt);
        // A pixel either way is where the hit test's edge falls between pixels, not a smaller target.
        if w >= minimum - 1.0 && h >= minimum - 1.0 {
            continue;
        }
        problems.push(format!(
            "SMALL TARGET {}: a touch reaches it over {w:.0}x{h:.0}, under {minimum}x{minimum}",
            element_key(hit)
        ));
    }
    let mut unjudged: Vec<String> = Vec::new();
    let last = recording.frames.last().unwrap();
    for text in recording.inspect["texts"].as_array().into_iter().flatten().filter(|_| inspected) {
        let key = py::str(&text["key"]);
        // WCAG asks nothing of the text of a disabled control.
        if last.element(&key).is_some_and(|e| truthy(e.get("disabled"))) {
            continue;
        }
        let Some((fg, background, shown)) = text_colors(text) else {
            unjudged.push(key);
            continue;
        };
        let ratio = contrast(over(fg, background), background);
        // A text size estimated from its bitmap could be large or not: only the large-text limit holds for sure.
        let estimated = truthy(text.get("estimated"));
        let size_pt = text.get("size").map(f).unwrap_or(0.0) * to_pt;
        let large =
            estimated || size_pt >= 18.0 || (size_pt >= 14.0 && text.get("weight").map(f).unwrap_or(400.0) >= 700.0);
        let needed = if large { 3.0 } else { 4.5 };
        if ratio < needed {
            problems.push(format!(
                "LOW CONTRAST {key}: {ratio:.1}:1, {shown} on {}, needs {}:1{}",
                hex(background),
                if large { "3" } else { "4.5" },
                if estimated { " at least (its size is estimated)" } else { "" }
            ));
        }
    }
    for line in &problems {
        say!("{line}");
    }
    if !args.shared {
        if problems.is_empty() {
            say!("no accessibility problems found");
        }
        if let Some(why) = not_inspected(recording) {
            say!("touch targets and contrast not judged: {why}");
        } else if !unjudged.is_empty() {
            unjudged.sort();
            unjudged.dedup();
            say!(
                "contrast not judged (covered, more than one color, or over an image or a gradient): {}",
                unjudged.join(", ")
            );
        }
    }
    Ok(problems.len())
}
