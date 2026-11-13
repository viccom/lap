# Read-only probe: does identical content in different paths get multiple ids?
import sqlite3, sys, json

db = r"C:\Users\viccom\AppData\Local\com.julyx10.lap.debug\libraries\default.db"
conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)

cur = conn.cursor()

print("== afiles PK / unique constraints ==")
for row in cur.execute("SELECT sql FROM sqlite_master WHERE tbl_name IN ('afiles','file_hashes','duplicate_groups')"):
    print(row[0][:300].replace("\n", " "))
print()

print("== file_hashes: same content (hash+size) with multiple file_ids ==")
rows = list(cur.execute("""
    SELECT h.hash, h.file_size, COUNT(*) AS n, GROUP_CONCAT(h.file_id)
    FROM file_hashes h
    GROUP BY h.hash, h.file_size HAVING n > 1 LIMIT 10"""))
if not rows:
    print("(none — dedup scan may not have run, or no byte-identical files)")
else:
    for r in rows:
        print(f"hash={r[0][:16]}… size={r[1]} ids={r[3]} (count={r[2]})")
print()

print("== same name+size in different folders (no hash needed) ==")
rows = list(cur.execute("""
    SELECT a.name, a.size, COUNT(*) AS n, GROUP_CONCAT(a.folder_id), GROUP_CONCAT(a.id)
    FROM afiles a GROUP BY a.name, a.size HAVING n > 1 LIMIT 10"""))
if not rows:
    print("(none)")
else:
    for r in rows:
        print(f"name={r[0]} size={r[1]} folder_ids={r[3]} file_ids={r[4]} (count={r[2]})")
print()

print("== do identical-content ids share one afiles row? (id vs hash join) ==")
rows = list(cur.execute("""
    SELECT h.hash, f.id, f.folder_id, f.name
    FROM file_hashes h JOIN afiles f ON f.id = h.file_id
    WHERE h.hash IN (SELECT hash FROM file_hashes GROUP BY hash HAVING COUNT(*) > 1)
    ORDER BY h.hash, f.id LIMIT 20"""))
if not rows:
    print("(no multi-id content found in this library)")
else:
    for r in rows:
        print(f"hash={r[0][:16]}… -> file_id={r[1]} folder_id={r[2]} name={r[3]}")
conn.close()
