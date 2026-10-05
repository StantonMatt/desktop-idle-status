#!/usr/bin/python3
"""Own all fixture descendants until reaped, including QML command grandchildren."""
import ctypes
import json
import os
import pathlib
import signal
import subprocess
import sys
import time


def identity(pid):
    try:
        return pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
    except FileNotFoundError:
        return None


def descendants(pid):
    """Only walk this supervisor's tree, never the user's process/session tree."""
    owned = {}
    children = set()
    # Qt can launch commands from worker threads; /proc children is per-thread.
    for task in pathlib.Path(f"/proc/{pid}/task").glob("*/children"):
        try:
            children.update(task.read_text().split())
        except FileNotFoundError:
            continue
    for child in map(int, children):
        start = identity(child)
        if start is not None:
            owned[child] = start
            owned.update(descendants(child))
    return owned


def cleanup(processes):
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, signal.SIG_IGN)
    deadline = time.monotonic() + 5
    while True:
        owned = descendants(os.getpid())
        for pid, start in reversed(list(owned.items())):
            if identity(pid) == start:
                try:
                    os.kill(pid, signal.SIGTERM if time.monotonic() < deadline else signal.SIGKILL)
                except ProcessLookupError:
                    pass
        for process in processes:
            process.poll()
        # Subreaper adoption includes separate process groups created by timeout
        # inside QML's executable data source. Reap those orphan grandchildren.
        while True:
            try:
                pid, _ = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                return
            if pid == 0:
                break
        time.sleep(0.025)


def main():
    mode, repo_arg, root_arg, *args = sys.argv[1:]
    repo, root = pathlib.Path(repo_arg), pathlib.Path(root_arg)
    if (os.environ.get("DIS_SESSION_ROOT") != str(root)
            or os.environ.get("XDG_RUNTIME_DIR") != str(root / "runtime")
            or os.environ.get("XDG_CONFIG_HOME") != str(root / "config")
            or os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")
            or not os.environ.get("DBUS_SESSION_BUS_ADDRESS", "").startswith(f"unix:path={root}/bus")):
        raise RuntimeError("use run.sh or unit.sh on its isolated private bus")
    # Linux-only test harness: orphaned grandchildren stay owned and reapable.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "cannot become fixture subreaper")
    processes, logs = [], []
    spawn_state = {"busy": False, "cancel": None}

    def terminate(sig, _frame):
        if spawn_state["busy"]:
            spawn_state["cancel"] = sig
        else:
            raise SystemExit(128 + sig)

    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, terminate)

    def spawn(command, log=None, **kwargs):
        if log is not None:
            output = open(log, "w")
            logs.append(output)
            kwargs.update(stdout=output, stderr=subprocess.STDOUT)
        spawn_state["busy"] = True
        try:
            process = subprocess.Popen(command, start_new_session=True, **kwargs)
            processes.append(process)
            return process
        finally:
            spawn_state["busy"] = False
            if spawn_state["cancel"] is not None:
                raise SystemExit(128 + spawn_state["cancel"])

    try:
        (root / "harness.pid").write_text(str(os.getpid()))
        state = args[0] if mode == "render" else "ready"
        prefix = pathlib.Path(args[1]) if mode == "render" else root / "unit"
        mock_log = pathlib.Path(f"{prefix}-mock.log")
        mock = spawn(["/usr/bin/python3", str(repo / "tests/plasmoid/mock.py"), state], mock_log)
        deadline = time.monotonic() + 2
        while not mock_log.stat().st_size and time.monotonic() < deadline:
            if mock.poll() is not None:
                raise RuntimeError("private mock exited before becoming ready")
            time.sleep(0.05)
        probe = os.environ.get("DIS_CANCELLATION_PROBE") == "1"
        if mode == "unit":
            viewers = [spawn(["/usr/lib/qt6/bin/qmltestrunner", "-input", str(repo / "tests/plasmoid")])]
        elif mode == "render":
            viewers = []
            count = 2 if os.environ.get("DIS_INSTANCES") == "2" else 1
            for index in range(count):
                env = dict(os.environ)
                suffix = "-second" if index == 0 and count == 2 else ""
                if suffix:
                    env.update(DIS_SHOT_PREFIX=f"{prefix}{suffix}", DIS_EMIT_RETURN="0")
                if not probe:
                    env["LD_PRELOAD"] = str(root / "capture.so")
                viewers.append(spawn(["plasmoidviewer", "-a", str(repo / "plasmoid"), "-s", "432x432",
                                      "-f", "horizontal", "-l", "bottomedge"],
                                     f"{prefix}{suffix}-viewer.log", env=env))
        else:
            raise ValueError(f"unknown fixture mode: {mode}")
        if probe:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                assert mock.poll() is None and all(p.poll() is None for p in viewers), "probe child exited early"
                commands = root / "command-pids"
                if mode != "unit" or commands.exists() and commands.stat().st_size:
                    break
                time.sleep(0.025)
            else:
                raise AssertionError("unit probe did not start a pending command")
            owned = descendants(os.getpid())
            owned[os.getpid()] = identity(os.getpid())
            # Include the private bus daemon, a sibling under dbus-run-session.
            owned.update(descendants(os.getppid()))
            marker = {"wrapper": int(os.environ["DIS_RUN_WRAPPER_PID"]), "pids": owned,
                      "mock": mock.pid, "viewers": [p.pid for p in viewers]}
            (root / "cancellation-ready.json").write_text(json.dumps(marker))
            while True:
                assert all(p.poll() is None for p in processes), "probe child exited early"
                time.sleep(0.05)
        deadline = time.monotonic() + (60 if mode == "unit" else 15)
        for viewer in viewers:
            status = viewer.wait(timeout=max(0, deadline - time.monotonic()))
            if status:
                raise SystemExit(status)
    finally:
        cleanup(processes)
        for log in logs:
            log.close()


if __name__ == "__main__":
    main()
