#!/usr/bin/env python3
"""Builds dist/ddp: one executable file with the CLI and its guides, to copy into another repo.
Usage: python3 cli/build.py [out path]"""
import shutil
import sys
import tempfile
import zipapp
from pathlib import Path

root = Path(__file__).resolve().parent.parent
out = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "dist" / "ddp"
with tempfile.TemporaryDirectory() as staging:
    package = Path(staging, "ddpcli")
    shutil.copytree(root / "cli" / "ddpcli", package, ignore=shutil.ignore_patterns("__pycache__"))
    # The guides travel inside the file, so `ddp skills` works where docs/ is not.
    shutil.copytree(root / "docs", package / "docs")
    out.parent.mkdir(parents=True, exist_ok=True)
    zipapp.create_archive(staging, out, interpreter="/usr/bin/env python3", main="ddpcli.cli:main", compressed=True)
print(f"{out} ({out.stat().st_size // 1024} KB)")
