"""Verify the runtime LAP_CONSOLE opt-in on the deployed portable exe.

A: no env var  -> stdout pipe captures startup logs, process owns NO console.
B: LAP_CONSOLE=1 -> startup logs go to the allocated console (pipe stays
   mostly empty) and the process DOES own a console (AttachConsole proof).
"""
import ctypes
import os
import subprocess
import time

k32 = ctypes.windll.kernel32
EXE = r"D:\LapPortable\Lap.exe"
CWD = r"D:\LapPortable"


def owns_console(pid):
    """True if the given process has a console attached (AttachConsole probe)."""
    k32.FreeConsole()  # detach ourselves first; ignore errors
    ok = bool(k32.AttachConsole(pid))
    hwnd = k32.GetConsoleWindow() if ok else 0
    k32.FreeConsole()
    return ok, hwnd


def run_case(tag, env_extra):
    env = dict(os.environ)
    env.pop("LAP_CONSOLE", None)
    env.update(env_extra)
    p = subprocess.Popen(
        [EXE], cwd=CWD, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        stdin=subprocess.DEVNULL, text=True, errors="replace", env=env,
    )
    time.sleep(9)
    alive = p.poll() is None
    con = owns_console(p.pid) if alive else (None, None)
    p.kill()
    try:
        out, _ = p.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        out = ""
    logs = [l for l in out.splitlines() if l.strip()]
    print(f"{tag}: alive={alive} owns_console={con} piped_log_lines={len(logs)}")
    if logs:
        print(f"  first log: {logs[0][:90]}")


run_case("A no-env   ", {})
run_case("B LAP_CONSOLE=1", {"LAP_CONSOLE": "1"})
