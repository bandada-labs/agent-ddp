#!/usr/bin/env python3
"""The hook matrix: run before each release on every OS version in docs/hooks.md.

    python3 samples/hook_matrix.py ios <simulator UDID>
    python3 samples/hook_matrix.py android <emulator serial>

Builds the sample app for the device, starts it fresh (a used screen gives other answers), then runs
`ddp doctor`, `check tappable` and `check all`. A hook that counts its answers as ok can still answer
wrong on a new OS version; the sample's traps catch that: every problem below must be reported and
nothing else. Exit 1 on any difference. Needs the release CLI (cargo build --release).
"""
import os
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DDP = ROOT / "cli/target/release/ddp"
APP = "dev.agentddp.sample"
SDK = Path(os.environ.get("ANDROID_HOME", "/opt/homebrew/share/android-commandlinetools"))
ADB = str(SDK / "platform-tools/adb")
os.environ.setdefault("ANDROID_HOME", str(SDK))

# What the sample apps must report, and nothing more. Each trap and each quiet case is in the sample's source.
EXPECTED = {
    "ios": {
        "tappable": {"DISABLED trapDisabled", "COVERED trapCovered", "UNTOUCHABLE trapIgnored", "OUTSIDE trapOutside"},
        "all": {"COVERED coveredLabel", "NOT SETTLED clockBar", "SMALL TARGET smallTarget", "SMALL TARGET smallNext",
                "LOW CONTRAST faintText", "LOW CONTRAST trapOutside", "LOW CONTRAST tapAwayButton"},
    },
    "android": {
        "tappable": {"DISABLED trapDisabled", "COVERED trapCovered", "INTERCEPTED trapIgnored", "OUTSIDE trapOutside",
                     "INTERCEPTED trapIntercepted"},
        "all": {"COVERED boxCovered", "COVERED zCovered", "OUTSIDE CORNERS hatchCard", "NOT SETTLED clockBar",
                "NO LABEL smallTarget", "NO LABEL smallNext", "SMALL TARGET smallTarget", "SMALL TARGET smallNext",
                "LOW CONTRAST faintText"},
    },
}


def run(*args, cwd=ROOT, check=True):
    result = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    if check and result.returncode != 0:
        sys.exit(f"failed: {' '.join(map(str, args))}\n{result.stdout}{result.stderr}")
    return result


def ddp(*args):
    return run(DDP, *args, check=False).stdout


def problems(output):
    """The problem lines: an upper-case kind and the element, as `SMALL TARGET smallNext`."""
    found = set()
    for line in output.splitlines():
        match = re.match(r"([A-Z][A-Z ]+[A-Z]) (\S+?)(:|$)", line)
        if match:
            found.add(f"{match[1]} {match[2]}")
    return found


def start_ios(udid):
    build = ROOT / "samples/ios/build"
    run("xcodebuild", "-project", "DDPSample.xcodeproj", "-scheme", "DDPSample", "-destination", f"id={udid}",
        "-derivedDataPath", build, "build", cwd=ROOT / "samples/ios")
    run("xcrun", "simctl", "install", udid, build / "Build/Products/Debug-iphonesimulator/DDPSample.app")
    run("xcrun", "simctl", "launch", "--terminate-running-process", udid, APP)
    version = run("xcrun", "simctl", "getenv", udid, "SIMULATOR_RUNTIME_VERSION").stdout.strip()
    return f"iOS {version} simulator"


def start_android(serial):
    env = {**os.environ, "ANDROID_SERIAL": serial}
    subprocess.run(["./gradlew", "-q", ":app:installDebug"], cwd=ROOT / "samples/android", env=env, check=True)
    run(ADB, "-s", serial, "shell", "am", "force-stop", APP)
    run(ADB, "-s", serial, "shell", "am", "start", "-W", "-n", f"{APP}/.MainActivity")
    api = run(ADB, "-s", serial, "shell", "getprop", "ro.build.version.sdk").stdout.strip()
    return f"API {api} emulator"


def main():
    platform, device = sys.argv[1], sys.argv[2]
    where = start_ios(device) if platform == "ios" else start_android(device)
    # The app opens its endpoint a moment after launch.
    for _ in range(10):
        time.sleep(2)
        connect = run(DDP, "connect", device, "--app", APP, check=False)
        if connect.returncode == 0:
            break
    else:
        sys.exit(f"could not connect: {connect.stdout}{connect.stderr}")
    print(connect.stdout.strip())
    doctor = ddp("doctor")
    hooks = [line for line in doctor.splitlines() if " hook " in line]
    print(f"{where}:")
    print("\n".join(hooks))
    differences = [f"doctor: {line}" for line in hooks if line.startswith("FAIL")]
    for check in ("tappable", "all"):
        found, expected = problems(ddp("check", check)), EXPECTED[platform][check]
        differences += [f"check {check}: missing {p}" for p in sorted(expected - found)]
        differences += [f"check {check}: new {p}" for p in sorted(found - expected)]
    ddp("disconnect")
    print("\n".join(differences) or f"PASS: every hook answers and every trap matches on {where}")
    sys.exit(1 if differences else 0)


if __name__ == "__main__":
    main()
