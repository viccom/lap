"""E2E smoke for the task-queue scheduler (v28 model) on the portable build.

Injects one pending vl_tasks row into the DEBUG library whose file set is
exactly two ALREADY-ANNOTATED photos, firing ~2 minutes from now. The fired
run must: come back source='scheduled', finish instantly with both files
skipped, produce zero new gateway calls, and flip the task to fired+run_id.

Run while the app is CLOSED (a raw write to the debug library db).
"""
import json
import sqlite3
from datetime import datetime, timedelta
from pathlib import Path

DEBUG_DB = Path(r"C:\Users\viccom\AppData\Local\com.julyx10.lap.debug\libraries\default.db")


def main():
    conn = sqlite3.connect(DEBUG_DB)
    # v28 must already be applied by the just-built exe? No — the app has not
    # opened this db since the rebuild. Apply the migration the same way the
    # app would: run the guarded statements directly.
    tables = {
        row[0]
        for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
    }
    assert "vl_runs" in tables and "vl_calls" in tables, "v27 tables missing"
    if "vl_tasks" not in tables:
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS vl_tasks (
                id TEXT PRIMARY KEY,
                template_id INTEGER NOT NULL,
                file_ids TEXT NOT NULL,
                force INTEGER NOT NULL DEFAULT 0,
                time TEXT NOT NULL,
                date TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending',
                run_id TEXT,
                error TEXT,
                created_at INTEGER NOT NULL
            )
            """
        )
    columns = {
        row[1] for row in conn.execute("PRAGMA table_info(vl_runs)").fetchall()
    }
    if "source" not in columns:
        conn.execute(
            "ALTER TABLE vl_runs ADD COLUMN source TEXT NOT NULL DEFAULT 'manual'"
        )
    # Pick two files that already carry an annotation for template 1.
    annotated = [
        row[0]
        for row in conn.execute(
            "SELECT DISTINCT file_id FROM vl_annotations WHERE template_id = 1 ORDER BY file_id LIMIT 2"
        ).fetchall()
    ]
    assert len(annotated) == 2, f"need two annotated files, got {annotated}"
    fire_at = datetime.now() + timedelta(minutes=2)
    assert fire_at.date() == datetime.now().date(), "crossed midnight, rerun"
    conn.execute("DELETE FROM vl_tasks WHERE id LIKE 'smoke-%'")
    conn.execute(
        "INSERT INTO vl_tasks(id, template_id, file_ids, force, time, date, state, created_at)"
        " VALUES('smoke-task-1', 1, ?, 0, ?, ?, 'pending', ?)",
        (
            json.dumps(annotated),
            fire_at.strftime("%H:%M"),
            fire_at.strftime("%Y-%m-%d"),
            int(datetime.now().timestamp()),
        ),
    )
    conn.commit()
    calls_before = conn.execute("SELECT COUNT(*) FROM vl_calls").fetchone()[0]
    conn.close()
    print("task injected for", annotated, "at", fire_at.strftime("%H:%M"))
    print("vl_calls before:", calls_before)


if __name__ == "__main__":
    main()
