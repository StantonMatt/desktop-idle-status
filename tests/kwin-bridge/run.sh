#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
scratch="$HOME/.cache/agent-scratch/desktop-idle-status"
mkdir -p "$scratch"
run=$(mktemp -d "$scratch/nested-XXXXXX")
active=
launching=false
pending_exit=
private_bus=false
cleanup() {
    trap '' TERM INT HUP
    if [[ -n "$active" ]]; then
        if $private_bus && [[ -f "$run/harness.pid" ]]; then
            # Python owns the separately grouped compositor/clients. Let its
            # finally block finish before the private bus or directories go away.
            local harness parent
            harness=$(cat "$run/harness.pid" 2>/dev/null || true)
            parent=
            if [[ "$harness" =~ ^[0-9]+$ ]]; then
                read -r parent < <(ps -o ppid= -p "$harness") || true
            fi
            if [[ "$parent" == "$active" ]]; then
                kill -TERM "$harness" 2>/dev/null || true
            else
                # Startup/exit race: only signal our supervisor's group, never
                # a stale or reused harness PID from the private pid file.
                kill -TERM -- "-$active" 2>/dev/null || true
            fi
        else
            kill -TERM -- "-$active" 2>/dev/null || true
        fi
        wait "$active" 2>/dev/null || true
    fi
    rm -rf -- "$run"
}
trap cleanup EXIT
cancel() {
    if $launching; then
        pending_exit=$1
    else
        exit "$1"
    fi
}
trap 'cancel 143' TERM
trap 'cancel 130' INT
trap 'cancel 129' HUP
run_child() {
    launching=true
    setsid "$@" &
    active=$!
    launching=false
    if [[ -n "$pending_exit" ]]; then exit "$pending_exit"; fi
    local status=0
    wait "$active" || status=$?
    active=
    return "$status"
}
run_child cmake -S "$repo/kwin-bridge" -B "$run/bridge-build" -G Ninja
run_child cmake --build "$run/bridge-build"
run_child cmake -S "$repo/tests/kwin-bridge" -B "$run/client-build" -G Ninja
run_child cmake --build "$run/client-build"
grep -aq 'org.kde.kwin.PluginFactoryInterface6\.6\.6' \
    "$run/bridge-build/plugins/kwin/plugins/desktopidlestatusbridge.so"

# No inherited desktop display, activation variables, or service directories.
mkdir -m 700 "$run/runtime" "$run/config" "$run/cache" "$run/data" "$run/state" "$run/home"
cat > "$run/bus.conf" <<BUS
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=$run/runtime</listen>
  <auth>EXTERNAL</auth>
  <apparmor mode="disabled"/>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>
BUS
private_bus=true
run_child env -i PATH=/usr/bin:/bin HOME="$run/home" USER="$(id -un)" LOGNAME="$(id -un)" \
    LANG=C.UTF-8 XDG_RUNTIME_DIR="$run/runtime" XDG_CONFIG_HOME="$run/config" \
    XDG_CACHE_HOME="$run/cache" XDG_DATA_HOME="$run/data" XDG_STATE_HOME="$run/state" \
    QT_PLUGIN_PATH="$run/bridge-build/plugins:$run/client-build/plugins" QT_QUICK_BACKEND=software \
    KWIN_COMPOSE=Q LIBGL_ALWAYS_SOFTWARE=1 DIS_NESTED_TEST_ROOT="$run" DIS_RUN_WRAPPER_PID="$$" \
    dbus-run-session --config-file="$run/bus.conf" -- /usr/bin/python3 "$repo/tests/kwin-bridge/integration.py" \
        "$run/client-build/inhibitor-client" "${1:-}"
private_bus=false
if [[ "${1:-}" != --cancellation-probe ]]; then
    run_child /usr/bin/python3 "$repo/tests/kwin-bridge/cancellation.py" "$repo/tests/kwin-bridge/run.sh" "$scratch"
fi
