#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Fail when upstream release versions or the locked service version differ."""
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib


def main():
    root = Path(__file__).resolve().parent.parent
    versions = {}
    for component in ("kwin-bridge", "plasmoid"):
        text = (root / component / "CMakeLists.txt").read_text()
        versions[component + " CMake"] = re.search(r"project\([^)]*VERSION\s+(\S+)", text).group(1)
        versions[component + " metadata"] = json.loads((root / component / "metadata.json").read_text())["KPlugin"]["Version"]
    versions["Cargo"] = tomllib.loads((root / "service/Cargo.toml").read_text())["package"]["version"]
    lock = tomllib.loads((root / "service/Cargo.lock").read_text())
    versions["Cargo.lock"] = next(p["version"] for p in lock["package"] if p["name"] == "desktop-idle-status")
    debian = subprocess.check_output(["dpkg-parsechangelog", "-l" + str(root / "debian/changelog"), "-SVersion"], text=True, timeout=30).strip()
    versions["Debian"] = debian.split(":")[-1].split("-")[0]
    if len(set(versions.values())) != 1:
        raise SystemExit("Release versions disagree: " + str(versions))
    print("Release versions agree: " + versions["Cargo"])


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, AttributeError, StopIteration,
            TypeError, subprocess.SubprocessError) as error:
        print(f"Cannot check release versions: {error}", file=sys.stderr)
        sys.exit(2)
