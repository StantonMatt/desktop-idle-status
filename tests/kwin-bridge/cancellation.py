#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Cancel concurrent run.sh invocations with live private inhibiting clients."""
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time


def identity(pid):
    try:
        # PID plus start time avoids mistaking an unrelated reused PID for ours.
        stat = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return stat[19]
    except FileNotFoundError:
        return None


def main():
    scratch = pathlib.Path(sys.argv[2])
    root = pathlib.Path(tempfile.mkdtemp(prefix="cancellation-", dir=scratch))
    cases = []
    owned = {}

    spawn_state = {"busy": False, "cancel": None}

    def terminate(sig, _frame):
        if spawn_state["busy"]:
            spawn_state["cancel"] = sig
        else:
            raise SystemExit(128 + sig)

    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, terminate)
    try:
        for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            log_path = root / f"{sig.name}.log"
            with log_path.open("w") as log:
                # Defer cancellation through spawn/registration, without
                # inheriting blocked signals into the child.
                spawn_state["busy"] = True
                try:
                    wrapper = subprocess.Popen([sys.argv[1], "--cancellation-probe"],
                                               stdout=log, stderr=subprocess.STDOUT)
                    cases.append({"wrapper": wrapper, "signal": sig, "log": log_path})
                finally:
                    spawn_state["busy"] = False
                    if spawn_state["cancel"] is not None:
                        raise SystemExit(128 + spawn_state["cancel"])
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            for case in cases:
                if case["wrapper"].poll() is not None:
                    raise AssertionError(f"probe exited early: {case['wrapper'].returncode}")
            for marker in scratch.glob("nested-*/cancellation-ready.json"):
                try:
                    data = json.loads(marker.read_text())
                except (FileNotFoundError, json.JSONDecodeError):
                    continue
                for case in cases:
                    if data["wrapper"] == case["wrapper"].pid and "session" not in case:
                        case["session"] = marker.parent
                        case["owned"] = {pid: identity(pid) for pid in data["pids"]}
                        assert all(case["owned"].values()), "probe process already missing"
                        # Include the private dbus-run-session supervisor.
                        children_file = pathlib.Path(f"/proc/{case['wrapper'].pid}/task/{case['wrapper'].pid}/children")
                        for pid in map(int, children_file.read_text().split()):
                            case["owned"][pid] = identity(pid)
                        owned.update(case["owned"])
            if all("session" in case for case in cases):
                break
            time.sleep(0.05)
        assert all("session" in case for case in cases), "probe did not reach a live inhibiting client"
        assert len({case["session"] for case in cases}) == len(cases)
        for case in cases:
            for build in ("bridge-build", "client-build"):
                assert (case["session"] / build / "CMakeCache.txt").is_file()
        print("PASS concurrent invocation isolation: three distinct runtime, bridge-build and client-build directories", flush=True)
        for case in cases:
            wrapper, sig = case["wrapper"], case["signal"]
            wrapper.send_signal(sig)
            assert wrapper.wait(timeout=15) == 128 + int(sig)
            assert not case["session"].exists(), "private runtime/build directories survived cancellation"
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and any(identity(pid) == start for pid, start in case["owned"].items()):
                time.sleep(0.05)
            remaining = [pid for pid, start in case["owned"].items() if identity(pid) == start]
            assert not remaining, f"leftover harness/compositor/client/bus PIDs: {remaining}"
            for other in cases:
                if other["wrapper"].poll() is None:
                    assert other["session"].exists()
                    assert all(identity(pid) == start for pid, start in other["owned"].items())
            print(f"PASS wrapper {sig.name} cancellation: Python, KWin, inhibiting client and private bus exited; directories removed; no leftover processes; concurrent runs unaffected", flush=True)
    except BaseException:
        for case in cases:
            print(case["log"].read_text(), file=sys.stderr)
        raise
    finally:
        for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            signal.signal(sig, signal.SIG_IGN)
        for case in cases:
            wrapper = case["wrapper"]
            if wrapper.poll() is None:
                wrapper.terminate()
                wrapper.wait(timeout=20)
        # Failure recovery is restricted to identities reported by our probes.
        for pid, start in owned.items():
            if identity(pid) == start:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        shutil.rmtree(root)


if __name__ == "__main__":
    main()
