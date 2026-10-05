#!/usr/bin/env bash
# Source after creating $run. The private bus's harness owns/reaps its children,
# just as in kwin-bridge/run.sh; keep the bus alive until that cleanup finishes.
: "${run:?private session scratch directory required}"
active=
launching=false
pending_exit=
private_bus=false
cleanup() {
    trap '' TERM INT HUP
    if [[ -n $active ]]; then
        local harness='' parent=''
        if $private_bus && [[ -f $run/harness.pid ]]; then
            harness=$(cat "$run/harness.pid" 2>/dev/null || true)
            if [[ $harness =~ ^[0-9]+$ ]]; then
                read -r parent < <(ps -o ppid= -p "$harness") || true
            fi
        fi
        if [[ -n $parent && $parent == "$active" ]]; then
            kill -TERM "$harness" 2>/dev/null || true
        else
            # Startup/exit race: signal only the owned supervisor's session.
            kill -TERM -- "-$active" 2>/dev/null || true
        fi
        wait "$active" 2>/dev/null || true
    fi
    rm -rf -- "$run"
}
trap cleanup EXIT
cancel() {
    if $launching; then pending_exit=$1; else exit "$1"; fi
}
trap 'cancel 143' TERM
trap 'cancel 130' INT
trap 'cancel 129' HUP
run_child() {
    launching=true
    setsid "$@" &
    active=$!
    launching=false
    if [[ -n $pending_exit ]]; then exit "$pending_exit"; fi
    local status=0
    wait "$active" || status=$?
    active=
    return "$status"
}
run_private_session() {
    private_bus=true
    run_child "$@"
    private_bus=false
}
