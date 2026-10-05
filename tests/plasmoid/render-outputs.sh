#!/usr/bin/env bash
# Regression: simultaneous renders sharing an output root retain separate evidence.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
scratch="${DIS_SCRATCH:-$HOME/.cache/agent-scratch/desktop-idle-status}"
mkdir -p "$scratch"
check=$(mktemp -d "$scratch/render-outputs-XXXXXX")
pids=()
cleanup() {
    trap '' TERM INT HUP
    for pid in "${pids[@]}"; do kill -TERM "$pid" 2>/dev/null || true; done
    for pid in "${pids[@]}"; do wait "$pid" 2>/dev/null || true; done
    rm -rf -- "$check"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT
trap 'exit 129' HUP
for index in 1 2; do
    env DIS_SHOT_DIR="$check/shots" DIS_STATES=late-seconds DIS_THEMES=light DIS_EXERCISE=1 \
        "$repo/tests/plasmoid/run.sh" > "$check/$index.log" 2>&1 &
    pids+=("$!")
done
status=0
for pid in "${pids[@]}"; do wait "$pid" || status=$?; done
pids=()
if [[ $status != 0 ]]; then cat "$check/1.log" "$check/2.log"; exit "$status"; fi
first=$(sed -n 's/^Render output: //p' "$check/1.log")
second=$(sed -n 's/^Render output: //p' "$check/2.log")
[[ -n $first && -n $second && $first != "$second" ]]
for output in "$first" "$second"; do
    prefix="$output/late-seconds-light"
    for suffix in compact.png full.png state.json actions.log mock.log viewer.log; do
        [[ -s "$prefix-$suffix" ]]
    done
    [[ -s "$output/light-overview.png" ]]
    rg -q '^Screen locks after 20 seconds$' "$prefix-state.json"
    rg -q 'NOTIFICATION ' "$prefix-mock.log"
done
echo 'Concurrent render outputs remain isolated'
