//! `check jank` and `check settle`: frame timing and animations that never stop.
use crate::args::Args;
use crate::model::{element_keys, Frame, Recording};
use crate::py::{self, f, truthy};
use crate::say;
use crate::state::Result;
use crate::summary::numeric_tracks;
use serde_json::Value;

const JANK_FACTOR: f64 = 3.0; // a frame interval 3x the usual one means at least two missed refreshes
const SETTLE_WINDOW_MS: f64 = 500.0;
const JITTER_PT: f64 = 1.0;

/// Missed refreshes from the frame timestamps the recorder already writes.
pub fn check_jank(args: &Args, recording: &Recording) -> Result<usize> {
    if truthy(recording.header.get("history")) {
        if !args.shared {
            say!("jank not judged: a history take keeps only the frames that changed; record live for jank");
        }
        return Ok(0);
    }
    let times: Vec<f64> = recording.frames.iter().map(|f| f.t).collect();
    let mut intervals: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    if intervals.is_empty() {
        return Ok(0);
    }
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let usual = intervals[intervals.len() / 2];
    let hitches: Vec<(f64, f64)> =
        times.windows(2).filter(|w| w[1] - w[0] > usual * JANK_FACTOR).map(|w| (w[0], w[1] - w[0])).collect();
    if let Some(worst) = py::max_first(hitches.iter(), |h| h.1) {
        let listed: Vec<String> = hitches.iter().take(8).map(|(t, gap)| format!("{t:.0}ms+{gap:.0}")).collect();
        say!(
            "JANK {} hitches over {:.0}ms (usual frame {usual:.1}ms), worst {:.0}ms at t={:.0}ms: {}{}",
            hitches.len(),
            times[times.len() - 1] - times[0],
            worst.1,
            worst.0,
            listed.join(", "),
            if hitches.len() > 8 { " ..." } else { "" }
        );
        if let Some(average) = recording.end.get("averageCaptureMs").filter(|v| !v.is_null()) {
            // Live takes end without the recorder's numbers.
            say!(
                "  capture cost per frame: average {} ms, slowest {} ms (the recorder's own share)",
                py::str(average),
                py::str(recording.end.get("slowestCaptureMs").unwrap_or(&Value::Null))
            );
        }
    } else if !args.shared {
        say!("no jank: {} frames, usual frame {usual:.1}ms", times.len());
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
