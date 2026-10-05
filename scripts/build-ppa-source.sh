#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Matthew Stanton
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
umask 022
project_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output_dir=${1:-"$project_root/dist/ppa"}
mkdir -p -- "$output_dir"
output_dir=$(cd -- "$output_dir" && pwd)
scratch=${PPA_SCRATCH_DIR:-"$HOME/.cache/agent-scratch/desktop-idle-status"}
mkdir -p -- "$scratch"
build_root=$(mktemp -d "$scratch/ppa-source-XXXXXX")
trap 'rm -rf -- "$build_root"' EXIT
source_name=$(dpkg-parsechangelog -l"$project_root/debian/changelog" -SSource)
package_version=$(dpkg-parsechangelog -l"$project_root/debian/changelog" -SVersion)
upstream_version=${package_version#*:}
upstream_version=${upstream_version%%-*}
source_ref=${PPA_SOURCE_REF:-"refs/tags/v$upstream_version"}
[[ "$source_name" == desktop-idle-status ]] || { echo 'Unexpected source package' >&2; exit 1; }
python3 "$project_root/scripts/check-versions.py"
if ! source_commit=$(git -C "$project_root" rev-parse --verify --end-of-options "$source_ref^{commit}"); then
    echo "Cannot resolve source commit: $source_ref" >&2
    exit 1
fi
SOURCE_DATE_EPOCH=$(git -C "$project_root" show -s --format=%ct "$source_commit")
[[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || { echo 'Invalid source commit timestamp' >&2; exit 1; }
export SOURCE_DATE_EPOCH
source_dir="$build_root/$source_name-$upstream_version"
orig="$build_root/${source_name}_${upstream_version}.orig.tar.xz"
git -C "$project_root" archive --format=tar --prefix="$source_name-$upstream_version/" \
    "$source_commit" -- . ':(exclude)debian' | xz -T2 > "$orig"
tar -xJf "$orig" -C "$build_root"
cp -a -- "$project_root/debian" "$source_dir/debian"
# Local pre-commit verification only: preserve the exact git-exported orig and
# capture uncommitted upstream changes in a generated quilt patch. Never upload it.
if [[ "${PPA_INCLUDE_WORKTREE:-0}" == 1 ]]; then
    [[ "$source_ref" == HEAD ]] || { echo 'Worktree validation requires PPA_SOURCE_REF=HEAD' >&2; exit 1; }
    git -C "$project_root" ls-files -z --cached --others --exclude-standard > "$build_root/worktree-files"
    while IFS= read -r -d '' path; do
        [[ "$path" == debian/* ]] && continue
        if [[ -f "$project_root/$path" ]]; then
            mkdir -p -- "$(dirname -- "$source_dir/$path")"
            cp -p -- "$project_root/$path" "$source_dir/$path"
        elif [[ -f "$source_dir/$path" ]]; then
            rm -- "$source_dir/$path"
        fi
    done < "$build_root/worktree-files"
fi
# Vendor from the exported lock, using a disk-backed cache independent of the
# maintainer's Cargo config. Only this preparation step needs network access.
export CARGO_HOME="$scratch/ppa-cargo-home"
mkdir -p -- "$CARGO_HOME"
cargo vendor --locked --manifest-path "$source_dir/service/Cargo.toml" "$source_dir/vendor" > "$build_root/cargo-config.toml"
# Cargo verifies crate checksums; the orig-vendor archive is reproducible for
# the same locked crates, independent of registry cache times and local IDs.
tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
    -C "$source_dir" -c vendor | xz -T2 > "$build_root/${source_name}_${upstream_version}.orig-vendor.tar.xz"
python3 "$project_root/scripts/vendor-copyright.py" "$source_dir/vendor" > "$source_dir/debian/copyright"
if [[ "${PPA_INCLUDE_WORKTREE:-0}" == 1 ]]; then
    (cd -- "$source_dir"; EDITOR=true dpkg-source --commit . validation-worktree)
fi
(cd -- "$source_dir"; python3 scripts/check-versions.py; dpkg-buildpackage --build=source -sa -us -uc)
source_changes="$build_root/${source_name}_${package_version}_source.changes"
lintian --fail-on error "$source_changes"
dpkg-source --extract "$build_root/${source_name}_${package_version}.dsc" "$build_root/verify-source"
python3 "$build_root/verify-source/scripts/check-versions.py"
if [[ "${PPA_BUILD_BINARY:-1}" == 1 ]]; then
    "$HOME/.local/bin/heavy" "$project_root/scripts/build-deb.sh" "$build_root/${source_name}_${package_version}.dsc" "$build_root/binary"
    cp -- "$build_root/binary/"* "$output_dir/"
fi
# Preserve artifacts from other revisions; replace only this build's named files.
while IFS= read -r -d '' artifact; do cp -- "$artifact" "$output_dir/"; done < <(find "$build_root" -maxdepth 1 -type f \( -name '*.dsc' -o -name '*.tar.xz' -o -name '*.changes' -o -name '*.buildinfo' \) -print0)
python3 "$project_root/scripts/ppa-checksums.py" "$output_dir" "$source_name" "$package_version"
echo "Verified artifacts: $output_dir"
