#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
set -eu

rustc=${1:-/usr/bin/rustc}
libdir=$("$rustc" --print target-libdir)
# Query the actual std archive, not the rust-defaults compiler metapackage.
set -- "$libdir"/libstd-*.rlib
if [ ! -f "$1" ]; then
    echo "Rust standard library archive not found in $libdir" >&2
    exit 1
fi
owner=$(dpkg-query --search "$1")
package=${owner%%: /*}
dpkg-query --show --showformat='${source:Package} (= ${source:Version})\n' "$package"
