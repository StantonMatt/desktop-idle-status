#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Read-only comparison against Ubuntu's published Release/Updates/Security.

Exit 0: same KWin upstream version; 1: rebuild needed; 2: unavailable/error.
Default baseline is the latest changelog entry. --ppa reads the dependency of
our latest published bridge .deb, avoiding guesses based on publication dates.
"""
import argparse
from http.client import HTTPException
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from urllib.parse import urlencode
from urllib.request import urlopen

API = "https://api.launchpad.net/1.0"
ROOT = Path(__file__).resolve().parent.parent


def request(url, **params):
    if params:
        url += ("&" if "?" in url else "?") + urlencode(params)
    with urlopen(url, timeout=60) as response:
        return json.load(response)


def binaries(archive, name, series, arch, **params):
    url = archive + "?" + urlencode(dict(ws_op="getPublishedBinaries")).replace("ws_op", "ws.op")
    url += "&" + urlencode(dict(binary_name=name, exact_match="true", status="Published",
                                distro_arch_series=f"{API}/ubuntu/{series}/{arch}", **params))
    entries = []
    while url:
        page = request(url)
        entries.extend(page["entries"])
        url = page.get("next_collection_link")
    return entries


def newest(entries):
    version = None
    chosen = None
    for entry in entries:
        candidate = entry["binary_package_version"]
        if version is None:
            newer = True
        else:
            result = subprocess.run(["dpkg", "--compare-versions", candidate, "gt", version],
                                    check=False, timeout=30)
            if result.returncode not in (0, 1):
                raise subprocess.CalledProcessError(result.returncode, result.args)
            newer = result.returncode == 0
        if newer:
            version, chosen = candidate, entry
    if chosen is None:
        raise ValueError("No matching published binaries found")
    return chosen


def upstream(version):
    return version.split(":")[-1].rsplit("-", 1)[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ppa", action="store_true", help="Use the latest published PPA bridge instead of local changelog")
    parser.add_argument("--series", default="resolute")
    parser.add_argument("--arch", default="amd64")
    parser.add_argument("--built-version", help="Explicit libkwin6 build version (for validation)")
    parser.add_argument("--archive-version", help="Explicit archive libkwin6 version (for validation)")
    args = parser.parse_args()
    built = args.built_version
    if built is None and args.ppa:
        archive = f"{API}/~stantonmatt/+archive/ubuntu/plasma-visual-screensaver"
        entry = newest(binaries(archive, "desktop-idle-status-kwin-bridge", args.series, args.arch))
        urls = request(entry["self_link"], **{"ws.op": "binaryFileUrls"})
        url = next(u for u in urls if u.endswith(".deb"))
        scratch = Path(os.environ.get("PPA_SCRATCH_DIR", Path.home() / ".cache/agent-scratch/desktop-idle-status"))
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="kwin-check-", dir=scratch) as run:
            deb = Path(run) / "bridge.deb"
            with urlopen(url, timeout=60) as response:
                deb.write_bytes(response.read())
            depends = subprocess.check_output(["dpkg-deb", "-f", str(deb), "Depends"], text=True, timeout=30)
        built = re.search(r"libkwin6\s*\(>=\s*([^\s)]+)\)", depends).group(1)
    elif built is None:
        entry = (ROOT / "debian/changelog").read_text().split("\n -- ", 1)[0]
        match = re.search(r"Bridge built against libkwin6\s+(\S+)\.", entry)
        if match is None:
            raise ValueError("Latest changelog entry must record 'Bridge built against libkwin6 VERSION.'")
        built = match.group(1)
    current = args.archive_version
    if current is None:
        entries = []
        for pocket in ("Release", "Updates", "Security"):
            entries += binaries(f"{API}/ubuntu/+archive/primary", "libkwin6", args.series, args.arch, pocket=pocket)
        current = newest(entries)["binary_package_version"]
    print(f"Bridge libkwin6: {built}; Ubuntu {args.series}/{args.arch}: {current}")
    if upstream(built) != upstream(current):
        print("REBUILD NEEDED: bump the PPA revision, record the new libkwin6 version, and rebuild/publish.")
        return 1
    print("No rebuild needed: KWin upstream version is unchanged (Ubuntu-only revisions do not change the IID).")
    return 0


def cli():
    try:
        return main()
    except (OSError, ValueError, KeyError, AttributeError, StopIteration,
            TypeError, HTTPException, subprocess.SubprocessError) as error:
        print(f"Cannot determine KWin rebuild status: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(cli())
