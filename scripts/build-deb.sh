#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Caller must run production builds through heavy.
set -euo pipefail
[[ $# == 2 ]] || { echo "Usage: $0 SOURCE.dsc OUTPUT_DIR" >&2; exit 2; }
dsc=$(realpath -- "$1")
mkdir -p -- "$2"
output=$(realpath -- "$2")
scratch=${PPA_SCRATCH_DIR:-"$HOME/.cache/agent-scratch/desktop-idle-status"}
mkdir -p -- "$scratch"
run=$(mktemp -d "$scratch/deb-build-XXXXXX")
trap 'rm -rf -- "$run"' EXIT
dpkg-source -x "$dsc" "$run/source"
(cd -- "$run/source"; dpkg-buildpackage --build=binary -us -uc)
for changes in "$run/"*.changes; do lintian --fail-on error "$changes"; done
while IFS= read -r -d '' artifact; do cp -- "$artifact" "$output/"; done < <(find "$run" -maxdepth 1 -type f \( -name '*.deb' -o -name '*.ddeb' -o -name '*.changes' -o -name '*.buildinfo' \) -print0)
