# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Measure fresh-process server-only starts in a NEW disposable data folder.

Runs the same preparation/supervisor path as the launcher. Never starts the
game, selects an existing save, or copies account data. Reports and prepared
game content stay local; review outputs before sharing.
"""
from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
from datetime import datetime, timezone


def utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def read(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8-sig"))


class WindowsMemory:
    """Read the OS peak working set for the receipt's server PID only."""
    class Counters(ctypes.Structure):
        _fields_ = [("cb", ctypes.c_ulong), ("faults", ctypes.c_ulong)] + [
            (name, ctypes.c_size_t) for name in (
                "peak_working_set", "working_set", "peak_paged_pool", "paged_pool",
                "peak_nonpaged_pool", "nonpaged_pool", "pagefile", "peak_pagefile",
            )
        ]

    def __init__(self) -> None:
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.memory = ctypes.WinDLL("psapi", use_last_error=True)
        self.kernel.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
        self.kernel.OpenProcess.restype = ctypes.c_void_p
        self.kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        self.kernel.CloseHandle.restype = ctypes.c_int
        self.memory.GetProcessMemoryInfo.argtypes = [
            ctypes.c_void_p, ctypes.POINTER(self.Counters), ctypes.c_ulong,
        ]
        self.memory.GetProcessMemoryInfo.restype = ctypes.c_int

    def peak(self, pid: int) -> int | None:
        handle = self.kernel.OpenProcess(0x0400 | 0x0010, False, pid)
        if not handle:
            return None
        try:
            counters = self.Counters()
            counters.cb = ctypes.sizeof(counters)
            if not self.memory.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                return None
            return counters.peak_working_set
        finally:
            self.kernel.CloseHandle(handle)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True, help="New disposable directory")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=600)
    args = parser.parse_args()
    if not 1 <= args.runs <= 10 or not 10 <= args.timeout <= 900:
        parser.error("runs must be 1..10 and timeout 10..900 seconds")
    exe, game, out = (p.resolve() for p in (args.executable, args.game_dir, args.out))
    if not exe.is_file() or not (game / "NFS16.exe").is_file():
        parser.error("executable or supported installation is missing")
    if out == game or game in out.parents or out in game.parents:
        parser.error("the benchmark directory must be separate from the game")
    out.mkdir(parents=True, exist_ok=False)
    data = out / "data"
    env = os.environ.copy()
    temp = out / "tmp"
    temp.mkdir()
    env.update(TEMP=str(temp), TMP=str(temp))
    flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    common = [str(exe), "--data-dir", str(data)]
    report = {
        "started_utc": utc(), "executable_sha256": digest(exe),
        "game_sha256": digest(game / "NFS16.exe"), "game_launched": False, "runs": [],
    }
    # No developer overrides influence the selected executable or server.
    for name in ("AFTERHOURS_EXE", "AFTERHOURS_API_TOKEN"):
        env.pop(name, None)
    version = subprocess.run([str(exe), "version", "--json"], capture_output=True,
                             check=True, timeout=30, env=env, creationflags=flags)
    report["version"] = json.loads(version.stdout)
    baseline_save = None
    memory = WindowsMemory() if os.name == "nt" else None
    for number in range(args.runs):
        row = {"index": number, "started_utc": utc(), "ready": False,
               "server_peak_working_set_bytes": None}
        report["runs"].append(row)
        previous = read(data / "current-session.json")["id"] if (data / "current-session.json").exists() else None
        started = time.monotonic()
        proc = None
        try:
            with (out / f"run-{number}.stdout.log").open("wb") as stdout, (out / f"run-{number}.stderr.log").open("wb") as stderr:
                proc = subprocess.Popen(common + ["serve", "--server-only", "--events", "--game-dir", str(game)],
                                        stdout=stdout, stderr=stderr, env=env, creationflags=flags)
                while time.monotonic() - started < args.timeout:
                    pointer = data / "current-session.json"
                    if pointer.exists():
                        session = read(pointer)["id"]
                        if len(session) != 32 or any(c not in "0123456789abcdef" for c in session):
                            raise RuntimeError("invalid session identifier")
                        if session != previous:
                            run = data / "artifacts" / "launcher" / session
                            status_path = run / "status.json"
                            if status_path.exists():
                                status = read(status_path)
                                if status.get("game_running") or status.get("game_pid"):
                                    raise RuntimeError("unexpected game in server-only benchmark")
                                if memory and isinstance(status.get("server_pid"), int):
                                    peak = memory.peak(status["server_pid"])
                                    if peak is not None:
                                        row["server_peak_working_set_bytes"] = max(
                                            row["server_peak_working_set_bytes"] or 0, peak)
                                if status["state"] == "ready":
                                    row.update(ready=True, seconds=time.monotonic() - started, session=session)
                                    receipt = read(run / "server-process.json")
                                    row["server_seconds"] = (datetime.fromisoformat(receipt["ready_utc"].replace("Z", "+00:00")) - datetime.fromisoformat(receipt["spawned_utc"].replace("Z", "+00:00"))).total_seconds()
                                    break
                                if status["state"] in ("finished", "failed", "interrupted"):
                                    raise RuntimeError(f"preparation ended with {status['state']}: {status.get('message', '')}")
                    if proc.poll() is not None:
                        raise RuntimeError(f"supervisor exited early: {proc.returncode}")
                    time.sleep(0.2)
                if not row["ready"]:
                    raise TimeoutError("server readiness deadline")
        except Exception as error:
            row["error"] = str(error)
            raise
        finally:
            if proc is not None:
                try:
                    stop = subprocess.run(common + ["stop", "--json"], capture_output=True,
                                          timeout=30, env=env, creationflags=flags)
                    row["stop_exit"] = stop.returncode
                    (out / f"run-{number}.stop.log").write_bytes(stop.stdout + stop.stderr)
                    row["exit_code"] = proc.wait(timeout=30)
                except (subprocess.TimeoutExpired, OSError) as error:
                    row["cleanup_error"] = str(error)
                    proc.kill()  # Only the supervisor this invocation created.
                    proc.wait(timeout=30)
                    row["forced"] = True
            row["finished_utc"] = utc()
            (out / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        if row.get("stop_exit") != 0 or row.get("exit_code") != 0 or row.get("forced"):
            raise RuntimeError("server did not stop normally")
        saves = list((data / "artifacts" / "account").glob("*.sqlite"))
        if len(saves) != 1:
            raise RuntimeError("expected one disposable account")
        save = digest(saves[0])
        if baseline_save is not None and baseline_save != save:
            raise RuntimeError("server-only repeat changed the disposable save")
        baseline_save = save
        row["save_sha256"] = save
        report["finished_utc"] = utc()
        (out / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"run": number, "seconds": row["seconds"], "server_seconds": row["server_seconds"], "clean_stop": True}), flush=True)


if __name__ == "__main__":
    main()
