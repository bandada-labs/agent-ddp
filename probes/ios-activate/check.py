"""`ddp activate` on iOS against the sample app, with and without the library. Prints one PASS or FAIL line per check
(name, expected, got) and RESULT PASS or RESULT FAIL last.

python3 check.py DDP UDID

The sample's "Last action" text is the truth: it changes only when the button's own action ran.
"""
import json
import subprocess
import sys
import time

APP = "dev.agentddp.sample"
ddp_path, udid = sys.argv[1:3]
failed = []


def ddp(*args, device=True):
    command = [ddp_path, *args, "--json"] + (["--device", udid] if device else [])
    done = subprocess.run(command, capture_output=True, text=True, timeout=60)
    return done.returncode, done.stdout + done.stderr


def check(name, expected, got, ok):
    print(f"{'PASS' if ok else 'FAIL'} {name}: expected {expected}; got {got}")
    if not ok:
        failed.append(name)


def relaunch():
    subprocess.run(["xcrun", "simctl", "terminate", udid, APP], capture_output=True)
    subprocess.run(["xcrun", "simctl", "launch", udid, APP], capture_output=True, check=True)
    time.sleep(3)  # first frames and the launch animation


def last_action(device=True):
    """The sample's "Last action: ..." text, as the accessibility tree has it now."""
    _, out = ddp("tree", device=device)
    for line in out.split('"'):
        if line.startswith("Last action: "):
            return line[len("Last action: "):]
    return None


def code_of(out):
    try:
        return json.loads(out.strip().splitlines()[-1]).get("code")
    except (ValueError, IndexError, AttributeError):
        return None


def settled(expected, device=True):
    for _ in range(20):
        got = last_action(device)
        if got == expected:
            return got
        time.sleep(0.15)
    return last_action(device)


# Without the library: the platform's accessibility only.
relaunch()
status, out = ddp("tap", "trapIgnored")
# How tap answers is tap's business (refused, or "nothing changed"); that it pressed nothing is this check's.
print(f"INFO tap trapIgnored: exit {status}, code {code_of(out)}: {out.strip()[:200]}")
check("tap on allowsHitTesting(false) pressed nothing", "Last action: none", settled("none"), settled("none") == "none")

for key, expected in [("trapIgnored", "ignored"), ("trapAccessibilityOnly", "accessibility")]:
    relaunch()
    status, out = ddp("activate", key)
    check(f"activate {key} answers", "exit 0", f"exit {status}: {out.strip()[:300]}", status == 0)
    got = settled(expected)
    check(f"activate {key} ran its action", f"Last action: {expected}", f"Last action: {got}", got == expected)

relaunch()
status, out = ddp("activate", "animatedButton")
check("activate a plain button", "exit 0 and Selected on screen", f"exit {status}: {out.strip()[:300]}",
      status == 0 and "Selected" in ddp("tree")[1])

relaunch()
before = last_action()
status, out = ddp("activate", "trapDisabled")
check("activate a disabled button is refused", "exit 1, code no_action or refused",
      f"exit {status}, code {code_of(out)}", status != 0 and code_of(out) in ("no_action", "refused"))
check("the refused activate pressed nothing", f"Last action: {before}", f"Last action: {last_action()}",
      last_action() == before)

status, out = ddp("activate", "100,200")
check("activate a point is refused", "code bad_value", f"code {code_of(out)}", code_of(out) == "bad_value")

# With the library: ddp connects to the app, activate still goes through the platform's accessibility.
relaunch()
status, out = ddp("connect", udid, "--app", APP, "--platform", "ios", device=False)
check("connect to the library", "exit 0", f"exit {status}: {out.strip()[:300]}", status == 0)
status, out = ddp("activate", "trapAccessibilityOnly", device=False)
check("activate with the library answers", "exit 0", f"exit {status}: {out.strip()[:300]}", status == 0)
got = settled("accessibility", device=False)
check("activate with the library ran its action", "Last action: accessibility", f"Last action: {got}",
      got == "accessibility")
ddp("disconnect", device=False)

print(f"RESULT {'FAIL' if failed else 'PASS'}" + (f" ({len(failed)} failed: {', '.join(failed)})" if failed else ""))
sys.exit(1 if failed else 0)
