#!/usr/bin/python3
"""Cancel both wrappers with live private-bus fixtures and assert full reaping."""
import json
import os
import pathlib
import shutil
import signal
import subprocess
import tempfile
import time

from session import descendants, identity


def main():
    repo = pathlib.Path(__file__).resolve().parents[2]
    scratch = pathlib.Path(os.environ.get("DIS_SCRATCH", pathlib.Path.home()
                                       / ".cache/agent-scratch/desktop-idle-status"))
    scratch.mkdir(parents=True, exist_ok=True)
    # Keep AF_UNIX socket paths below Linux's 108-byte sun_path limit.
    root = pathlib.Path(tempfile.mkdtemp(prefix="pc-", dir=scratch))
    cases, owned = [], {}
    spawn_state = {"busy": False, "cancel": None}

    def terminate(sig, _frame):
        if spawn_state["busy"]:
            spawn_state["cancel"] = sig
        else:
            raise SystemExit(128 + sig)

    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, terminate)
    try:
        # Three simultaneous invocations prove cancellation isolation as well.
        for name in ("run.sh", "unit.sh"):
            current = []
            for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
                directory = root / f"{name[0]}{int(sig)}"
                directory.mkdir()
                log_path = directory / "wrapper.log"
                env = dict(os.environ, DIS_SCRATCH=str(directory), DIS_SHOT_DIR=str(directory / "shots"),
                           DIS_CANCELLATION_PROBE="1", DIS_THEMES="light", DIS_STATES="ready", DIS_INSTANCES="2")
                with log_path.open("w") as log:
                    spawn_state["busy"] = True
                    try:
                        wrapper = subprocess.Popen([str(repo / "tests/plasmoid" / name)], env=env,
                                                   stdout=log, stderr=subprocess.STDOUT)
                        case = {"wrapper": wrapper, "signal": sig, "directory": directory, "log": log_path}
                        cases.append(case)
                        current.append(case)
                    finally:
                        spawn_state["busy"] = False
                        if spawn_state["cancel"] is not None:
                            raise SystemExit(128 + spawn_state["cancel"])
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                for case in current:
                    assert case["wrapper"].poll() is None, f"{name} exited before cancellation"
                    for marker in case["directory"].glob("*/cancellation-ready.json"):
                        try:
                            data = json.loads(marker.read_text())
                        except (FileNotFoundError, json.JSONDecodeError):
                            continue
                        if "session" in case or data["wrapper"] != case["wrapper"].pid:
                            continue
                        case["session"] = marker.parent
                        case["owned"] = {int(pid): start for pid, start in data["pids"].items()}
                        case["owned"].update(descendants(case["wrapper"].pid))
                        assert all(identity(pid) == start for pid, start in case["owned"].items())
                        assert data["mock"] in case["owned"]
                        assert len(data["viewers"]) == (2 if name == "run.sh" else 1)
                        if name == "unit.sh":
                            commands = (case["session"] / "command-pids").read_text().split()
                            assert commands and all(int(pid) in case["owned"] for pid in commands)
                        owned.update(case["owned"])
                if all("session" in case for case in current):
                    break
                time.sleep(0.025)
            assert all("session" in case for case in current), f"{name} fixture readiness timed out"
            assert len({case["session"] for case in current}) == 3
            for case in current:
                wrapper, sig = case["wrapper"], case["signal"]
                wrapper.send_signal(sig)
                assert wrapper.wait(timeout=15) == 128 + int(sig), f"wrong {name} cancellation status"
                assert not case["session"].exists(), "scratch directory survived cancellation"
                # Check immediately: wrapper exit must mean children were reaped,
                # not merely signalled and left to disappear asynchronously.
                remaining = [pid for pid, start in case["owned"].items() if identity(pid) == start]
                assert not remaining, f"leftover fixture/bus/viewer/command PIDs: {remaining}"
                for other in current:
                    if other["wrapper"].poll() is None:
                        assert other["session"].exists(), "another invocation's scratch was removed"
                        assert all(identity(pid) == start for pid, start in other["owned"].items())
                print(f"PASS {name} {sig.name}: fixture, viewers/runner, pending commands and private bus reaped; scratch removed; concurrent invocations unaffected", flush=True)
    except BaseException:
        for case in cases:
            print(case["log"].read_text())
        raise
    finally:
        for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            signal.signal(sig, signal.SIG_IGN)
        for case in cases:
            wrapper = case["wrapper"]
            if wrapper.poll() is None:
                # Include children even if failure happened before readiness.
                owned.update(descendants(wrapper.pid))
                wrapper.terminate()
                wrapper.wait(timeout=20)
        for pid, start in owned.items():
            if identity(pid) == start:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        shutil.rmtree(root)


if __name__ == "__main__":
    main()
