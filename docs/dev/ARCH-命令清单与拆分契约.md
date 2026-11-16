# lap 命令清单与拆分契约（前后端分离 / 服务化）

> 生成方式：`python -I docs/dev/gen-command-inventory.py`（解析 `src-tauri/src/main.rs` 的
> `invoke_handler` 注册表 + 各模块 `#[tauri::command]` 签名，勿手改本文件）。
> 用途：前后端分离阶段的**唯一命令契约**；新增/删除命令后重跑脚本并提交差异。

## 1. 分类规则

两条**互相独立**的轴：

| 轴 | 取值 | 含义 |
|---|---|---|
| 拆分分类 | **A 纯内核** | 签名无 Tauri 注入参数，只依赖参数与进程内 DB/文件系统，可直接迁入 `lap-core` |
| | **B 需适配** | 注入 `AppHandle`/`State<…>`/`Window`/`Channel`，迁移时替换为 `AppCtx`/`EventSink` |
| | **C 原生专属** | 窗口、原生对话框、剪贴板、出窗拖拽、外部程序、壁纸、更新器；留在桌面壳，**不暴露 HTTP** |
| 暴露等级 | **只读** | 查询/状态/缩略图；社区/手机端默认可用 |
| | **写入** | 改元数据、标签、收藏、相册编排、VL 任务等；需要已配对令牌 |
| | **管理员** | 建/删库、迁移存储、导入、删除文件、扫描索引、备份恢复、密钥写入；仅本地或显式管理员令牌 |
| | **不暴露** | 桌面专属命令，HTTP 白名单中不存在这些名字 |

> 暴露等级是**阶段 3 的初始白名单草案**，实现时按命令逐条复核；默认拒绝，未列入白名单即 403。

> A/B 由签名**自动判定**；C 与暴露等级来自脚本内的策展清单（`DESKTOP_ONLY` /
> `EXPOSURE` / `READ_PREFIXES`）。新增命令默认落入「所在分组默认值」，必须人工复核。

## 2. 统计

- 命令总数：**253**
- 拆分分类：A 纯内核 **233** · B 需适配 **4** · C 原生专属 **16**
- 暴露等级：只读 **110** · 写入 **87** · 管理员 **38** · 不暴露 **18**

## 3. 清单（按注册表分组）

### 启动与原生拖拽（5）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_startup::record_startup_stage` | C 原生专属 | 不暴露 | 启动阶段遥测（桌面启动画面） |
| 2 | `t_startup::finish_startup` | A 纯内核 | 不暴露 |  |
| 3 | `t_startup::get_ai_initialization_status` | A 纯内核 | 不暴露 |  |
| 4 | `t_drag::start_file_drag` | C 原生专属 | 不暴露 | 出窗拖拽 |
| 5 | `t_drag::cancel_file_drag` | C 原生专属 | 不暴露 | 出窗拖拽 |

### 图库（25）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_app_config` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_system_region_locale` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::get_supported_format_extensions` | A 纯内核 | 只读 |  |
| 4 | `t_cmds::get_ffmpeg_backed_image_extensions` | A 纯内核 | 只读 |  |
| 5 | `t_cmds::check_gstreamer_available` | A 纯内核 | 只读 |  |
| 6 | `t_cmds::set_last_selected_item_index` | A 纯内核 | 写入 |  |
| 7 | `t_cmds::get_thumbnail_cache_location` | A 纯内核 | 只读 |  |
| 8 | `t_cmds::get_thumbnail_cache_migration_count` | A 纯内核 | 只读 |  |
| 9 | `t_cmds::change_thumbnail_cache_location` | B 需适配 | 管理员 | Channel |
| 10 | `t_cmds::cancel_thumbnail_cache_migration` | A 纯内核 | 写入 |  |
| 11 | `t_cmds::get_db_storage_dir` | A 纯内核 | 只读 |  |
| 12 | `t_cmds::is_using_custom_db_storage` | A 纯内核 | 只读 |  |
| 13 | `t_cmds::change_db_storage_dir` | A 纯内核 | 管理员 |  |
| 14 | `t_cmds::reset_db_storage_dir` | A 纯内核 | 管理员 |  |
| 15 | `t_cmds::add_library` | A 纯内核 | 管理员 |  |
| 16 | `t_cmds::edit_library` | A 纯内核 | 管理员 |  |
| 17 | `t_cmds::remove_library` | A 纯内核 | 管理员 |  |
| 18 | `t_cmds::hide_library` | A 纯内核 | 写入 |  |
| 19 | `t_cmds::reorder_libraries` | A 纯内核 | 写入 |  |
| 20 | `t_cmds::switch_library` | B 需适配 | 管理员 | AppHandle |
| 21 | `t_cmds::is_database_corrupted` | A 纯内核 | 只读 |  |
| 22 | `t_cmds::get_library_info` | A 纯内核 | 只读 |  |
| 23 | `t_cmds::save_library_state` | A 纯内核 | 写入 |  |
| 24 | `t_cmds::get_library_state` | A 纯内核 | 只读 |  |
| 25 | `t_cmds::get_current_library_state` | A 纯内核 | 只读 |  |

### 相册（16）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_all_albums` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_all_album_folders` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::generate_directory_thumbnails` | A 纯内核 | 管理员 |  |
| 4 | `t_cmds::get_album` | A 纯内核 | 只读 |  |
| 5 | `t_cmds::check_album_accessibility` | A 纯内核 | 只读 |  |
| 6 | `t_cmds::recount_album` | A 纯内核 | 管理员 |  |
| 7 | `t_cmds::get_album_visible_counts` | A 纯内核 | 只读 |  |
| 8 | `t_cmds::add_album` | B 需适配 | 管理员 | AppHandle |
| 9 | `t_cmds::edit_album` | A 纯内核 | 写入 |  |
| 10 | `t_cmds::remove_album` | A 纯内核 | 管理员 |  |
| 11 | `t_cmds::reorder_albums` | A 纯内核 | 写入 |  |
| 12 | `t_cmds::set_album_cover` | A 纯内核 | 写入 |  |
| 13 | `t_cmds::index_album` | A 纯内核 | 管理员 |  |
| 14 | `t_cmds::cancel_indexing` | A 纯内核 | 写入 |  |
| 15 | `t_cmds::get_index_recovery_info` | A 纯内核 | 只读 |  |
| 16 | `t_cmds::clear_index_recovery_info` | A 纯内核 | 写入 |  |

### 文件夹（18）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::select_folder` | C 原生专属 | 不暴露 | 原生目录选择对话框 |
| 2 | `t_cmds::fetch_folder` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::count_folder` | A 纯内核 | 只读 |  |
| 4 | `t_cmds::list_album_subfolders` | A 纯内核 | 只读 |  |
| 5 | `t_cmds::create_folder` | A 纯内核 | 写入 |  |
| 6 | `t_cmds::rename_folder` | A 纯内核 | 写入 |  |
| 7 | `t_cmds::move_folder` | A 纯内核 | 写入 |  |
| 8 | `t_cmds::move_folder_outside_library` | A 纯内核 | 管理员 |  |
| 9 | `t_cmds::copy_folder` | A 纯内核 | 写入 |  |
| 10 | `t_cmds::delete_folder` | A 纯内核 | 管理员 |  |
| 11 | `t_cmds::prepare_folder_delete` | A 纯内核 | 只读 |  |
| 12 | `t_cmds::delete_folder_permanently` | A 纯内核 | 管理员 |  |
| 13 | `t_cmds::reveal_path` | C 原生专属 | 不暴露 | 资源管理器/Finder 定位 |
| 14 | `t_cmds::open_external_url` | C 原生专属 | 不暴露 | 外部浏览器/深链 |
| 15 | `t_cmds::set_desktop_wallpaper` | C 原生专属 | 不暴露 | 系统壁纸 |
| 16 | `t_cmds::get_external_app_display_name` | C 原生专属 | 不暴露 | 外部程序 |
| 17 | `t_cmds::open_file_with_app` | C 原生专属 | 不暴露 | 外部程序 |
| 18 | `t_cmds::open_files_with_app` | C 原生专属 | 不暴露 | 外部程序 |

### 文件查询（9）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_query_count_and_sum` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_query_time_line` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::get_query_files` | A 纯内核 | 只读 |  |
| 4 | `t_cmds::get_grouped_query_rows` | A 纯内核 | 只读 |  |
| 5 | `t_cmds::get_group_file_ids` | A 纯内核 | 只读 |  |
| 6 | `t_cmds::get_grouped_file_position` | A 纯内核 | 只读 |  |
| 7 | `t_cmds::get_query_file_ids` | A 纯内核 | 只读 |  |
| 8 | `t_cmds::get_library_visible_counts` | A 纯内核 | 只读 |  |
| 9 | `t_cmds::get_query_file_position` | A 纯内核 | 只读 |  |

### 智能相册（7）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_smart_query_count_and_sum` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_smart_query_time_line` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::get_smart_query_files` | A 纯内核 | 只读 |  |
| 4 | `t_cmds::get_smart_grouped_query_rows` | A 纯内核 | 只读 |  |
| 5 | `t_cmds::get_smart_group_file_ids` | A 纯内核 | 只读 |  |
| 6 | `t_cmds::get_smart_query_file_ids` | A 纯内核 | 只读 |  |
| 7 | `t_cmds::get_smart_query_file_position` | A 纯内核 | 只读 |  |

### 集合（17）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::list_collections` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_collection_counts` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::create_collection` | A 纯内核 | 写入 |  |
| 4 | `t_cmds::rename_collection` | A 纯内核 | 写入 |  |
| 5 | `t_cmds::delete_collection` | A 纯内核 | 写入 |  |
| 6 | `t_cmds::reorder_collections` | A 纯内核 | 写入 |  |
| 7 | `t_cmds::add_files_to_collection` | A 纯内核 | 写入 |  |
| 8 | `t_cmds::remove_files_from_collection` | A 纯内核 | 写入 |  |
| 9 | `t_cmds::get_collection_selection_counts` | A 纯内核 | 只读 |  |
| 10 | `t_cmds::clear_collection` | A 纯内核 | 写入 |  |
| 11 | `t_cmds::get_collection_file_ids` | A 纯内核 | 只读 |  |
| 12 | `t_cmds::get_file_collections` | A 纯内核 | 只读 |  |
| 13 | `t_cmds::get_collection_count_and_sum` | A 纯内核 | 只读 |  |
| 14 | `t_cmds::get_collection_files` | A 纯内核 | 只读 |  |
| 15 | `t_cmds::get_collection_grouped_query_rows` | A 纯内核 | 只读 |  |
| 16 | `t_cmds::get_collection_group_file_ids` | A 纯内核 | 只读 |  |
| 17 | `t_cmds::get_collection_query_file_ids` | A 纯内核 | 只读 |  |

### 文件夹文件查询（6）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_files_by_ids` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_folder_files` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::sync_album_folder_mtimes` | A 纯内核 | 管理员 |  |
| 4 | `t_cmds::refresh_album_subfolders` | A 纯内核 | 管理员 |  |
| 5 | `t_cmds::is_directory_accessible` | A 纯内核 | 只读 |  |
| 6 | `t_cmds::get_folder_thumb_count` | A 纯内核 | 只读 |  |

### 文件操作（24）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::edit_image` | A 纯内核 | 写入 |  |
| 2 | `t_cmds::render_montage` | A 纯内核 | 写入 |  |
| 3 | `t_cmds::copy_edited_image` | C 原生专属 | 不暴露 | 系统剪贴板 |
| 4 | `t_cmds::copy_images` | B 需适配 | 写入 | AppHandle |
| 5 | `t_cmds::copy_file_paths` | C 原生专属 | 不暴露 | 系统剪贴板 |
| 6 | `t_cmds::rename_file` | A 纯内核 | 写入 |  |
| 7 | `t_cmds::preview_batch_rename` | A 纯内核 | 只读 |  |
| 8 | `t_cmds::batch_rename_files` | A 纯内核 | 写入 |  |
| 9 | `t_cmds::move_file` | A 纯内核 | 写入 |  |
| 10 | `t_cmds::move_file_outside_library` | A 纯内核 | 写入 |  |
| 11 | `t_cmds::copy_file` | A 纯内核 | 写入 |  |
| 12 | `t_cmds::import_file` | A 纯内核 | 管理员 |  |
| 13 | `t_cmds::import_and_organize` | A 纯内核 | 管理员 |  |
| 14 | `t_cmds::cancel_import_and_organize` | A 纯内核 | 写入 |  |
| 15 | `t_cmds::import_url` | A 纯内核 | 管理员 |  |
| 16 | `t_cmds::import_from_drag` | C 原生专属 | 不暴露 | OS 拖放载荷 |
| 17 | `t_cmds::get_drag_payload` | C 原生专属 | 不暴露 | OS 拖放载荷 |
| 18 | `t_cmds::import_file_bytes` | A 纯内核 | 管理员 |  |
| 19 | `t_cmds::has_importable_clipboard` | C 原生专属 | 不暴露 | 系统剪贴板 |
| 20 | `t_cmds::import_clipboard` | C 原生专属 | 不暴露 | 系统剪贴板 |
| 21 | `t_cmds::delete_file` | A 纯内核 | 写入 |  |
| 22 | `t_cmds::delete_file_permanently` | A 纯内核 | 管理员 |  |
| 23 | `t_cmds::delete_db_file` | A 纯内核 | 管理员 |  |
| 24 | `t_cmds::batch_delete_files` | A 纯内核 | 写入 |  |

### 文件元数据（23）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::edit_file_comment` | A 纯内核 | 写入 |  |
| 2 | `t_cmds::clean_unused_thumbnail_cache` | A 纯内核 | 管理员 |  |
| 3 | `t_cmds::get_file_thumb` | A 纯内核 | 只读 |  |
| 4 | `t_cmds::get_file_thumb_by_id` | A 纯内核 | 只读 |  |
| 5 | `t_cmds::get_file_thumbs` | A 纯内核 | 只读 |  |
| 6 | `t_cmds::get_file_info` | A 纯内核 | 只读 |  |
| 7 | `t_cmds::get_file_metadata_details` | A 纯内核 | 只读 |  |
| 8 | `t_cmds::cancel_file_metadata_requests` | A 纯内核 | 写入 |  |
| 9 | `t_cmds::update_file_info` | A 纯内核 | 写入 |  |
| 10 | `t_cmds::begin_file_transfer_batch` | A 纯内核 | 写入 |  |
| 11 | `t_cmds::end_file_transfer_batch` | A 纯内核 | 写入 |  |
| 12 | `t_cmds::begin_file_refresh_batch` | A 纯内核 | 写入 |  |
| 13 | `t_cmds::end_file_refresh_batch` | A 纯内核 | 写入 |  |
| 14 | `t_cmds::refresh_selected_file_info` | A 纯内核 | 写入 |  |
| 15 | `t_cmds::change_video_thumbnail` | A 纯内核 | 写入 |  |
| 16 | `t_cmds::index_cloud_file` | A 纯内核 | 写入 |  |
| 17 | `t_cmds::is_cloud_only_file` | A 纯内核 | 只读 |  |
| 18 | `t_cmds::prepare_motion_photo_video` | A 纯内核 | 只读 |  |
| 19 | `t_cmds::add_file_to_db` | A 纯内核 | 管理员 |  |
| 20 | `t_cmds::check_file_exists` | A 纯内核 | 只读 |  |
| 21 | `t_cmds::check_file_accessibility` | A 纯内核 | 只读 |  |
| 22 | `t_cmds::set_file_rotate` | A 纯内核 | 写入 |  |
| 23 | `t_cmds::get_file_has_tags` | A 纯内核 | 只读 |  |

### 收藏与评分（9）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_favorite_folders` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_folder_favorite` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::set_folder_favorite` | A 纯内核 | 写入 |  |
| 4 | `t_cmds::get_folder_search_excluded` | A 纯内核 | 只读 |  |
| 5 | `t_cmds::set_folder_search_excluded` | A 纯内核 | 写入 |  |
| 6 | `t_cmds::set_file_favorite` | A 纯内核 | 写入 |  |
| 7 | `t_cmds::set_file_rating` | A 纯内核 | 写入 |  |
| 8 | `t_cmds::set_file_culling_flag` | A 纯内核 | 写入 |  |
| 9 | `t_cmds::batch_update_file_metadata` | A 纯内核 | 写入 |  |

### XMP（3）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_xmp_import_settings` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::set_xmp_import_settings` | A 纯内核 | 写入 |  |
| 3 | `t_cmds::save_xmp_metadata` | A 纯内核 | 写入 |  |

### 标签（13）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_all_tags` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_tag_counts` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::get_tag_name` | A 纯内核 | 只读 |  |
| 4 | `t_cmds::create_tag` | A 纯内核 | 写入 |  |
| 5 | `t_cmds::move_tag` | A 纯内核 | 写入 |  |
| 6 | `t_cmds::get_tag_subtree_info` | A 纯内核 | 只读 |  |
| 7 | `t_cmds::rename_tag` | A 纯内核 | 写入 |  |
| 8 | `t_cmds::delete_tag` | A 纯内核 | 写入 |  |
| 9 | `t_cmds::get_tags_for_file` | A 纯内核 | 只读 |  |
| 10 | `t_cmds::add_tag_to_file` | A 纯内核 | 写入 |  |
| 11 | `t_cmds::remove_tag_from_file` | A 纯内核 | 写入 |  |
| 12 | `t_cmds::get_tag_selection_counts` | A 纯内核 | 只读 |  |
| 13 | `t_cmds::apply_tags_to_files` | A 纯内核 | 写入 |  |

### 日历（1）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_taken_dates` | A 纯内核 | 只读 |  |

### 相机（2）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_camera_info` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_lens_info` | A 纯内核 | 只读 |  |

### 位置（2）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_location_info` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_gps_map_points` | A 纯内核 | 只读 |  |

### 设置（3）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_package_info` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_build_time` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::get_storage_file_info` | A 纯内核 | 只读 |  |

### AI / 向量（7）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::check_ai_status` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::get_image_search_model_status` | A 纯内核 | 只读 |  |
| 3 | `t_cmds::set_image_search_model` | A 纯内核 | 管理员 |  |
| 4 | `t_cmds::download_multilingual_image_search_model` | A 纯内核 | 管理员 |  |
| 5 | `t_cmds::cancel_multilingual_image_search_model_download` | A 纯内核 | 写入 |  |
| 6 | `t_cmds::generate_embedding` | A 纯内核 | 只读 |  |
| 7 | `t_cmds::search_similar_images` | A 纯内核 | 只读 |  |

### 描述向量（语义搜索）（3）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_embed::embed_status` | A 纯内核 | 只读 |  |
| 2 | `t_embed::rebuild_caption_embeds` | A 纯内核 | 管理员 |  |
| 3 | `t_embed::search_captions` | A 纯内核 | 只读 |  |

### 人物（人脸）（12）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::index_faces` | A 纯内核 | 管理员 |  |
| 2 | `t_cmds::cancel_face_index` | A 纯内核 | 写入 |  |
| 3 | `t_cmds::reset_faces` | A 纯内核 | 管理员 |  |
| 4 | `t_cmds::is_face_indexing` | A 纯内核 | 只读 |  |
| 5 | `t_cmds::get_face_stats` | A 纯内核 | 只读 |  |
| 6 | `t_cmds::get_persons` | A 纯内核 | 只读 |  |
| 7 | `t_cmds::get_persons_page` | A 纯内核 | 只读 |  |
| 8 | `t_cmds::rename_person` | A 纯内核 | 写入 |  |
| 9 | `t_cmds::delete_person` | A 纯内核 | 写入 |  |
| 10 | `t_cmds::get_faces_for_file` | A 纯内核 | 只读 |  |
| 11 | `t_af_area::get_af_areas` | A 纯内核 | 只读 |  |
| 12 | `t_cmds::get_person_thumbnail` | A 纯内核 | 只读 |  |

### 去重 / 相似（16）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::dedup_start_scan` | A 纯内核 | 管理员 |  |
| 2 | `t_cmds::dedup_get_scan_status` | A 纯内核 | 写入 |  |
| 3 | `t_cmds::dedup_cancel_scan` | A 纯内核 | 写入 |  |
| 4 | `t_cmds::dedup_list_groups` | A 纯内核 | 写入 |  |
| 5 | `t_cmds::dedup_get_overview` | A 纯内核 | 写入 |  |
| 6 | `t_cmds::dedup_set_keep` | A 纯内核 | 写入 |  |
| 7 | `t_cmds::dedup_delete` | A 纯内核 | 管理员 |  |
| 8 | `t_cmds::similar_start_scan` | A 纯内核 | 管理员 |  |
| 9 | `t_cmds::similar_get_scan_status` | A 纯内核 | 写入 |  |
| 10 | `t_cmds::similar_cancel_scan` | A 纯内核 | 写入 |  |
| 11 | `t_cmds::similar_get_eligible_count` | A 纯内核 | 写入 |  |
| 12 | `t_cmds::similar_list_groups` | A 纯内核 | 写入 |  |
| 13 | `t_cmds::similar_get_overview` | A 纯内核 | 写入 |  |
| 14 | `t_cmds::similar_get_group` | A 纯内核 | 写入 |  |
| 15 | `t_cmds::similar_set_keep` | A 纯内核 | 写入 |  |
| 16 | `t_cmds::similar_has_scan` | A 纯内核 | 写入 |  |

### 图片理解（VL）（21）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_vl::get_vl_config` | A 纯内核 | 只读 |  |
| 2 | `t_vl::set_vl_config` | A 纯内核 | 管理员 |  |
| 3 | `t_vl::list_vl_templates` | A 纯内核 | 只读 |  |
| 4 | `t_vl::create_vl_template` | A 纯内核 | 写入 |  |
| 5 | `t_vl::update_vl_template` | A 纯内核 | 写入 |  |
| 6 | `t_vl::delete_vl_template` | A 纯内核 | 写入 |  |
| 7 | `t_vl::run_vl_analysis` | A 纯内核 | 写入 |  |
| 8 | `t_vl::get_vl_annotations` | A 纯内核 | 只读 |  |
| 9 | `t_vl::get_vl_calls` | A 纯内核 | 只读 |  |
| 10 | `t_vl::cancel_vl_run` | A 纯内核 | 写入 |  |
| 11 | `t_vl::rerank_search_with_llm` | A 纯内核 | 写入 |  |
| 12 | `t_vl::list_vl_models` | A 纯内核 | 只读 |  |
| 13 | `t_vl::list_vl_runs` | A 纯内核 | 只读 |  |
| 14 | `t_vl::list_vl_run_calls` | A 纯内核 | 只读 |  |
| 15 | `t_vl::retry_vl_run` | A 纯内核 | 写入 |  |
| 16 | `t_vl::delete_vl_run` | A 纯内核 | 写入 |  |
| 17 | `t_vl::clear_vl_runs` | A 纯内核 | 写入 |  |
| 18 | `t_vl_schedule::queue_vl_task` | A 纯内核 | 写入 |  |
| 19 | `t_vl_schedule::list_vl_tasks` | A 纯内核 | 只读 |  |
| 20 | `t_vl_schedule::update_vl_task_time` | A 纯内核 | 写入 |  |
| 21 | `t_vl_schedule::delete_vl_task` | A 纯内核 | 写入 |  |

### 精确搜索（FTS）（3）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_fts::search_fts` | A 纯内核 | 只读 |  |
| 2 | `t_fts::fts_reindex` | A 纯内核 | 管理员 |  |
| 3 | `t_fts::fts_status` | A 纯内核 | 只读 |  |

### 视频（4）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_video::prepare_video` | A 纯内核 | 只读 |  |
| 2 | `t_video::get_video_stream_url` | A 纯内核 | 只读 |  |
| 3 | `t_video::cancel_video_prepare` | A 纯内核 | 只读 |  |
| 4 | `t_video::clear_video_cache` | A 纯内核 | 管理员 |  |

### 备份 / 恢复（4）

| # | 命令 | 拆分分类 | 暴露 | 注入/备注 |
|---:|---|---|---|---|
| 1 | `t_cmds::get_db_storage_info` | A 纯内核 | 只读 |  |
| 2 | `t_cmds::backup_databases` | A 纯内核 | 管理员 |  |
| 3 | `t_cmds::parse_backup_file` | A 纯内核 | 只读 |  |
| 4 | `t_cmds::restore_databases` | A 纯内核 | 管理员 |  |

## 4. 迁移顺序建议（与方案文档一致）

1. **浏览闭环第一批**：`file query` / `smart album` / `collection` 只读子集 /
   `folder file query` 只读子集 / `album` 只读子集 / `folder` 只读子集 / `settings` /
   `calendar` / `camera` / `location` / `exact search` 只读子集
   —— 足够支撑 Web 端「搜索 → 网格 → 查看」。
2. **配置与状态**：`get_app_config` / `get_library_state` / `get_current_library_state` /
   `save_library_state`；`switch_library` 等库管理命令留到管理员面。
3. **写入面**：标签、收藏、评分、集合、相册编排。
4. **重活与管理员面**：索引、导入、删除、去重、人脸、VL、备份恢复、模型下载。
5. **C 类永不迁移**：只做 `capabilities()` 门控，Web 端隐藏入口。

## 5. 维护约定

- 新增命令必须同时更新本清单（重跑脚本），并在 PR 中说明拆分分类与暴露等级。
- 集成测试沿用既有 `VL_COMMANDS` 式守卫：注册表与清单不一致即失败。
- 本文件由脚本生成；手改会在下次重跑时被覆盖。
