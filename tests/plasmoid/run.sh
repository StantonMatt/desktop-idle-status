#!/usr/bin/env bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
scratch="${DIS_SCRATCH:-$HOME/.cache/agent-scratch/desktop-idle-status}"
shotroot="${DIS_SHOT_DIR:-$scratch/plasmoid-shots}"
mkdir -p "$shotroot" "$scratch"
# Even explicit output roots get a unique child; concurrent invocations never
# truncate another run's logs, captures, actions or contact sheets.
shotdir=$(mktemp -d "$shotroot/run-XXXXXX")
echo "Render output: $shotdir"
run=$(mktemp -d "$scratch/p-XXXXXX")
# shellcheck source=tests/owned-session.sh
source "$repo/tests/owned-session.sh"
if [[ ${DIS_CANCELLATION_PROBE:-0} != 1 ]]; then
    # pkg-config emits separate compiler/linker arguments.
    # shellcheck disable=SC2046
    run_child g++ -shared -fPIC -std=c++17 "$repo/tests/plasmoid/capture.cpp" -o "$run/capture.so" $(pkg-config --cflags --libs Qt6Quick Qt6DBus)
fi
mkdir -m 700 "$run"/{home,runtime,config,data,cache,state,bin}
cat > "$run/bus.conf" <<BUS
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path=$run/bus</listen><auth>EXTERNAL</auth><apparmor mode="disabled"/><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>
BUS
mkdir -p "$run/data/knotifications6" "$run/data/icons/hicolor/scalable/apps"
cp "$repo/plasmoid/desktop-idle-status.notifyrc" "$run/data/knotifications6/"
cp "$repo/plasmoid/contents/icons/ready-32.svg" "$run/data/icons/hicolor/scalable/apps/desktop-idle-status.svg"
cat > "$run/bin/systemctl" <<'FAKE'
#!/bin/sh
printf 'SYSTEMCTL %s\n' "$*" >> "$DIS_ACTION_LOG"
gdbus call --session --dest org.example.Fixture --object-path /Fixture --method org.example.Fixture.StartService >/dev/null
FAKE
cat > "$run/bin/systemsettings" <<'FAKE'
#!/bin/sh
printf 'KCM %s\n' "$*" >> "$DIS_ACTION_LOG"
FAKE
cp "$run/bin/systemsettings" "$run/bin/kcmshell6"
chmod +x "$run/bin/systemctl" "$run/bin/systemsettings" "$run/bin/kcmshell6"
for theme in ${DIS_THEMES:-light dark}; do
    scheme=BreezeLight; [[ $theme != dark ]] || scheme=BreezeDark
    printf '[Formats]\nLANG=es_CL.UTF-8\nLC_TIME=es_CL.UTF-8\n' > "$run/config/plasma-localerc"
    cp "/usr/share/color-schemes/$scheme.colors" "$run/config/kdeglobals"
    printf '\n[General]\nColorScheme=%s\n' "$scheme" >> "$run/config/kdeglobals"
    for state in ${DIS_STATES:-ready blocked-one blocked-many unidentified late screensaver-off running service-down unknown-kwin empty long loading ready-one late-fractional late-seconds late-power-lock timeout-unknown timeout-missing unknown-initializing unknown-policy unknown-future history-error clear-error markup caption retention}; do
        prefix="$shotdir/$state-$theme"
        : > "$prefix-actions.log"
        run_private_session env -i PATH="$run/bin:/usr/bin:/bin" HOME="$run/home" USER="$(id -un)" LANG=es_CL.UTF-8 LC_TIME=es_CL.UTF-8 \
            XDG_RUNTIME_DIR="$run/runtime" XDG_CONFIG_HOME="$run/config" XDG_DATA_HOME="$run/data" XDG_DATA_DIRS=/usr/share \
            XDG_CACHE_HOME="$run/cache" XDG_STATE_HOME="$run/state" QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software QT_QPA_PLATFORMTHEME=kde QT_QUICK_CONTROLS_STYLE=org.kde.desktop \
            DIS_SESSION_ROOT="$run" DIS_RUN_WRAPPER_PID="$$" DIS_CANCELLATION_PROBE="${DIS_CANCELLATION_PROBE:-0}" \
            DIS_INSTANCES="${DIS_INSTANCES:-1}" DIS_TEST_STATE="$state" DIS_THEME="$theme" DIS_SHOT_PREFIX="$prefix" DIS_ACTION_LOG="$prefix-actions.log" DIS_EXERCISE="${DIS_EXERCISE:-0}" \
            dbus-run-session --config-file="$run/bus.conf" -- /usr/bin/python3 "$repo/tests/plasmoid/session.py" render "$repo" "$run" "$state" "$prefix"
        expected=$state
        case "$state" in blocked-one|blocked-many|unidentified|long|markup|caption) expected=blocked;; empty|ready-one|timeout-unknown|timeout-missing|history-error|clear-error|retention) expected=ready;; unknown-*) expected=unknown;; late-fractional|late-seconds|late-power-lock) expected=late;; esac
        actual=$(head -n1 "$prefix-state.json")
        [[ "$actual" == "$expected" ]] || { echo "Expected $expected, got $actual" >&2; exit 1; }
        if rg 'file://.*/plasmoid/.*(TypeError|ReferenceError|Error:)' "$prefix-viewer.log"; then exit 1; fi
        # Only the explicitly exercised Start Service action may launch systemctl.
        starts=$(rg -c '^SYSTEMCTL ' "$prefix-actions.log" || true)
        if [[ ${DIS_EXERCISE:-0} == 1 && $state == service-down ]]; then
            [[ $starts == 1 ]]
            rg -q 'TRIGGER startServiceAction' "$prefix-viewer.log"
        else
            [[ -z $starts ]] || { echo "Unexpected service start in $state" >&2; exit 1; }
        fi
        case "$state" in
            unknown-*|screensaver-off)
                if rg -q 'RAW ERROR|org.freedesktop.DBus.Error' "$prefix-state.json"; then
                    echo "Raw diagnostic shown in $state" >&2; exit 1
                fi
                rg -q 'RAW ERROR' "$prefix-viewer.log";;
            timeout-unknown|timeout-missing) [[ -z $(sed -n '3p' "$prefix-state.json") && -z $(sed -n '4p' "$prefix-state.json") ]];;
            history-error) rg -q 'HISTORY_ERROR visible' "$prefix-viewer.log";;
            retention) rg -q 'RETENTION expired on clock advance' "$prefix-viewer.log";;
            markup) rg -q '<b>Firefox & Co</b>' "$prefix-state.json";;
            ready-one) rg -q '^After 1 minute of inactivity$' "$prefix-state.json";;
            late-seconds) rg -q '^Screen locks after 20 seconds$' "$prefix-state.json";;
            late-power-lock) rg -q 'POWER_LOCK correct settings page' "$prefix-viewer.log";;
            late-fractional) rg -q '^Screen locks after 1 minute$' "$prefix-state.json";;
        esac
        if [[ ${DIS_EXERCISE:-0} == 1 && $state != loading ]]; then
            rg -q 'NOTIFICATION ' "$prefix-mock.log"
            if [[ ${DIS_INSTANCES:-1} == 2 ]]; then
                [[ $(rg -c '^NOTIFICATION ' "$prefix-mock.log") == 1 ]]
                [[ $(rg -c '^CALL ClaimReturnNotice ' "$prefix-mock.log") == 2 ]]
                rg -q '^CLAIM True$' "$prefix-mock.log"
                rg -q '^CLAIM False$' "$prefix-mock.log"
                rg -q 'NOTIFICATION_ACTION expanded= QVariant\(bool, true\)' "$prefix-viewer.log" "$prefix-second-viewer.log"
            else
                rg -q 'NOTIFICATION_ACTION expanded= QVariant\(bool, true\)' "$prefix-viewer.log"
            fi
            case "$state" in
                clear-error) rg -q 'CLEAR_ERROR visible' "$prefix-viewer.log";;
                caption) rg -q 'Report — Firefox' "$prefix-mock.log";;
                blocked-one) rg -q "CALL ActivateWindow \('firefox-id',\)" "$prefix-mock.log"; rg -q 'CALL ClearHistory' "$prefix-mock.log";;
                unidentified)
                    if rg -q 'CALL ActivateWindow' "$prefix-mock.log"; then
                        echo "Unidentified row activated a window" >&2; exit 1
                    fi
                    rg -q "NOTIFICATION .*'desktop-idle-status'" "$prefix-mock.log";;
                late|late-seconds) rg -q 'KCM kcm_screenlocker' "$prefix-actions.log"; rg -q 'KCM kcm_powerdevilprofilesconfig' "$prefix-actions.log";;
                late-power-lock) rg -q 'KCM kcm_powerdevilprofilesconfig' "$prefix-actions.log";;
                screensaver-off) rg -q 'CALL StartScreensaver' "$prefix-mock.log";;
                service-down) rg -q 'SYSTEMCTL --user start desktop-idle-status.service' "$prefix-actions.log"; rg -q 'SERVICE STARTED' "$prefix-mock.log";;
            esac
        fi
        echo "Rendered $state ($theme)"
    done
done
run_child env DIS_SHOT_DIR="$shotdir" /usr/bin/python3 "$repo/tests/plasmoid/overview.py"
