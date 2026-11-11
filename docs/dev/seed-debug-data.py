"""把安装版的库做一致性快照复制到 debug 数据目录（源库正在运行，不能裸拷 WAL）。"""
import os
import shutil
import sqlite3
import sys

SRC_ROOT = r"C:\Users\viccom\AppData\Local\com.julyx10.lap"
DST_ROOT = r"C:\Users\viccom\AppData\Local\com.julyx10.lap.debug"

src_db = os.path.join(SRC_ROOT, "libraries", "default.db")
dst_db = os.path.join(DST_ROOT, "libraries", "default.db")
os.makedirs(os.path.dirname(dst_db), exist_ok=True)

# 1) 一致性快照：sqlite 官方 backup API，对正在写入的 WAL 库也安全
source = sqlite3.connect(f"file:{src_db}?mode=ro", uri=True)
target = sqlite3.connect(dst_db)
source.backup(target)
target.close()
source.close()
print("库快照完成:", dst_db, os.path.getsize(dst_db), "bytes")

# 2) 缩略图缓存：静态文件直接复制（{library}/{album}/{key前2位}/...）
src_cache = os.path.join(SRC_ROOT, "default")
dst_cache = os.path.join(DST_ROOT, "default")
if os.path.isdir(src_cache):
    shutil.copytree(src_cache, dst_cache, dirs_exist_ok=True)
    print("缩略图缓存复制完成:", dst_cache)

# 3) 配置：沿用安装版的相册/库状态（指向同一个真实相册）
src_cfg = os.path.join(SRC_ROOT, "app-config.json")
dst_cfg = os.path.join(DST_ROOT, "app-config.json")
if os.path.isfile(src_cfg):
    shutil.copyfile(src_cfg, dst_cfg)
    print("app-config.json 已复制（相册路径沿用安装版）")

# 4) 复核
conn = sqlite3.connect(f"file:{dst_db}?mode=ro", uri=True)
for sql in ("SELECT COUNT(*) FROM afiles",
            "SELECT COUNT(*) FROM afiles WHERE embeds IS NOT NULL",
            "SELECT COUNT(*) FROM faces",
            "SELECT COUNT(*) FROM albums"):
    print(sql, "=>", conn.execute(sql).fetchone()[0])
conn.close()
