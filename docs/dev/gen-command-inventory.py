#!/usr/bin/env python3
"""Generate the command inventory / split contract for the lap core-server split.

Parses the authoritative registration list in `src-tauri/src/main.rs`
(`invoke_handler`), pairs each command with its Rust signature, and classifies
it on two independent axes:

  split class  A = pure core function (no Tauri-injected parameter)
               B = needs adaptation (AppHandle / State / Window / Channel)
               C = desktop-only (window, native dialog, clipboard, drag, ...)
  exposure     read / write / admin / internal (never exposed over the network)

The classification is a first-pass contract; curated lists below are the source
of truth for C and for exposure exceptions. Re-run after adding commands:

    python -I docs/dev/gen-command-inventory.py

Outputs `docs/dev/ARCH-命令清单与拆分契约.md` (deterministic, no timestamps).
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MAIN_RS = ROOT / "src-tauri" / "src" / "main.rs"
SRC_DIR = ROOT / "src-tauri" / "src"
OUT = ROOT / "docs" / "dev" / "ARCH-命令清单与拆分契约.md"

# Commands that must stay in the desktop shell: they need a window, a native
# dialog, the OS clipboard, out-of-window drag, shell/OS integration, or the
# updater. They are never exposed over HTTP.
DESKTOP_ONLY = {
    # startup telemetry drives the desktop splash screen only
    "record_startup_stage",
    # native out-of-window file drag
    "start_file_drag",
    "cancel_file_drag",
    # native folder picker
    "select_folder",
    # OS shell integration
    "reveal_path",
    "open_external_url",
    "set_desktop_wallpaper",
    "get_external_app_display_name",
    "open_file_with_app",
    "open_files_with_app",
    # OS clipboard
    "copy_file_paths",
    "has_importable_clipboard",
    "import_clipboard",
    "copy_edited_image",
    # OS drag payload from an external drop
    "import_from_drag",
    "get_drag_payload",
}

# Exposure exceptions; everything else falls back to the group default.
EXPOSURE = {
    # --- read
    "get_app_config": "read",
    "get_system_region_locale": "read",
    "get_supported_format_extensions": "read",
    "get_ffmpeg_backed_image_extensions": "read",
    "check_gstreamer_available": "read",
    "get_thumbnail_cache_location": "read",
    "get_thumbnail_cache_migration_count": "read",
    "get_db_storage_dir": "read",
    "is_using_custom_db_storage": "read",
    "is_database_corrupted": "read",
    "get_library_info": "read",
    "get_library_state": "read",
    "get_current_library_state": "read",
    "save_library_state": "write",
    "set_last_selected_item_index": "write",
    "get_package_info": "read",
    "get_build_time": "read",
    "get_storage_file_info": "read",
    "get_index_recovery_info": "read",
    "clear_index_recovery_info": "write",
    # --- admin (state-changing, destructive, expensive, or secret-bearing)
    "change_thumbnail_cache_location": "admin",
    "cancel_thumbnail_cache_migration": "write",
    "change_db_storage_dir": "admin",
    "reset_db_storage_dir": "admin",
    "add_library": "admin",
    "edit_library": "admin",
    "remove_library": "admin",
    "hide_library": "write",
    "reorder_libraries": "write",
    "switch_library": "admin",
    "index_album": "admin",
    "cancel_indexing": "write",
    "recount_album": "admin",
    "generate_directory_thumbnails": "admin",
    "add_album": "admin",
    "edit_album": "write",
    "remove_album": "admin",
    "set_album_cover": "write",
    "reorder_albums": "write",
    "sync_album_folder_mtimes": "admin",
    "refresh_album_subfolders": "admin",
    "clean_unused_thumbnail_cache": "admin",
    "prepare_motion_photo_video": "read",
    "delete_folder": "admin",
    "prepare_folder_delete": "read",
    "delete_folder_permanently": "admin",
    "move_folder_outside_library": "admin",
    "delete_file_permanently": "admin",
    "delete_db_file": "admin",
    "batch_delete_files": "write",
    "delete_file": "write",
    "import_file": "admin",
    "import_and_organize": "admin",
    "cancel_import_and_organize": "write",
    "import_url": "admin",
    "import_file_bytes": "admin",
    "add_file_to_db": "admin",
    "backup_databases": "admin",
    "restore_databases": "admin",
    "parse_backup_file": "read",
    "get_db_storage_info": "read",
    "fts_reindex": "admin",
    "rebuild_caption_embeds": "admin",
    "set_image_search_model": "admin",
    "download_multilingual_image_search_model": "admin",
    "cancel_multilingual_image_search_model_download": "write",
    "generate_embedding": "read",
    "search_similar_images": "read",
    "index_faces": "admin",
    "cancel_face_index": "write",
    "reset_faces": "admin",
    "dedup_start_scan": "admin",
    "dedup_cancel_scan": "write",
    "dedup_delete": "admin",
    "similar_start_scan": "admin",
    "similar_cancel_scan": "write",
    "clear_video_cache": "admin",
    "set_vl_config": "admin",
    "run_vl_analysis": "write",
    "retry_vl_run": "write",
    "cancel_vl_run": "write",
    "rerank_search_with_llm": "write",
    "delete_vl_run": "write",
    "clear_vl_runs": "write",
    "create_vl_template": "write",
    "update_vl_template": "write",
    "delete_vl_template": "write",
    "queue_vl_task": "write",
    "update_vl_task_time": "write",
    "delete_vl_task": "write",
    # --- internal (core-only: no network surface at all)
    "record_startup_stage": "internal",
    "finish_startup": "internal",
    "get_ai_initialization_status": "internal",
    "start_file_drag": "internal",
    "cancel_file_drag": "internal",
    "select_folder": "internal",
    "reveal_path": "internal",
    "open_external_url": "internal",
    "set_desktop_wallpaper": "internal",
    "get_external_app_display_name": "internal",
    "open_file_with_app": "internal",
    "open_files_with_app": "internal",
    "copy_file_paths": "internal",
    "has_importable_clipboard": "internal",
    "import_clipboard": "internal",
    "copy_edited_image": "internal",
    "import_from_drag": "internal",
    "get_drag_payload": "internal",
}

# Read verbs: a command whose name starts with one of these only reads state.
READ_PREFIXES = (
    "get_", "list_", "is_", "has_", "check_", "search_", "fetch_", "count_",
    "preview_", "parse_", "embed_",
)

# Human-readable group headings (the registration comments are terse English).
GROUP_LABELS = {
    "startup / 原生拖拽": "启动与原生拖拽",
    "library": "图库",
    "album": "相册",
    "folder": "文件夹",
    "file query": "文件查询",
    "smart album": "智能相册",
    "collection": "集合",
    "folder file query": "文件夹文件查询",
    "file operations": "文件操作",
    "file metadata": "文件元数据",
    "favorite": "收藏与评分",
    "xmp": "XMP",
    "tag": "标签",
    "calendar": "日历",
    "camera": "相机",
    "location": "位置",
    "settings": "设置",
    "ai": "AI / 向量",
    "caption embeddings (semantic search over descriptions)": "描述向量（语义搜索）",
    "person (face recognition)": "人物（人脸）",
    "dedup": "去重 / 相似",
    "image understanding (vision language)": "图片理解（VL）",
    "exact search": "精确搜索（FTS）",
    "video": "视频",
    "backup / restore": "备份 / 恢复",
}

# Why a command is desktop-only.
DESKTOP_NOTES = {
    "record_startup_stage": "启动阶段遥测（桌面启动画面）",
    "start_file_drag": "出窗拖拽",
    "cancel_file_drag": "出窗拖拽",
    "select_folder": "原生目录选择对话框",
    "reveal_path": "资源管理器/Finder 定位",
    "open_external_url": "外部浏览器/深链",
    "set_desktop_wallpaper": "系统壁纸",
    "get_external_app_display_name": "外部程序",
    "open_file_with_app": "外部程序",
    "open_files_with_app": "外部程序",
    "copy_file_paths": "系统剪贴板",
    "has_importable_clipboard": "系统剪贴板",
    "import_clipboard": "系统剪贴板",
    "copy_edited_image": "系统剪贴板",
    "import_from_drag": "OS 拖放载荷",
    "get_drag_payload": "OS 拖放载荷",
}

GROUP_EXPOSURE = {
    "startup / 原生拖拽": "internal",
    "library": "admin",
    "album": "write",
    "folder": "write",
    "file query": "read",
    "smart album": "read",
    "collection": "write",
    "folder file query": "read",
    "file operations": "write",
    "file metadata": "write",
    "favorite": "write",
    "xmp": "write",
    "tag": "write",
    "calendar": "read",
    "camera": "read",
    "location": "read",
    "settings": "read",
    "ai": "admin",
    "caption embeddings (semantic search over descriptions)": "read",
    "person (face recognition)": "write",
    "dedup": "write",
    "image understanding (vision language)": "write",
    "exact search": "read",
    "video": "read",
    "backup / restore": "admin",
}

INJECTED_RE = re.compile(r":\s*&?\s*(AppHandle|State\s*<|Window\b|WebviewWindow|Channel\s*<|App\b)")


def parse_handler() -> list[dict]:
    text = MAIN_RS.read_text(encoding="utf-8")
    start = text.index(".invoke_handler(tauri::generate_handler![")
    end = text.index("])", start)
    block = text[start:end]

    entries: list[dict] = []
    group = "startup / 原生拖拽"
    for raw in block.splitlines():
        line = raw.strip()
        if not line:
            continue
        if line.startswith("//"):
            group = line.lstrip("/ ").strip()
            continue
        match = re.fullmatch(r"(t_[a-z_0-9]+)::([a-z_0-9]+),", line)
        if match:
            entries.append({"module": match.group(1), "name": match.group(2), "group": group})
    return entries


def parse_signatures() -> dict[str, str]:
    """Map command name -> raw parameter list (text between the outer parens)."""
    signatures: dict[str, str] = {}
    for path in sorted(SRC_DIR.glob("t_*.rs")):
        text = path.read_text(encoding="utf-8")
        for match in re.finditer(r"#\[tauri::command\]", text):
            tail = text[match.end(): match.end() + 4000]
            fn = re.search(r"\bfn\s+([a-z_0-9]+)\s*(?:<[^>]*>)?\s*\(", tail)
            if not fn:
                continue
            name = fn.group(1)
            # Step back onto the opening paren itself, then balance it so the
            # captured text starts at the first parameter character exactly
            # (an off-by-one here used to truncate every signature's first
            # parameter silently — classification never noticed because the
            # injection regex only needs `: Type` substrings).
            open_idx = match.end() + fn.end() - 1
            assert text[open_idx] == "(", f"{name}: paren not found"
            depth = 0
            close_idx = open_idx
            for close_idx in range(open_idx, len(text)):
                ch = text[close_idx]
                if ch == "(":
                    depth += 1
                elif ch == ")":
                    depth -= 1
                    if depth == 0:
                        break
            signatures[name] = text[open_idx + 1: close_idx]
    return signatures


def classify(entry: dict, signatures: dict[str, str]) -> tuple[str, str]:
    params = signatures.get(entry["name"])
    if params is None:
        return "B", "签名未解析"
    if entry["name"] in DESKTOP_ONLY:
        return "C", "桌面能力"
    hits = INJECTED_RE.findall(params)
    if hits:
        kinds = []
        for hit in hits:
            kind = re.sub(r"\s*<$", "", hit).strip()
            if kind and kind not in kinds:
                kinds.append(kind)
        return "B", "/".join(kinds)
    return "A", ""


def exposure_of(entry: dict) -> str:
    if entry["name"] in EXPOSURE:
        return EXPOSURE[entry["name"]]
    if entry["name"].startswith(READ_PREFIXES):
        return "read"
    return GROUP_EXPOSURE.get(entry["group"], "write")


CLASS_LABEL = {"A": "纯内核", "B": "需适配", "C": "原生专属"}
EXPOSURE_LABEL = {"read": "只读", "write": "写入", "admin": "管理员", "internal": "不暴露"}


def render(entries: list[dict], signatures: dict[str, str]) -> str:
    rows_by_group: dict[str, list[dict]] = {}
    for entry in entries:
        cls, note = classify(entry, signatures)
        entry["class"] = cls
        entry["note"] = note
        entry["exposure"] = exposure_of(entry)
        rows_by_group.setdefault(entry["group"], []).append(entry)

    counts = {"A": 0, "B": 0, "C": 0}
    exposure_counts = {"read": 0, "write": 0, "admin": 0, "internal": 0}
    for entry in entries:
        counts[entry["class"]] += 1
        exposure_counts[entry["exposure"]] += 1

    lines: list[str] = []
    lines.append("# lap 命令清单与拆分契约（前后端分离 / 服务化）")
    lines.append("")
    lines.append("> 生成方式：`python -I docs/dev/gen-command-inventory.py`（解析 `src-tauri/src/main.rs` 的")
    lines.append("> `invoke_handler` 注册表 + 各模块 `#[tauri::command]` 签名，勿手改本文件）。")
    lines.append("> 用途：前后端分离阶段的**唯一命令契约**；新增/删除命令后重跑脚本并提交差异。")
    lines.append("")
    lines.append("## 1. 分类规则")
    lines.append("")
    lines.append("两条**互相独立**的轴：")
    lines.append("")
    lines.append("| 轴 | 取值 | 含义 |")
    lines.append("|---|---|---|")
    lines.append("| 拆分分类 | **A 纯内核** | 签名无 Tauri 注入参数，只依赖参数与进程内 DB/文件系统，可直接迁入 `lap-core` |")
    lines.append("| | **B 需适配** | 注入 `AppHandle`/`State<…>`/`Window`/`Channel`，迁移时替换为 `AppCtx`/`EventSink` |")
    lines.append("| | **C 原生专属** | 窗口、原生对话框、剪贴板、出窗拖拽、外部程序、壁纸、更新器；留在桌面壳，**不暴露 HTTP** |")
    lines.append("| 暴露等级 | **只读** | 查询/状态/缩略图；社区/手机端默认可用 |")
    lines.append("| | **写入** | 改元数据、标签、收藏、相册编排、VL 任务等；需要已配对令牌 |")
    lines.append("| | **管理员** | 建/删库、迁移存储、导入、删除文件、扫描索引、备份恢复、密钥写入；仅本地或显式管理员令牌 |")
    lines.append("| | **不暴露** | 桌面专属命令，HTTP 白名单中不存在这些名字 |")
    lines.append("")
    lines.append("> 暴露等级是**阶段 3 的初始白名单草案**，实现时按命令逐条复核；默认拒绝，未列入白名单即 403。")
    lines.append("")
    lines.append("> A/B 由签名**自动判定**；C 与暴露等级来自脚本内的策展清单（`DESKTOP_ONLY` /")
    lines.append("> `EXPOSURE` / `READ_PREFIXES`）。新增命令默认落入「所在分组默认值」，必须人工复核。")
    lines.append("")
    lines.append("## 2. 统计")
    lines.append("")
    lines.append(f"- 命令总数：**{len(entries)}**")
    lines.append(f"- 拆分分类：A 纯内核 **{counts['A']}** · B 需适配 **{counts['B']}** · C 原生专属 **{counts['C']}**")
    lines.append(
        f"- 暴露等级：只读 **{exposure_counts['read']}** · 写入 **{exposure_counts['write']}** · "
        f"管理员 **{exposure_counts['admin']}** · 不暴露 **{exposure_counts['internal']}**"
    )
    lines.append("")
    lines.append("## 3. 清单（按注册表分组）")
    lines.append("")

    for group, group_entries in rows_by_group.items():
        label = GROUP_LABELS.get(group, group)
        lines.append(f"### {label}（{len(group_entries)}）")
        lines.append("")
        lines.append("| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |")
        lines.append("|---:|---|---|---|---|")
        for index, entry in enumerate(group_entries, 1):
            note = DESKTOP_NOTES.get(entry["name"], entry["note"])
            lines.append(
                f"| {index} | `{entry['module']}::{entry['name']}` | "
                f"{entry['class']} {CLASS_LABEL[entry['class']]} | "
                f"{EXPOSURE_LABEL[entry['exposure']]} | {note} |"
            )
        lines.append("")

    lines.append("## 4. 迁移顺序建议（与方案文档一致）")
    lines.append("")
    lines.append("1. **浏览闭环第一批**：`file query` / `smart album` / `collection` 只读子集 /")
    lines.append("   `folder file query` 只读子集 / `album` 只读子集 / `folder` 只读子集 / `settings` /")
    lines.append("   `calendar` / `camera` / `location` / `exact search` 只读子集")
    lines.append("   —— 足够支撑 Web 端「搜索 → 网格 → 查看」。")
    lines.append("2. **配置与状态**：`get_app_config` / `get_library_state` / `get_current_library_state` /")
    lines.append("   `save_library_state`；`switch_library` 等库管理命令留到管理员面。")
    lines.append("3. **写入面**：标签、收藏、评分、集合、相册编排。")
    lines.append("4. **重活与管理员面**：索引、导入、删除、去重、人脸、VL、备份恢复、模型下载。")
    lines.append("5. **C 类永不迁移**：只做 `capabilities()` 门控，Web 端隐藏入口。")
    lines.append("")
    lines.append("## 5. 维护约定")
    lines.append("")
    lines.append("- 新增命令必须同时更新本清单（重跑脚本），并在 PR 中说明拆分分类与暴露等级。")
    lines.append("- 集成测试沿用既有 `VL_COMMANDS` 式守卫：注册表与清单不一致即失败。")
    lines.append("- 本文件由脚本生成；手改会在下次重跑时被覆盖。")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    entries = parse_handler()
    signatures = parse_signatures()
    if len(entries) != len(signatures):
        print(f"warning: handler={len(entries)} signatures={len(signatures)}", file=sys.stderr)
    missing = [e["name"] for e in entries if e["name"] not in signatures]
    if missing:
        print(f"warning: no signature for {missing}", file=sys.stderr)
    if "--dump" in sys.argv:
        print(json.dumps(entries, ensure_ascii=False, indent=2))
        return 0
    OUT.write_text(render(entries, signatures), encoding="utf-8", newline="\n")
    print(f"wrote {OUT.relative_to(ROOT)} ({len(entries)} commands)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
