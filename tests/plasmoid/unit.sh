#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
scratch="${DIS_SCRATCH:-$HOME/.cache/agent-scratch/desktop-idle-status}"
mkdir -p "$scratch"
run=$(mktemp -d "$scratch/u-XXXXXX")
# shellcheck source=tests/owned-session.sh
source "$repo/tests/owned-session.sh"
mkdir -m 700 "$run"/{home,runtime,config,data,cache,state,bin}
cat > "$run/bus.conf" <<BUS
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path=$run/bus</listen><auth>EXTERNAL</auth><apparmor mode="disabled"/><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>
BUS
cat > "$run/bin/systemctl" <<'FAKE'
#!/bin/sh
printf '%s\n' "$$" >> "$DIS_COMMAND_PIDS"
exec sleep 60
FAKE
chmod +x "$run/bin/systemctl"
run_private_session env -i PATH="$run/bin:/usr/bin:/bin" DIS_COMMAND_PIDS="$run/command-pids" HOME="$run/home" LANG=es_CL.UTF-8 XDG_RUNTIME_DIR="$run/runtime" \
 XDG_CONFIG_HOME="$run/config" XDG_DATA_HOME="$run/data" XDG_CACHE_HOME="$run/cache" XDG_STATE_HOME="$run/state" \
 QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software \
 DIS_SESSION_ROOT="$run" DIS_RUN_WRAPPER_PID="$$" DIS_CANCELLATION_PROBE="${DIS_CANCELLATION_PROBE:-0}" \
 dbus-run-session --config-file="$run/bus.conf" -- /usr/bin/python3 "$repo/tests/plasmoid/session.py" unit "$repo" "$run"

# Both retries must really execute, and timeout must reap both pending commands.
[[ $(wc -l < "$run/command-pids") == 2 ]]
while read -r pid; do
    if kill -0 "$pid" 2>/dev/null; then
        echo "Pending test systemctl survived its deadline: $pid" >&2
        exit 1
    fi
done < "$run/command-pids"
