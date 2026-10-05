#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
project_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output=${PPA_OUTPUT_DIR:-"$project_root/dist/ppa"}
ppa=${PPA_TARGET:-ppa:stantonmatt/plasma-visual-screensaver}
key=${PPA_SIGNING_KEY:-F78C76E2C1BE76E07CF81202B4468B1BCFFD55F5}
[[ "$ppa" =~ ^ppa:[a-z0-9][a-z0-9+.-]*/[a-z0-9][a-z0-9+.-]*$ ]] || { echo 'Invalid PPA target' >&2; exit 1; }
tree_status=$(git -C "$project_root" status --porcelain --untracked-files=all)
# The builder overlays all of debian/, including ignored build products or
# packaging hidden by excludes. None may enter a release source unnoticed.
packaging_status=$(git -C "$project_root" status --porcelain --untracked-files=all --ignored -- debian)
if [[ "${PPA_INCLUDE_WORKTREE:-0}" != 0 || -n "$tree_status" || -n "$packaging_status" ]]; then
    echo 'Publishing (including signing dry-runs) requires a clean committed tree; worktree validation artifacts cannot be uploaded.' >&2
    exit 1
fi
# Upload only a release tag. HEAD is for build validation, not publication.
[[ -z "${PPA_SOURCE_REF:-}" ]] || { echo 'Publishing requires the default upstream release tag' >&2; exit 1; }
PPA_BUILD_BINARY=${PPA_BUILD_BINARY:-0} "$project_root/scripts/build-ppa-source.sh" "$output"
version=$(dpkg-parsechangelog -l"$project_root/debian/changelog" -SVersion)
changes="$output/desktop-idle-status_${version}_source.changes"
debsign -k"$key" "$changes"
python3 "$project_root/scripts/ppa-checksums.py" "$output" desktop-idle-status "$version"
if [[ "${PPA_DRY_RUN:-0}" == 1 ]]; then
    dput --simulate --lintian "$ppa" "$changes"
else
    dput --lintian "$ppa" "$changes"
fi
