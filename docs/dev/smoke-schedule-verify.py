"""Wait for the injected schedule to fire and verify the resulting run.

Polls (backup-API snapshots, safe on a live DB) until a new vl_runs row for
the smoke template appears or the timeout hits, then reports:
- the run row (state / counters / template)
- the persisted last_fired_at of the schedule in app-config.json
- the in-flight LLM call evidence count (must stay 0: everything skipped)
"""
import json
import sqlite3
import time
from pathlib import Path

CONFIG = Path(r"C:\Users\viccom\AppData\Local\com.julyx10.lap.debug\app-config.json")
DEBUG_DB = Path(r"C:\Users\viccom\AppData\Local\com.julyx10.lap.debug\libraries\default.db")
SMOKE_ID = "smoke-0000-0000-0000-000000000001"
TIMEOUT_SECS = 300


def snapshot():
    src = sqlite3.connect(f"file:{DEBUG_DB.as_posix()}?mode=ro", uri=True)
    dst = sqlite3.connect(":memory:")
    src.backup(dst)
    src.close()
    rows = dst.execute(
        "SELECT run_id, state, total, processed, succeeded, failed, skipped, started_at, finished_at"
        " FROM vl_runs ORDER BY started_at DESC LIMIT 3"
    ).fetchall()
    calls = dst.execute("SELECT COUNT(*) FROM vl_calls").fetchone()[0]
    dst.close()
    return rows, calls


def main():
    started_before = time.time()
    deadline = started_before + TIMEOUT_SECS
    while time.time() < deadline:
        rows, calls = snapshot()
        # Only a run whose row is younger than this script counts: the
        # history already holds terminal runs from earlier sessions.
        newest = rows[0] if rows else None
        fired = newest is not None and newest[7] > started_before - 60
        if fired and newest[1] in ("finished", "cancelled", "failed", "interrupted"):
            config = json.loads(CONFIG.read_text(encoding="utf-8"))
            schedules = config.get("vl", {}).get("schedules", [])
            smoke = next((s for s in schedules if s.get("id") == SMOKE_ID), None)
            print("newest runs:", rows)
            print("vl_calls total:", calls)
            print("schedule after fire:", json.dumps(smoke, ensure_ascii=False))
            return
        time.sleep(10)
    print("TIMEOUT: no new terminal run appeared")
    rows, calls = snapshot()
    print("runs:", rows)


if __name__ == "__main__":
    main()
