//! `baseline` and `check baseline`: a kept take to compare against, with JUnit output.
use super::{comparable_state, state_differences};
use crate::args::Args;
use crate::live::recording_for;
use crate::model::{element_keys, frame_at, load, read_text, recording_path, with_origin};
use crate::state::{ctx, write_file, Result};
use crate::{fail, say};
use std::path::Path;

const BASELINE_DIR: &str = "ddp-baselines";

/// Keep the last take as a baseline next to the code, so it can be committed and checked in CI.
pub fn baseline(args: &mut Args) -> Result<i32> {
    let source = match &args.file {
        Some(file) => recording_path(&file.to_string_lossy()),
        None => ctx().last,
    };
    if !source.exists() {
        fail!(format!("{} not found", source.display()), "file_not_found", "ddp record first");
    }
    let name = args.name.clone().unwrap_or_default();
    let target = Path::new(BASELINE_DIR).join(format!("{name}.jsonl.gz"));
    write_file(&target, crate::model::gzip(read_text(&source)?.as_bytes()))?;
    let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
    say!("baseline {name} -> {} ({size} bytes)", target.display());
    Ok(0)
}

pub fn check_baseline(args: &Args) -> Result<usize> {
    let Some(against) = &args.against else {
        fail!(
            "check baseline needs --against <baseline name>",
            "missing_option",
            "ddp baseline save NAME, then --against NAME"
        );
    };
    let name = against.display().to_string();
    let stored = Path::new(BASELINE_DIR).join(format!("{name}.jsonl.gz"));
    let path = if stored.exists() { stored } else { recording_path(&name) };
    let mut expected = load(&path)?;
    let mut actual = recording_for(args)?;
    with_origin(&mut expected);
    with_origin(&mut actual);
    let mut keys = element_keys(&expected.frames);
    for key in element_keys(&actual.frames) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    type FirstChange = Option<(f64, Vec<String>)>;
    let mut results: Vec<(String, FirstChange)> = Vec::new();
    for key in keys {
        let mut first = None;
        for frame in &actual.frames {
            let t = frame.t - actual.origin;
            let other = frame_at(&expected.frames, t + expected.origin);
            let found = state_differences(
                &comparable_state(&expected, other, &key),
                &comparable_state(&actual, frame, &key),
                &[],
            );
            if !found.is_empty() {
                first = Some((t, found));
                break;
            }
        }
        if let Some((t, found)) = &first {
            say!(
                "CHANGED {key} from t={t:.0}ms: {} (baseline vs now)",
                found.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
            );
        }
        results.push((key, first));
    }
    let changed = results.iter().filter(|(_, first)| first.is_some()).count();
    say!("baseline {name}: {changed} of {} elements changed", results.len());
    if let Some(junit) = &args.junit {
        let cases: Vec<(String, Option<String>)> = results
            .iter()
            .map(|(key, first)| {
                (key.clone(), first.as_ref().map(|(t, found)| format!("from t={t:.0}ms: {}", found.join("; "))))
            })
            .collect();
        write_junit(junit, &format!("ddp baseline {name}"), &cases)?;
    }
    Ok(changed)
}

fn quote_attribute(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('>', "&gt;")
        .replace('<', "&lt;")
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
        .replace('\t', "&#9;");
    if escaped.contains('"') {
        if escaped.contains('\'') {
            format!("\"{}\"", escaped.replace('"', "&quot;"))
        } else {
            format!("'{escaped}'")
        }
    } else {
        format!("\"{escaped}\"")
    }
}

/// cases: (name, failure message or None).
fn write_junit(path: &Path, suite: &str, cases: &[(String, Option<String>)]) -> Result<()> {
    let failures = cases.iter().filter(|(_, m)| m.is_some()).count();
    let mut lines =
        vec![format!("<testsuite name={} tests=\"{}\" failures=\"{failures}\">", quote_attribute(suite), cases.len())];
    for (name, message) in cases {
        lines.push(match message {
            Some(message) => format!(
                "  <testcase name={}><failure message={}/></testcase>",
                quote_attribute(name),
                quote_attribute(message)
            ),
            None => format!("  <testcase name={}/>", quote_attribute(name)),
        });
    }
    lines.push("</testsuite>".into());
    write_file(path, lines.join("\n") + "\n")
}
