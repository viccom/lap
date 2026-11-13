"""Read-only verification of a Lap library database.

Reports the user_version (migration level), the VL tables, the run/call counts
and the newest run rows, so the migration state of a library can be verified
without touching it. Usage: python -I verify_library_db.py <path-to.db>
"""
import sqlite3
import sys


def main():
    path = sys.argv[1]
    con = sqlite3.connect(f'file:{path}?mode=ro', uri=True)
    cur = con.cursor()

    version = cur.execute('PRAGMA user_version').fetchone()[0]
    print('user_version:', version)

    tables = [row[0] for row in cur.execute(
        "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")]
    for name in ('vl_runs', 'vl_calls', 'vl_annotations', 'vl_templates', 'afiles'):
        print(f'table {name}:', name in tables)

    try:
        print('afiles:', cur.execute('SELECT COUNT(*) FROM afiles').fetchone()[0])
    except sqlite3.Error as error:
        print('afiles: error', error)

    try:
        columns = [row[1] for row in cur.execute('PRAGMA table_info(vl_calls)')]
        print('vl_calls.run_id present:', 'run_id' in columns)
        print('vl_calls rows:', cur.execute('SELECT COUNT(*) FROM vl_calls').fetchone()[0])
    except sqlite3.Error as error:
        print('vl_calls: error', error)

    try:
        print('vl_runs rows:', cur.execute('SELECT COUNT(*) FROM vl_runs').fetchone()[0])
        for row in cur.execute(
            'SELECT run_id, state, total, processed, succeeded, failed, skipped '
            'FROM vl_runs ORDER BY started_at DESC LIMIT 5'):
            print('  run', row)
    except sqlite3.Error as error:
        print('vl_runs: error', error)

    con.close()


if __name__ == '__main__':
    main()
