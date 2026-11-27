#!/bin/bash
# What the Mac runner's ddp-dev job runs from the bundle's clone: probes/ios-activate/run.sh MODE RUN_FOLDER.
#   probe   lists AccessibilityPlatformTranslation's classes and disassembles the methods behind a press (no simulator)
#   check   builds ddp and the sample, then checks `ddp activate` on a simulator of its own (deleted at the end)
# Each check prints PASS or FAIL with what it expected and got; the last line is RESULT PASS or RESULT FAIL.
set -euo pipefail
mode=${1:?probe or check}
run=${2:?run folder}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)

probe() {
    swiftc -O -o "$run/axp_dump" "$here/axp_dump.swift"
    "$run/axp_dump" > "$run/axp_dump.txt"
    grep -v '^DISASSEMBLE ' "$run/axp_dump.txt"
    # Disassembled inside the dump's own process: the framework is in the shared cache, not a file lldb can open.
    # macOS bash is 3.2: no coproc. The dump waits on a fifo and writes its pid to a file.
    mkfifo "$run/hold"
    "$run/axp_dump" --wait < "$run/hold" > "$run/waiting.txt" &
    local dump=$!
    exec 3> "$run/hold"
    local pid=""
    for _ in $(seq 50); do
        pid=$(sed -n 's/^READY //p' "$run/waiting.txt")
        [[ -n $pid ]] && break
        sleep 0.1
    done
    if [[ -z $pid ]]; then
        echo "FAIL the dump never said READY"
        echo "RESULT FAIL"
        return 1
    fi
    local commands=()
    while read -r name; do
        commands+=(-o "disassemble -n '${name#DISASSEMBLE }'")
    done < <(grep '^DISASSEMBLE ' "$run/axp_dump.txt")
    # bash 3.2 calls an empty array unbound under set -u.
    if [[ ${#commands[@]} -gt 0 ]]; then
        lldb --batch -p "$pid" "${commands[@]}" -o detach 2>&1 | grep -v '^warning:' || true
    else
        echo "nothing to disassemble"
    fi
    echo >&3
    exec 3>&-
    wait "$dump" || true
    echo "RESULT PASS"
}

case $mode in
    probe) probe ;;
    *) echo "FAIL no mode $mode yet"; echo "RESULT FAIL"; exit 1 ;;
esac
