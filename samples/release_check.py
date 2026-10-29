"""Release checks around a tag, so a release is not checked by hand.

    python3 samples/release_check.py before 0.3.0   versions and changelog agree, before the tag
    python3 samples/release_check.py after 0.3.0    GitHub release, npm, a clean install and JitPack, after it

`after` only reads public pages; nothing is published. Exits 1 when a check fails.
"""
import json
import re
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REPO = "bandada-labs/agent-ddp"
# The release workflow builds these (.github/workflows/release.yml).
BINARIES = 5


def check(ok, what, detail=""):
    print(f"{'ok  ' if ok else 'FAIL'} {what}{f': {detail}' if detail and not ok else ''}")
    return ok


def before(version):
    cargo = re.search(r'^version = "([^"]+)"', (ROOT / "cli/Cargo.toml").read_text(), re.M)
    npm = json.loads((ROOT / "npm/package.json").read_text())["version"]
    changelog = (ROOT / "CHANGELOG.md").read_text()
    unreleased = re.search(r"^## Unreleased\n(.*?)(?=^## )", changelog, re.M | re.S)
    # The lines people copy. History in STATUS.md and the changelog names old versions on purpose.
    install_lines = [line for path in [ROOT / "README.md", *(ROOT / "docs/skills").glob("*.md")]
                     for line in path.read_text().splitlines() if "debugImplementation(\"com.github" in line]
    stale = [line.strip() for line in install_lines if f"agent-ddp:v{version}" not in line]
    if stale:
        # Right for a release where only the CLI changed, so a note, not a failure.
        print(f"note install lines name another tag (fine if the libraries did not change): {stale[0]}")
    return all([
        check(cargo and cargo[1] == version, "cli/Cargo.toml version", cargo and cargo[1]),
        check(npm == version, "npm/package.json version", npm),
        check(f"## {version} (" in changelog, f"CHANGELOG.md has a {version} section"),
        check(not unreleased or not unreleased[1].strip(), "nothing left under Unreleased"),
    ])


def after(version):
    tag = f"v{version}"
    release = subprocess.run(["gh", "release", "view", tag, "--repo", REPO, "--json", "assets"],
                             capture_output=True, text=True)
    assets = json.loads(release.stdout)["assets"] if release.returncode == 0 else []
    npm = subprocess.run(["npm", "view", "agent-ddp", "version"], capture_output=True, text=True).stdout.strip()
    with tempfile.TemporaryDirectory() as folder:
        subprocess.run(["npm", "install", "--silent", f"agent-ddp@{version}"], cwd=folder, capture_output=True)
        installed = subprocess.run([str(Path(folder, "node_modules/.bin/ddp")), "--version"],
                                   capture_output=True, text=True).stdout.strip()
    pom = f"https://jitpack.io/com/github/{REPO}/{tag}/agent-ddp-{tag}.pom"
    try:
        jitpack = urllib.request.urlopen(pom, timeout=60).status
    except Exception as error:  # a 404 or a timeout: JitPack builds on the first request, ask again later
        jitpack = str(error)
    return all([
        check(len(assets) == BINARIES, f"GitHub release {tag} has {BINARIES} binaries", f"{len(assets)} found"),
        check(npm == version, "npm latest is this version", npm),
        check(installed == f"ddp {version}", "a clean npm install prints the version", installed or "nothing"),
        check(jitpack == 200, "JitPack serves the Android library", f"{jitpack} for {pom}"),
    ])


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] not in ("before", "after"):
        sys.exit(__doc__)
    sys.exit(0 if {"before": before, "after": after}[sys.argv[1]](sys.argv[2]) else 1)
