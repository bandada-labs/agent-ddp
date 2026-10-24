//! `check jank` and `check settle`: frame timing and animations that never stop.
use crate::args::Args;
use crate::model::{element_keys, Frame, Recording};
use crate::py::{self, f, truthy};
use crate::say;
use crate::state::Result;
use crate::summary::numeric_tracks;
use serde_json::Value;

const SETTLE_WINDOW_MS: f64 = 500.0;
const JITTER_PT: f64 = 1.0;

/// Refreshes the app missed, as the libraries count them from the platform's own frame times (`missed` on a frame),
/// with their own capture time left out.
pub fn check_jank(args: &Args, recording: &Recording) -> Result<usize> {
    if truthy(recording.header.get("history")) || !truthy(recording.header.get("missed")) {
        if !args.shared {
            say!("jank not judged: the take has no frame times from the platform (history, or a take from before 0.2.0); record it live");
        }
        return Ok(0);
    }
    let hitches: Vec<(f64, u64)> = recording
        .frames
        .iter()
        .filter_map(|frame| frame.v["missed"].as_u64().map(|missed| (frame.t, missed)))
        .collect();
    if let Some(worst) = py::max_first(hitches.iter(), |h| h.1 as f64) {
        let listed: Vec<String> =
            hitches.iter().take(8).map(|(t, missed)| format!("{missed} before {t:.0}ms")).collect();
        say!(
            "JANK the app missed {} in {}, worst {} before t={:.0}ms: {}{}",
            match hitches.iter().map(|h| h.1).sum::<u64>() {
                1 => "1 refresh".to_string(),
                total => format!("{total} refreshes"),
            },
            if hitches.len() == 1 { "one place".to_string() } else { format!("{} places", hitches.len()) },
            worst.1,
            worst.0,
            listed.join(", "),
            if hitches.len() > 8 { " ..." } else { "" }
        );
    } else if !args.shared {
        say!("no jank: {} frames, no refresh missed", recording.frames.len());
    }
    Ok(hitches.len())
}

/// Elements still moving at the end of the take (an animation that never stops),
/// or shaking back and forth by about a point (jitter).
pub fn check_settle(args: &Args, recording: &Recording) -> Result<usize> {
    let frames: Vec<&Frame> = recording.frames.iter().collect();
    let to_pt = recording.to_pt();
    if frames[frames.len() - 1].t - frames[0].t < 2.0 * SETTLE_WINDOW_MS {
        if !args.shared {
            say!("take too short to tell: settle needs {}ms or more", 2.0 * SETTLE_WINDOW_MS);
        }
        return Ok(0);
    }
    let window_start = frames[frames.len() - 1].t - SETTLE_WINDOW_MS;
    let mut problems = 0;
    for key in element_keys(&recording.frames) {
        for (name, points) in numeric_tracks(&frames, &key) {
            let tail: Vec<&Value> = points.iter().filter(|(t, _)| f(t) >= window_start).map(|(_, v)| v).collect();
            let steps: Vec<f64> =
                tail.windows(2).filter(|w| !py::equal(w[0], w[1])).map(|w| f(w[1]) - f(w[0])).collect();
            if steps.len() < 3 {
                continue;
            }
            let turns = steps.windows(2).filter(|w| (w[0] > 0.0) != (w[1] > 0.0)).count();
            let values: Vec<f64> = tail.iter().map(|v| f(v)).collect();
            let spread = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
                - values.iter().cloned().fold(f64::INFINITY, f64::min);
            let amplitude = spread * if ["x", "y", "w", "h", "border"].contains(&name) { to_pt } else { 1.0 };
            if turns >= 2 && amplitude <= JITTER_PT && ["x", "y", "w", "h"].contains(&name) {
                say!(
                    "JITTER {key}: {name} shakes by {amplitude:.2}pt, {turns} turns in the last {}ms",
                    SETTLE_WINDOW_MS
                );
            } else {
                say!("NOT SETTLED {key}: {name} still changes in the last {}ms ({} changes, {} -> {}). A loop, or record longer to tell",
                     SETTLE_WINDOW_MS, steps.len(), py::str(tail[0]), py::str(tail[tail.len() - 1]));
            }
            problems += 1;
            break; // one line per element
        }
    }
    if problems == 0 && !args.shared {
        say!("everything settled in the last {}ms", SETTLE_WINDOW_MS);
    }
    Ok(problems)
}
