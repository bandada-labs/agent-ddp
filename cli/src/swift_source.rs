//! Where a SwiftUI view's `body` and its modifier calls are in the app's code, from the app's own debug info: the
//! function symbols and the line table LLDB reads from the Debug image. The running app keeps no source positions.
//! Only calls into `SwiftUI.View` (the modifier methods) are listed; which one wraps which element is not claimed.
use crate::state::capture;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq)]
pub struct Body {
    pub file: String,
    pub line: u64,
    /// The modifier methods the body calls, `(name, line)`, sorted by line.
    pub calls: Vec<(String, u64)>,
}

/// The image that holds the app's code: Xcode 16 and later put a Debug build's code in `<App>.debug.dylib`.
pub fn app_image(device: &str, app: &str) -> Option<PathBuf> {
    let found = capture("xcrun", &["simctl", "get_app_container", device, app, "app"]);
    let bundle = PathBuf::from(found.stdout.trim());
    if found.code != 0 || !bundle.is_dir() {
        return None;
    }
    let debug_dylib = std::fs::read_dir(&bundle)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .find(|path| path.to_string_lossy().ends_with(".debug.dylib"));
    if debug_dylib.is_some() {
        return debug_dylib;
    }
    let plist = bundle.join("Info.plist");
    let name = capture("plutil", &["-extract", "CFBundleExecutable", "raw", &plist.to_string_lossy()]);
    Some(bundle.join(name.stdout.trim())).filter(|path| path.is_file())
}

/// Each view type's body, for the types LLDB finds in the image. Two LLDB runs: one finds the functions, one reads
/// their line tables.
pub fn bodies(image: &Path, types: &[String]) -> HashMap<String, Body> {
    let image = image.to_string_lossy().to_string();
    let lookups: Vec<String> =
        types.iter().map(|t| format!("image lookup -r -n '{}\\.body\\.getter'", t.replace('.', "\\."))).collect();
    let found = lldb(&image, &lookups);
    let functions = parse_lookup(&found, types);
    let addresses: Vec<String> = functions.values().flatten().map(|f| f.address.clone()).collect();
    if addresses.is_empty() {
        return HashMap::new();
    }
    let disassembly = lldb(&image, &addresses.iter().map(|a| format!("disassemble -a {a} -m")).collect::<Vec<_>>());
    let calls = parse_disassembly(&disassembly);
    functions
        .into_iter()
        .filter_map(|(view, functions)| {
            let body = functions.iter().find(|f| !f.closure)?;
            let mut all: Vec<(String, u64)> =
                functions.iter().flat_map(|f| calls.get(&f.address).cloned().unwrap_or_default()).collect();
            all.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
            all.dedup();
            Some((view, Body { file: body.file.clone(), line: body.line, calls: all }))
        })
        .collect()
}

fn lldb(image: &str, commands: &[String]) -> String {
    let mut args = vec!["lldb", "--no-lldbinit", "--batch", image];
    for command in commands {
        args.extend(["-o", command.as_str()]);
    }
    capture("xcrun", &args).stdout
}

#[derive(Debug, Clone)]
struct Function {
    address: String,
    closure: bool,
    file: String,
    line: u64,
}

/// `image lookup` answers, per view type: its body getter and the closures inside it, with their address and place.
fn parse_lookup(output: &str, types: &[String]) -> HashMap<String, Vec<Function>> {
    let mut found: HashMap<String, Vec<Function>> = HashMap::new();
    let mut address: Option<String> = None;
    for line in output.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("Address: ") {
            address = rest.split_once('[').and_then(|(_, a)| a.split_once(']')).map(|(a, _)| normalize_address(a));
        } else if let (Some(rest), Some(at)) = (line.strip_prefix("Summary: "), address.take()) {
            let Some((_, name)) = rest.split_once('`') else { continue };
            let Some((symbol, place)) = name.rsplit_once(" at ") else { continue };
            let Some((file, number)) = place.rsplit_once(':') else { continue };
            let Ok(number) = number.parse::<u64>() else { continue };
            for view in types {
                let getter = format!("{view}.body.getter");
                let closure = symbol.contains(&format!(" in {getter}"));
                if symbol.starts_with(&getter) || closure {
                    let function = Function { address: at.clone(), closure, file: file.to_string(), line: number };
                    found.entry(view.clone()).or_default().push(function);
                }
            }
        }
    }
    found
}

/// `disassemble -m` answers: the modifier calls of each function, `(name, line)`, by the function's address.
fn parse_disassembly(output: &str) -> HashMap<String, Vec<(String, u64)>> {
    let mut calls: HashMap<String, Vec<(String, u64)>> = HashMap::new();
    let mut function: Option<String> = None;
    let mut line_now: Option<u64> = None;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("(lldb) disassemble -a ") {
            function = rest.split_whitespace().next().map(normalize_address);
            line_now = None;
        } else if let Some(rest) = line.trim_start().strip_prefix("** ") {
            line_now = rest.split_whitespace().next().and_then(|n| n.parse().ok());
        } else if let Some((_, called)) = line.split_once("; symbol stub for: ") {
            let name = ["SwiftUI.View.", "SwiftUICore.View."]
                .iter()
                .find_map(|prefix| called.strip_prefix(prefix))
                .and_then(|rest| rest.split_once('('))
                .map(|(name, _)| name.to_string());
            if let (Some(name), Some(number), Some(function)) = (name, line_now, function.clone()) {
                calls.entry(function).or_default().push((name, number));
            }
        }
    }
    calls
}

/// Where a UIKit view was created: the frames of its allocation stack that are in the app's own image, innermost
/// first, as `(function, file, line)`. malloc_history gives the stack; atos gives each line, asked one byte before the
/// return address so it names the call itself. Frames without a line (compiler-made thunks) are left out.
pub fn creation(pid: &str, address: &str, image: &str) -> Vec<(String, String, u64)> {
    let history = capture("malloc_history", &[pid, "-fullStacks", address]).stdout;
    let returns = app_frames(&history, image);
    if returns.is_empty() {
        return Vec::new();
    }
    let calls: Vec<String> = returns.iter().map(|a| format!("0x{:x}", a.saturating_sub(1))).collect();
    let mut args = vec!["-p", pid];
    args.extend(calls.iter().map(String::as_str));
    parse_atos(&capture("atos", &args).stdout)
}

/// The return addresses of the frames in `image`, from one `malloc_history -fullStacks` allocation.
fn app_frames(history: &str, image: &str) -> Vec<u64> {
    history
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (_, owner, address) = (words.next()?.parse::<u32>().ok()?, words.next()?, words.next()?);
            (owner == image).then(|| u64::from_str_radix(address.trim_start_matches("0x"), 16).ok()).flatten()
        })
        .collect()
}

/// atos lines: `makeCell() (in probe) (main.swift:6)`.
fn parse_atos(output: &str) -> Vec<(String, String, u64)> {
    output
        .lines()
        .filter_map(|line| {
            let (function, rest) = line.split_once(" (in ")?;
            let place = rest.rsplit_once(" (")?.1.strip_suffix(')')?;
            let (file, number) = place.rsplit_once(':')?;
            let number = number.parse::<u64>().ok().filter(|n| *n > 0)?;
            Some((function.to_string(), file.to_string(), number))
        })
        .collect()
}

/// `0x6f28` and `0x0000000000006f28` are the same address.
fn normalize_address(text: &str) -> String {
    let hex = text.trim_start_matches("0x").trim_start_matches('0');
    format!("0x{}", if hex.is_empty() { "0" } else { hex })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real LLDB output on the iOS sample's Debug image (DDPSample.debug.dylib), cut to the lines that matter.
    const LOOKUP: &str = "(lldb) image lookup -r -n 'DDPSample\\.SpinnerView\\.body\\.getter'
3 matches found in /x/DDPSample.debug.dylib:
        Address: DDPSample.debug.dylib[0x000000000000704c] (DDPSample.debug.dylib.__TEXT.__text + 21676)
        Summary: DDPSample.debug.dylib`closure #1 () -> SwiftUI.TupleView<(X)> in DDPSample.SpinnerView.body.getter : some at DDPSampleApp.swift:97
        Address: DDPSample.debug.dylib[0x0000000000006f28] (DDPSample.debug.dylib.__TEXT.__text + 21384)
        Summary: DDPSample.debug.dylib`DDPSample.SpinnerView.body.getter : some at DDPSampleApp.swift:96
        Address: DDPSample.debug.dylib[0x0000000000001000] (DDPSample.debug.dylib.__TEXT.__text + 1)
        Summary: DDPSample.debug.dylib`DDPSample.BigSpinnerView.body.getter : some at Other.swift:5";

    const DISASSEMBLY: &str = "(lldb) disassemble -a 0x0000000000006f28 -m
** 103 \t        .padding(24)
DDPSample.debug.dylib[0x7018] <+240>:  bl     0xce204                   ; symbol stub for: SwiftUI.View.padding(CoreGraphics.CGFloat) -> some
(lldb) disassemble -a 0x000000000000704c -m
** 98  \t            RedrawEveryFrame().frame(width: 40, height: 40)
DDPSample.debug.dylib[0x7198] <+332>:  bl     0x74cc                    ; default argument 2 of SwiftUI.View.frame(width:)
DDPSample.debug.dylib[0x71d0] <+388>:  bl     0xce1d4                   ; symbol stub for: SwiftUI.View.frame(width: Swift.Optional<CoreGraphics.CGFloat>) -> some
** 101 \t            ForEach(0..<30, id: \\.self) { Text(\"Row\").accessibilityIdentifier(\"row\") }
DDPSample.debug.dylib[0x7300] <+500>:  bl     0xce300                   ; symbol stub for: SwiftUICore.View.accessibilityIdentifier(Swift.String) -> some";

    #[test]
    fn reads_the_body_and_its_modifier_calls() {
        let types = vec!["DDPSample.SpinnerView".to_string()];
        let functions = parse_lookup(LOOKUP, &types);
        let spinner = &functions["DDPSample.SpinnerView"];
        // The other view whose name ends the same way is not taken.
        assert_eq!(spinner.len(), 2);
        let body = spinner.iter().find(|f| !f.closure).unwrap();
        assert_eq!((body.file.as_str(), body.line, body.address.as_str()), ("DDPSampleApp.swift", 96, "0x6f28"));
        let calls = parse_disassembly(DISASSEMBLY);
        assert_eq!(calls["0x6f28"], vec![("padding".to_string(), 103)]);
        assert_eq!(calls["0x704c"], vec![("frame".to_string(), 98), ("accessibilityIdentifier".to_string(), 101)]);
    }

    #[test]
    fn reads_where_a_view_was_created() {
        // Real output: a macOS probe that allocates a view in makeCell() at main.swift:6.
        let history = "ALLOC 0x7cb50e8000-0x7cb50e827f [size=640]:
0   libsystem_malloc.dylib                0x18f828820 _malloc_zone_calloc_instrumented_or_legacy + 132
2   libobjc.A.dylib                       0x18f58481c _objc_rootAllocWithZone + 52
3   probe                                 0x104c45310 CartCell.__allocating_init(frame:) + 36  main.swift:0
4   probe                                 0x104c45198 makeCell() + 52  main.swift:0
5   probe                                 0x104c45000 main + 16  main.swift:10";
        assert_eq!(app_frames(history, "probe"), vec![0x104c45310, 0x104c45198, 0x104c45000]);
        let atos = "CartCell.__allocating_init(frame:) (in probe) (main.swift:0)
makeCell() (in probe) (main.swift:6)
main (in probe) (main.swift:10)";
        assert_eq!(
            parse_atos(atos),
            vec![
                ("makeCell()".to_string(), "main.swift".to_string(), 6),
                ("main".to_string(), "main.swift".to_string(), 10)
            ]
        );
    }

    /// Against the built iOS sample: `cargo test --release swift_source -- --ignored` after building samples/ios.
    #[test]
    #[ignore]
    fn reads_the_sample_image() {
        let image = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../samples/ios/build/Build/Products/Debug-iphonesimulator/DDPSample.app/DDPSample.debug.dylib");
        let found = bodies(&image, &["DDPSample.SpinnerView".to_string()]);
        let body = &found["DDPSample.SpinnerView"];
        assert_eq!((body.file.as_str(), body.line), ("DDPSampleApp.swift", 96));
        for call in [("frame", 98), ("accessibilityIdentifier", 101), ("padding", 103)] {
            assert!(body.calls.contains(&(call.0.to_string(), call.1)), "{call:?} in {:?}", body.calls);
        }
    }
}
