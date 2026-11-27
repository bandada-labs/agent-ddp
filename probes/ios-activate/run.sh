#!/bin/bash
# What the Mac runner's ddp-dev job runs from the bundle's clone: probes/ios-activate/run.sh MODE RUN_FOLDER.
#   probe   prints the request AccessibilityPlatformTranslation makes for a press, and its classes (no simulator)
#   check   builds ddp and the sample, then checks `ddp activate` on a simulator of its own (deleted at the end)
# Each check prints PASS or FAIL with what it expected and got; the last line is RESULT PASS or RESULT FAIL.
set -euo pipefail
mode=${1:?probe or check}
run=${2:?run folder}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)

probe() {
    # Runtime only: no lldb, no simulator, nothing that asks the Mac's owner for anything.
    swiftc -O -o "$run/axp_dump" "$here/axp_dump.swift"
    "$run/axp_dump"
}

# The newest iPhone on the newest iOS runtime, as "devicetype runtime".
newest_iphone() {
    xcrun simctl list -j devicetypes runtimes | python3 -c '
import json, sys
data = json.load(sys.stdin)
runtime = [r for r in data["runtimes"] if r["platform"] == "iOS" and r["isAvailable"]][-1]
phones = [t["identifier"] for t in runtime.get("supportedDeviceTypes", []) if t.get("productFamily") == "iPhone"]
print(phones[-1], runtime["identifier"])'
}

check() {
    local ddp="$repo/cli/target/release/ddp"
    [[ -x $ddp ]] || cargo build --release --offline --manifest-path "$repo/cli/Cargo.toml"
    (cd "$repo/samples/ios" && xcodegen -q)
    xcodebuild -quiet -project "$repo/samples/ios/DDPSample.xcodeproj" -scheme DDPSample \
        -destination 'generic/platform=iOS Simulator' -derivedDataPath "$run/derived" build
    local app="$run/derived/Build/Products/Debug-iphonesimulator/DDPSample.app"
    read -r device_type runtime < <(newest_iphone)
    # A simulator of its own, never the runner's: deleted whatever happens.
    udid=$(xcrun simctl create ddp-dev-activate "$device_type" "$runtime")
    trap 'xcrun simctl shutdown "$udid" 2>/dev/null; xcrun simctl delete "$udid"' EXIT
    echo "simulator $udid ($device_type, $runtime)"
    xcrun simctl boot "$udid"
    xcrun simctl bootstatus "$udid" -b > /dev/null
    xcrun simctl install "$udid" "$app"
    "$ddp" version
    python3 "$here/check.py" "$ddp" "$udid"
}

case $mode in
    probe) probe ;;
    check) check ;;
    *) echo "FAIL no mode $mode yet"; echo "RESULT FAIL"; exit 1 ;;
esac
