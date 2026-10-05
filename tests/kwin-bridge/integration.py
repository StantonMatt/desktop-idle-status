#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-or-later
"""Private-bus integration test. Never invoke directly on a desktop session."""
import os
import json
import pathlib
import select
import signal
import subprocess
import sys
import time
import uuid
import xml.etree.ElementTree as ET

import dbus
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

SERVICE = "org.kde.KWin"
IFACE = "io.github.StantonMatt.DesktopIdleStatus.KWinBridge1"
PLUGIN = "desktopidlestatusbridge"


def main():
    spawn_state = {"busy": False, "cancel": None}

    def terminate(signum, _frame):
        if spawn_state["busy"]:
            spawn_state["cancel"] = signum
        else:
            raise SystemExit(128 + signum)

    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, terminate)
    root = pathlib.Path(os.environ.get("DIS_NESTED_TEST_ROOT", "")).resolve()
    runtime = pathlib.Path(os.environ.get("XDG_RUNTIME_DIR", "")).resolve()
    # Fail closed if accidentally invoked in a real session. run.sh provides
    # empty isolated directories and a new dbus-run-session address.
    if (not root.name.startswith("nested-") or runtime != root / "runtime"
            or os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")
            or pathlib.Path(os.environ.get("XDG_CONFIG_HOME", "")).resolve() != root / "config"):
        raise RuntimeError("use run.sh; isolated nested-session environment required")
    (root / "harness.pid").write_text(str(os.getpid()))
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    if bus.name_has_owner(SERVICE):
        raise RuntimeError("refusing to use a bus with an existing KWin")
    context = GLib.MainContext.default()
    processes = []
    clients = []
    changes = []
    output_buffers = {}

    def pump():
        while context.pending():
            context.iteration(False)

    def wait_for(description, predicate, timeout=8):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            pump()
            if kwin.poll() is not None:
                raise RuntimeError(f"KWin exited ({kwin.returncode}) while waiting for {description}")
            try:
                last = predicate()
                if last:
                    return last
            except dbus.DBusException as error:
                last = error
            time.sleep(0.025)
        raise AssertionError(f"timed out: {description}; last={last}")

    def spawn(args, **kwargs):
        # Do not leave an untracked child if cancellation lands during Popen.
        spawn_state["busy"] = True
        try:
            process = subprocess.Popen(args, start_new_session=True, **kwargs)
            processes.append(process)
            return process
        finally:
            spawn_state["busy"] = False
            if spawn_state["cancel"] is not None:
                raise SystemExit(128 + spawn_state["cancel"])

    def spawn_client(caption):
        env = dict(os.environ, WAYLAND_DISPLAY="wayland-dis-test")
        client = spawn([sys.argv[1], caption], env=env, stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                  text=True)
        clients.append(client)
        return client

    def command(client, text):
        client.stdin.write(text + "\n")
        client.stdin.flush()

    def client_line(client, timeout=0.2):
        deadline = time.monotonic() + timeout
        pending = output_buffers.get(client.pid, b"")
        while b"\n" not in pending:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([client.stdout], [], [], remaining)[0]:
                output_buffers[client.pid] = pending
                return None
            chunk = os.read(client.stdout.fileno(), 4096)
            if not chunk:
                raise AssertionError(f"client {client.pid} disconnected while reading reply")
            pending += chunk
        line, pending = pending.split(b"\n", 1)
        output_buffers[client.pid] = pending
        return line.decode()

    log = root / "kwin.log"
    try:
        # --platform offscreen avoids any Qt windows on the user's desktop;
        # --virtual keeps the compositor independent of DRM and real outputs.
        with log.open("w") as output:
            kwin = spawn([
                "kwin_wayland", "--platform", "offscreen", "--virtual",
                "--width", "1280", "--height", "800", "--socket", "wayland-dis-test",
                "--no-lockscreen", "--no-global-shortcuts", "--no-kactivities",
            ], stdout=output, stderr=subprocess.STDOUT)
        wait_for("nested KWin bus owner", lambda: bus.name_has_owner(SERVICE))
        # Check the bus owner is exactly our subprocess before any plugin call.
        daemon = dbus.Interface(bus.get_object("org.freedesktop.DBus", "/org/freedesktop/DBus"),
                                "org.freedesktop.DBus")
        if int(daemon.GetConnectionUnixProcessID(SERVICE)) != kwin.pid:
            raise RuntimeError("refusing plugin calls: KWin bus owner is not this test's compositor")
        plugins_object = bus.get_object(SERVICE, "/Plugins")
        plugins = dbus.Interface(plugins_object, "org.kde.KWin.Plugins")
        properties = dbus.Interface(plugins_object, "org.freedesktop.DBus.Properties")
        wait_for("plugin discovery", lambda: PLUGIN in properties.Get("org.kde.KWin.Plugins", "AvailablePlugins"))
        assert PLUGIN not in properties.Get("org.kde.KWin.Plugins", "LoadedPlugins")
        assert plugins.LoadPlugin("desktopidlestatustestoracle")
        oracle = dbus.Interface(bus.get_object(SERVICE, "/IdleTestOracle"),
                                "io.github.StantonMatt.DesktopIdleStatus.TestOracle")
        assert plugins.LoadPlugin(PLUGIN)
        assert PLUGIN in properties.Get("org.kde.KWin.Plugins", "LoadedPlugins")
        print("PASS plugin loaded: /Plugins LoadedPlugins contains desktopidlestatusbridge", flush=True)

        obj = bus.get_object(SERVICE, "/DesktopIdleStatus")
        bridge = dbus.Interface(obj, IFACE)
        props = dbus.Interface(obj, "org.freedesktop.DBus.Properties")
        assert int(props.Get(IFACE, "InterfaceVersion")) == 1
        assert str(props.Get(IFACE, "BuiltForKWin")) == "6.6.6"
        package = str(props.Get(IFACE, "BuiltAgainstPackage"))
        installed = subprocess.check_output(["dpkg-query", "-W", "-f=${Version}", "libkwin6"], text=True)
        assert package == installed
        xml = ET.fromstring(dbus.Interface(obj, "org.freedesktop.DBus.Introspectable").Introspect())
        interface = xml.find(f"./interface[@name='{IFACE}']")
        assert interface is not None
        assert {item.get("name") for item in xml.findall("interface")} == {
            IFACE, "org.freedesktop.DBus.Properties", "org.freedesktop.DBus.Introspectable",
            "org.freedesktop.DBus.Peer"}
        assert {method.get("name") for method in interface.findall("method")} == {"Snapshot", "ActivateWindow"}
        assert {prop.get("name") for prop in interface.findall("property")} == {
            "InterfaceVersion", "BuiltForKWin", "BuiltAgainstPackage"}
        assert all(prop.get("access") == "read" for prop in interface.findall("property"))
        assert {prop.get("name"): prop.get("type") for prop in interface.findall("property")} == {
            "InterfaceVersion": "u", "BuiltForKWin": "s", "BuiltAgainstPackage": "s"}
        assert {item.get("name") for item in interface.findall("signal")} == {"Changed"}
        assert [arg.get("type") for arg in interface.findall("./signal[@name='Changed']/arg")] == ["t"]
        assert [arg.get("type") for arg in interface.findall("./method[@name='ActivateWindow']/arg[@direction='in']")] == ["s"]
        assert [arg.get("type") for arg in interface.findall("./method[@name='ActivateWindow']/arg[@direction='out']")] == ["b"]
        outputs = interface.findall("./method[@name='Snapshot']/arg[@direction='out']")
        assert [arg.get("type") for arg in outputs] == ["t", "aa{sv}"]
        print(f"PASS contract: Snapshot -> (t, aa{{sv}}); version=1; KWin=6.6.6; package={package}", flush=True)
        bus.add_signal_receiver(lambda revision: changes.append(int(revision)), signal_name="Changed",
                                dbus_interface=IFACE, path="/DesktopIdleStatus", bus_name=SERVICE)

        def snapshot():
            revision, rows = bridge.Snapshot()
            return int(revision), [dict(row) for row in rows]

        assert snapshot() == (0, [])
        print("PASS initial Snapshot: revision=0 windows=[]", flush=True)
        assert not bridge.ActivateWindow("not-a-uuid")
        assert not bridge.ActivateWindow("00000000-0000-0000-0000-000000000001")

        first = spawn_client("Desktop Idle Status Test One")
        wait_for("Changed before Snapshot on appearance", lambda: changes)

        def single_effective():
            revision, rows = snapshot()
            return (revision, rows) if len(rows) == 1 and rows[0]["effective"] else None

        revision, rows = wait_for("first effective inhibitor", single_effective)
        row = rows[0]
        internal_id = str(row["internalId"])
        uuid.UUID(internal_id)
        assert set(row) == {"internalId", "pid", "executablePath", "appId", "desktopFileName",
                            "resourceClass", "caption", "effective", "notEffectiveReason"}
        assert isinstance(row["effective"], dbus.Boolean)
        assert all(isinstance(row[key], dbus.String) for key in row if key not in {"pid", "effective"})
        assert isinstance(row["pid"], dbus.UInt32) and int(row["pid"]) == first.pid
        assert str(row["caption"]) == "Desktop Idle Status Test One"
        assert str(row["executablePath"]) == str(pathlib.Path(sys.argv[1]).resolve())
        assert str(row["resourceClass"]) == "io.github.StantonMatt.DesktopIdleStatus.Test"
        # An ordinary unsandboxed client has no security-context app ID.
        # xdg_toplevel.app_id is represented separately by resourceClass.
        assert str(row["appId"]) == ""
        assert str(row["notEffectiveReason"]) == ""
        wait_for("Changed on appearance", lambda: revision in changes)
        print("PASS inhibitor effective: pid matches client; caption, executablePath, resourceClass correct; appId='' (no security context)", flush=True)
        print(f"PASS Changed: revision={revision}", flush=True)

        if len(sys.argv) > 2 and sys.argv[2] == "--cancellation-probe":
            (root / "cancellation-ready.json").write_text(json.dumps({
                "wrapper": int(os.environ["DIS_RUN_WRAPPER_PID"]),
                "pids": [os.getpid(), kwin.pid, first.pid,
                         int(daemon.GetConnectionUnixProcessID("org.freedesktop.DBus"))],
            }))
            while True:
                pump()
                time.sleep(0.025)

        def stable(description, expected=None, count=None):
            if expected is None:
                expected = snapshot()
            pump()
            if count is None:
                count = len(changes)
            deadline = time.monotonic() + 1.2
            while time.monotonic() < deadline:
                pump()
                assert snapshot() == expected, description
                time.sleep(0.025)
            assert len(changes) == count, description

        def ack(client):
            command(client, "p")
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if client_line(client) == "ack":
                    return
            raise AssertionError("client roundtrip acknowledgement timed out")

        def unchanged_command(client, text, description):
            expected = snapshot()
            pump()
            count = len(changes)
            command(client, text)
            ack(client)
            stable(description, expected, count)

        stable("unchanged safety poll")
        print("PASS unchanged snapshot: no revision or Changed during safety poll", flush=True)

        # These must signal without Snapshot driving refresh, preserve the UUID,
        # and cover both caption and xdg app ID (resourceClass, not security appId).
        for cmd, field, value in (("t", "caption", "Updated Bridge Title"),
                                  ("d", "resourceClass", "io.github.StantonMatt.Updated")):
            before = revision
            command(first, f"{cmd} {value}")
            wait_for(f"Changed before Snapshot on {field}", lambda: any(v > before for v in changes))
            revision, rows = snapshot()
            assert revision > before and len(rows) == 1
            assert str(rows[0][field]) == value
            assert str(rows[0]["internalId"]) == internal_id and rows[0]["effective"]
            assert rows[0]["appId"] == ""
            unchanged_command(first, f"{cmd} {value}", f"identical repeated {field}")
        print("PASS field-only updates: Changed, new revision, updated title/app-id, same UUID; repeated values stable", flush=True)

        command(first, "m")
        wait_for("Changed before Snapshot on minimize", lambda: any(value > revision for value in changes))

        def minimized():
            revision, rows = snapshot()
            return (revision, rows) if len(rows) == 1 and not rows[0]["effective"] and rows[0]["notEffectiveReason"] == "minimized" else None

        revision, rows = wait_for("latent minimized inhibitor", minimized)
        wait_for("Changed on minimize", lambda: revision in changes)
        print(f"PASS minimized: effective=false reason=minimized revision={revision}", flush=True)
        assert bridge.ActivateWindow(internal_id)
        wait_for("Changed before Snapshot on activation", lambda: any(value > revision for value in changes))
        revision, rows = wait_for("activated effective inhibitor", single_effective)
        wait_for("Changed on activation", lambda: revision in changes)
        info = dbus.Interface(bus.get_object(SERVICE, "/KWin"), "org.kde.KWin").getWindowInfo(internal_id)
        assert not info["minimized"]

        def client_activated():
            command(first, "a")
            return client_line(first) == "activated=1"

        wait_for("xdg toplevel activated state", client_activated)
        print(f"PASS ActivateWindow: unminimized, effective and xdg activated revision={revision}", flush=True)

        desktop_interface = "org.kde.KWin.VirtualDesktopManager"
        desktop_object = bus.get_object(SERVICE, "/VirtualDesktopManager")
        desktops = dbus.Interface(desktop_object, desktop_interface)
        desktop_props = dbus.Interface(desktop_object, "org.freedesktop.DBus.Properties")
        original_desktop = desktop_props.Get(desktop_interface, "current")
        desktops.createDesktop(dbus.UInt32(1), "Bridge Test Second Desktop")
        desktop_data = desktop_props.Get(desktop_interface, "desktops")
        other_desktop = next(item[1] for item in desktop_data if item[1] != original_desktop)
        desktop_props.Set(desktop_interface, "current", other_desktop)
        wait_for("Changed before Snapshot on desktop switch", lambda: any(value > revision for value in changes))

        def off_desktop():
            revision, rows = snapshot()
            return (revision, rows) if len(rows) == 1 and not rows[0]["effective"] and rows[0]["notEffectiveReason"] == "other-desktop" else None

        revision, rows = wait_for("latent off-desktop inhibitor", off_desktop)
        print(f"PASS other desktop: effective=false reason=other-desktop revision={revision}", flush=True)
        assert bridge.ActivateWindow(internal_id)
        wait_for("Changed before Snapshot on desktop activation", lambda: any(value > revision for value in changes))
        revision, rows = wait_for("activation switches desktop", single_effective)
        assert desktop_props.Get(desktop_interface, "current") == original_desktop
        wait_for("xdg activation after switching desktop", client_activated)
        print(f"PASS ActivateWindow: restored original desktop and xdg activated revision={revision}", flush=True)

        def assert_oracle(description):
            def matches():
                rev, current = snapshot()
                effective = set(map(str, oracle.EffectiveWindows()))
                represented = {str(item["internalId"]) for item in current if item["effective"]}
                if represented != effective or len(current) != 1:
                    return None
                item = current[0]
                if item["effective"]:
                    assert item["notEffectiveReason"] == ""
                else:
                    assert item["notEffectiveReason"] == "hidden"
                return rev, current
            return wait_for(description, matches)

        assert oracle.SetHidden(internal_id, True)
        revision, rows = wait_for("hidden latent inhibitor", lambda: (
            snapshot() if internal_id not in set(map(str, oracle.EffectiveWindows())) else None))
        revision, rows = assert_oracle("hidden agrees with KWin effective list")
        assert not rows[0]["effective"] and rows[0]["notEffectiveReason"] == "hidden"
        assert oracle.SetHidden(internal_id, False)
        wait_for("unhidden is effective", lambda: internal_id in set(map(str, oracle.EffectiveWindows())))
        revision, rows = assert_oracle("unhidden agrees with KWin effective list")
        for showing in (True, False):
            oracle.SetShowingDesktop(showing)
            assert bool(oracle.ShowingDesktop()) == showing
            # KWin's real policy determines effectiveness, including versions
            # where showing desktop does not hide an ordinary toplevel.
            revision, rows = assert_oracle(f"show-desktop={showing} agrees with effective list")
            stable(f"show-desktop={showing} stable policy")
        print("PASS hidden/unhidden and show-desktop on/off: rows match KWin input()->idleInhibitors()", flush=True)

        unchanged_command(first, "j", "adding a second inhibitor to the same window")
        unchanged_command(first, "r", "releasing one of two inhibitors")
        assert snapshot()[1][0]["internalId"] == internal_id
        command(first, "k")
        wait_for("last inhibitor release removes row", lambda: snapshot()[1] == [])
        command(first, "i")
        revision, rows = wait_for("parent inhibitor restored", single_effective)
        print("PASS multiple objects on one window: one row; add/release-one leave revision and Changed unchanged", flush=True)

        unchanged_command(first, "s", "parent-to-subsurface transfer")
        revision, rows = wait_for("subsurface-only effective inhibitor", single_effective)
        assert str(rows[0]["internalId"]) == internal_id
        command(first, "m")
        revision, rows = wait_for("subsurface-only latent minimized inhibitor", minimized)
        assert str(rows[0]["internalId"]) == internal_id
        assert bridge.ActivateWindow(internal_id)
        revision, rows = wait_for("subsurface-only activation", single_effective)
        command(first, "u")
        wait_for("subsurface release removes row", lambda: snapshot()[1] == [])
        command(first, "i")
        revision, rows = wait_for("parent inhibitor restored after subsurface", single_effective)
        print("PASS subsurface-only inhibition: effective, latent minimized, activation, release", flush=True)

        command(first, "r")
        wait_for("Changed before Snapshot on release", lambda: any(value > revision for value in changes))
        revision, rows = snapshot()
        assert rows == []
        command(first, "i")
        wait_for("Changed before Snapshot on reinhibit", lambda: any(value > revision for value in changes))
        revision, rows = wait_for("inhibitor recreated on same window", single_effective)
        assert str(rows[0]["internalId"]) == internal_id
        print(f"PASS inhibitor release/recreate: same window id revision={revision}", flush=True)

        second = spawn_client("Desktop Idle Status Test Two")
        wait_for("Changed before Snapshot on second appearance", lambda: any(value > revision for value in changes))

        def two_effective():
            revision, rows = snapshot()
            return (revision, rows) if len(rows) == 2 and all(row["effective"] for row in rows) else None

        revision, rows = wait_for("two simultaneous inhibitors", two_effective)
        assert {int(row["pid"]) for row in rows} == {first.pid, second.pid}
        assert len({str(row["internalId"]) for row in rows}) == 2
        wait_for("Changed on second appearance", lambda: revision in changes)
        print(f"PASS two simultaneous inhibitors: distinct ids and correct pids revision={revision}", flush=True)

        command(first, "q")
        first.wait(timeout=5)
        assert first.returncode == 0
        wait_for("Changed before Snapshot on first destruction", lambda: any(value > revision for value in changes))
        revision, rows = wait_for("first inhibitor removed", single_effective)
        assert int(rows[0]["pid"]) == second.pid
        assert not bridge.ActivateWindow(internal_id)
        survivor_id = str(rows[0]["internalId"])
        plugins.UnloadPlugin(PLUGIN)
        assert PLUGIN not in properties.Get("org.kde.KWin.Plugins", "LoadedPlugins")
        try:
            disappeared = ET.fromstring(dbus.Interface(obj, "org.freedesktop.DBus.Introspectable").Introspect())
        except dbus.DBusException as error:
            assert error.get_dbus_name() == "org.freedesktop.DBus.Error.UnknownObject"
        else:
            assert disappeared.find(f"./interface[@name='{IFACE}']") is None
        assert kwin.poll() is None
        command(second, "m")
        command(second, "t Changed While Unloaded")
        command(second, "d io.github.StantonMatt.Unloaded")
        ack(second)
        wait_for("window minimized while bridge unloaded", lambda: dbus.Interface(
            bus.get_object(SERVICE, "/KWin"), "org.kde.KWin").getWindowInfo(survivor_id)["minimized"])
        assert plugins.LoadPlugin(PLUGIN)
        revision, rows = snapshot()
        assert revision == 1 and len(rows) == 1
        assert str(rows[0]["internalId"]) == survivor_id
        assert rows[0]["caption"] == "Changed While Unloaded"
        assert rows[0]["resourceClass"] == "io.github.StantonMatt.Unloaded"
        assert not rows[0]["effective"] and rows[0]["notEffectiveReason"] == "minimized"
        assert bridge.ActivateWindow(survivor_id)
        revision, rows = wait_for("reloaded live inhibitor activated", single_effective)
        assert str(rows[0]["internalId"]) == survivor_id
        print("PASS live unload/reload: object disappears, unloaded mutations collected in initial revision=1 snapshot, compositor survives", flush=True)

        # Reload a second time while the existing window is effective.
        plugins.UnloadPlugin(PLUGIN)
        assert plugins.LoadPlugin(PLUGIN)
        revision, rows = snapshot()
        assert revision == 1 and len(rows) == 1 and rows[0]["effective"]
        pump()
        changes.clear()
        second.kill()
        second.wait(timeout=5)
        assert second.returncode == -signal.SIGKILL
        wait_for("Changed before Snapshot on abrupt disconnect", lambda: any(v > revision for v in changes))
        revision, rows = snapshot()
        assert rows == [] and kwin.poll() is None
        assert not bridge.ActivateWindow(survivor_id)
        print(f"PASS abrupt disconnect with active inhibition: windows=[] revision={revision}; compositor survives", flush=True)

        plugins.UnloadPlugin(PLUGIN)
        assert PLUGIN not in properties.Get("org.kde.KWin.Plugins", "LoadedPlugins")
        assert plugins.LoadPlugin(PLUGIN)
        assert snapshot() == (0, [])
        print("PASS unload/reload: object re-registered with empty revision=0 snapshot", flush=True)
        print("ALL TESTS PASSED (private D-Bus, virtual offscreen KWin)", flush=True)
    except BaseException:
        if log.exists():
            print("--- nested KWin log ---", file=sys.stderr)
            print(log.read_text(), file=sys.stderr)
        raise
    finally:
        # Ignore repeated cancellation while finishing cleanup. Signal all
        # owned groups first, then use one bounded grace period for the set.
        for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            signal.signal(sig, signal.SIG_IGN)
        for process in reversed(processes):
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        deadline = time.monotonic() + 5
        for process in reversed(processes):
            try:
                process.wait(timeout=max(0.01, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
            for stream in (process.stdin, process.stdout, process.stderr):
                if stream is not None:
                    stream.close()
        bus.close()


if __name__ == "__main__":
    main()
