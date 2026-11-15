"""E2E smoke for the VL scheduler on the portable debug build.

Steps (run while the app is CLOSED):
1. Read the debug library's template list (backup API snapshot).
2. Inject one `once` schedule into the debug app-config.json, set to fire
   ~2 minutes from now, bound to the debug library and its first enabled
   template. All 404 files of that library are already annotated, so the
   fired run is expected to finish instantly with everything skipped and
   zero gateway traffic.
The verification (waiting for the fire and reading vl_runs back) lives in a
separate script so the app can be launched in between.
"""
import json
import sqlite3
from datetime import datetime, timedelta
from pathlib import Path

CONFIG = Path(r"C:\Users\viccom\AppData\Local\com.julyx10.lap.debug\app-config.json")
DEBUG_DB = Path(r"C:\Users\viccom\AppData\Local\com.julyx10.lap.debug\libraries\default.db")


def snapshot_user_version_and_templates():
    src = sqlite3.connect(f"file:{DEBUG_DB.as_posix()}?mode=ro", uri=True)
    dst = sqlite3.connect(":memory:")
    src.backup(dst)
    version = dst.execute("PRAGMA user_version").fetchone()[0]
    templates = dst.execute(
        "SELECT id, name, enabled, version FROM vl_templates ORDER BY id"
    ).fetchall()
    files = dst.execute("SELECT COUNT(*) FROM afiles").fetchone()[0]
    src.close()
    dst.close()
    return version, templates, files


def inject_schedule(template_id):
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    fire_at = datetime.now() + timedelta(minutes=2)
    schedule = {
        "id": "smoke-0000-0000-0000-000000000001",
        "libraryId": "default",
        "templateId": template_id,
        "time": fire_at.strftime("%H:%M"),
        "mode": "once",
        "date": fire_at.strftime("%Y-%m-%d"),
        "enabled": True,
        "lastFiredAt": None,
    }
    # Note: the schedule must fire within its own date; if the +2 minutes
    # crossed midnight this smoke would expire unused — assert it does not.
    assert fire_at.date() == datetime.now().date(), "crossed midnight, rerun"
    config.setdefault("vl", {})["schedules"] = [schedule]
    CONFIG.write_text(json.dumps(config, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print("schedule injected:", json.dumps(schedule, ensure_ascii=False))


if __name__ == "__main__":
    version, templates, files = snapshot_user_version_and_templates()
    print("debug db user_version =", version, "| files =", files)
    print("templates =", templates)
    enabled = [t for t in templates if t[2]]
    assert enabled, "no enabled template in the debug library"
    inject_schedule(enabled[0][0])
