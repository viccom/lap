# lap 数据与存储机制

> 2026-11-13 整理。基于 develop 分支（库 schema v27）源码与 debug 库实测；所有论断均给出代码出处或实测证据。
> 阅读对象：在 lap 上做演进开发的人。目标：回答「数据放在哪、跟着谁走、什么操作会破坏什么」。

## 1. 身份链：一切机制的核心

```
library（每个库一个独立 default.db，按安装 identifier 隔离）
  └─ album（一个根路径，albums.path）
       └─ afolder（子目录，按 path 归属，可整目录排除搜索）
            └─ afile（文件行，唯一键 = folder_id + name）   ← 身份终点
```

- **文件身份 = （文件夹， 文件名），不是内容哈希**：`uidx_afiles_folder_id_name ON afiles(folder_id, name)`（`src-tauri/src/t_sqlite.rs`）。路径变 = 身份变，与内容是否相同无关。
- album 添加只拦两种情况：完全相同路径、与现有 album 嵌套/互相包含——*"A folder can only belong to one album tree"*（`Album::add_album_to_db`）。
- **没有 relink**：`edit_album` 只能改名称/描述/文件类型/小图过滤/排除目录，**不能改 path**。改路径 = 删旧 album + 加新 album。
- rating、收藏等主观数据存放在 afiles 行上（即跟随路径身份，不跟随内容）。

## 2. 数据四层模型（按「跟随什么」分类）

| 层 | 跟随对象 | 内容 | 身份一换的下场 |
|---|---|---|---|
| 目录层 | file_id | afiles 行本身、rating、收藏、taken_date | 新行从零开始 |
| 衍生层 | file_id（FK，多为 ON DELETE CASCADE） | VL 标注 `vl_annotations`、标签关联 `afile_tags`、语义向量 `caption_embeds`、人脸归属、FTS 文档 `files_fts`、去重哈希 `file_hashes` | **全部不跟随**，需重做 |
| 缓存层 | blake3 key + album_id | 缩略图缓存文件、video_cache | key 含 file_id → 新 key 新缓存 |
| 随文件层 | 文件系统 | XMP sidecar（手动「保存元数据到 XMP」落盘的内容） | **唯一跟着文件搬家走的层** |

关键表结构（debug 库实测 `PRAGMA table_info`）：

- `vl_annotations(id, file_id, template_id, template_version, model, result_json, raw_text, call_id, created_at)` — 图片理解结果，一个文件可多条（按模板版本）。
- `vl_runs(run_id, library_id, state, total, processed, succeeded, failed, skipped, ...)` — 任务队列持久化（重启后「VL 任务」页可查，遗留 running/queued 自动标 interrupted）。
- `vl_calls(id, file_id, ..., raw_response, thinking_text, input_tokens, output_tokens, status, error, duration_ms, run_id)` — 逐文件调用明细（status 取值 `ok`/`error`）。
- `caption_embeds(file_id, model, dims, embeds, updated_at)` — 语义向量；`model` 列区分向量空间（本地源向量名带模型文件大小指纹，换模型自动重嵌）。
- `athumbs(file_id UNIQUE, error_code, thumb_data BLOB, thumb_key, thumb_mtime, thumb_size)` — 缩略图行；实际像素可能在 BLOB，也可能只有 `thumb_key` 指向缓存文件（见 §2.1）。
- `files_fts`（FTS5）：列 `name/people/meta/tags/caption` + `aux UNINDEXED`；文档以 rowid=file_id 关联。
- `axmp_local_tags_{INSERT,...}`（`metadata/xmp.rs`）：**触发器**维护的**导入保护位**——标签关联变更（含 AI 标签写入）时把该文件 `axmp_imports.dirty |= Tags位` 置位，此后非破坏性重扫不再用 sidecar 覆盖库内标签。注意它**不是导出队列**：导出（手动「保存元数据到 XMP」）时直接读当前标签全量，不消费也不清该位（2026-11-14 后端审查纠正了本文早前"待导出队列"的错误表述）。**2026-11-14 已修复**：AI 标签写入会在同一事务清除 Tags 位（机器写入不借用用户编辑保护，见 REVIEW-2026-11-14 P0-4）；XMP 导出同时排除「AI理解」子树（P0-3）。

### 2.1 缩略图缓存

- key = `blake3("lap-thumb-v1" + library_id + file_id + 尺寸 + orientation + 源 mtime [+ raw 选项])`（`t_sqlite.rs build_thumb_key`）——同文件 mtime 变了也重生成；移动后新 file_id = 新 key。
- 缓存文件路径：`<cache_root>/<library_id>/<album_id>/<key[0:2]>/<key>.<ext>`；`cache_root` 默认在 appdata 下按库 id 分目录（debug 库实测：`com.julyx10.lap.debug\default\`），可在 app-config 的 `thumbnail_cache_dir` 自定义（`t_cache_storage::root()`，自定义位置离线时会明确报错）。
- debug 库形态：`athumbs.thumb_data` 全为 NULL，像素一律在缓存文件（**只读 BLOB 会在源盘离线时成片失败**——2026-11-14 已修：`AThumb::image_bytes_on` blob 优先、缓存文件兜底）。
- lap 内部移动文件时会 `relocate_for_thumb_keys(old_album_id, new_album_id)` 把缓存条目迁到新相册目录。

## 3. 离线语义：路径 A 不存在了

**结论：降级可用——不崩溃、数据不丢、需原图的操作被明确拦截。**

| 层面 | 行为 | 出处 |
|---|---|---|
| 数据库 | **绝不自动删行**（扫描路径无 `DELETE FROM afiles`）；文件行、标注、标签、向量全保留 | `t_utils.rs` 扫描管道 |
| 扫描防污染 | 只有**完整遍历**（`scan_complete`）才更新相册计数/扫描时间——离线盘的残缺遍历不覆盖既有总量 | `t_utils.rs` *"Recount only after a complete traversal so an offline disk cannot replace the existing album totals"* |
| 可达性探测 | 专用命令 `check_album_accessibility`（网络探测不持库锁、代际号防跨库串扰） | `t_cmds.rs` |
| 前端感知 | `availability.ts` 三级缓存（album/folder/path），`isOriginalUnavailable()` 屏蔽需原图的操作 | `src-vite/src/common/availability.ts` |
| 后端硬闸 | 导出/写 XMP 等直接报 `original_unavailable: <path>` | `t_cmds.rs` |
| 浏览/搜索 | 缩略图走缓存文件照常渲染（实测 F: 离线 404/404 张全出）；FTS/语义搜索查库与缓存向量，零原图依赖 | 本项目 2026-11-14 实测 |
| 手动操作缺失路径 | 报 `Path does not exist: <path>` | `t_utils.rs build_nodes` |
| 路径恢复 | 一切自动恢复，增量扫描继续 | 行未删 |

## 4. OS 层搬家再添加（A 全部文件移到 B，B 添加为新 album）

**结论：不会崩，但发生「身份断裂」——同一张照片变成两条互不相干的记录，A 上积累的成果全部不跟随。**

推演链（全部由 §1 身份链直接推出）：

1. B 顺利入库（不同路径且不与 A 重叠）；扫描生成**全新 file id**。
2. 「全部照片」出现双份：A 的旧行（进入 §3 离线态，缩略图仍从旧缓存渲染）+ B 的新行；侧栏同时挂 A、B 两个入口。
3. 衍生层全部不跟随（§2 表）：VL 标注、`AI理解` 标签、语义向量、人脸归属、rating/收藏——B 行从零开始，需重跑理解（force 重跑可重建标注/标签/向量）。
4. **唯一例外**：随文件搬走的 XMP sidecar 会在扫描时重新导入——之前手动保存过 XMP 的描述/评分/标签回到 B 行。VL 标注与向量是纯库内数据，永不随文件走。
5. 去重视图按内容哈希（`file_hashes`）会把 A/B 成对列出，可借它人工清理，但**不会自动合并**。

**正确姿势**：搬家前用 lap 内部移动/重命名（身份原地更新 + 缓存迁移，一切保留）；已经搬完的补救 = 删 A 相册（FK 级联清行与标签关联，废弃缓存由 `clean_unused_thumbnail_cache` 回收）+ B 重新理解。

## 5. 模型加载：appdata 优先、resource 兜底（2026-11-13 实施）

- 两类位置：**resource**（exe 旁 `models/`，随包分发的只读资产）与 **appdata**（`<identifier>/models/`，运行时下载的可写落点，按 identifier 隔离 debug/安装版）。
- 随包五件套（resource，官方核心）：`vision_model.onnx` + `text_model.onnx` + `tokenizer.json`（CLIP：以图搜图 + 文本搜图默认档，`t_ai.rs`）、`det_500m.onnx` + `w600k_mbf.onnx`（InsightFace：人脸检测/识别，`t_face.rs`）。
- 可选模型（双位置解析）：`bge-local`（语义搜索本地嵌入源，`t_embed.rs`）与 `multilingual`（多语言文本搜图，`t_ai.rs`）——**appdata 副本（需 model+tokenizer 齐）优先，resource 副本兜底；下载/安装目标永远 appdata**。核心函数 `resolve_local_model_dir` / `resolve_multilingual_model_dir`（纯函数 + 单测）；resource 目录在 `finish_startup` 用 Tauri resource 目录钉住（`t_config::get_resource_models_dir`），钉住前以 exe 旁目录兜底。
- 已知怪癖：debug 构建里 CLIP 的 `resource_model_dir` 有 dev-checkout 抢先分支（`CARGO_MANIFEST_DIR/resources/models` 存在时优先）——便携 debug exe 在有源码树的机器上，CLIP 从开发目录加载；人脸模型无此分支，始终读 exe 旁。

## 6. 身份模型的边界与演进（分析结论，待拍板）

**问题**：改用「文件内容 hash 作唯一 ID」（homepix 模式）对 lap 破坏多大？——**很大，且破坏点在归属模型而非 ID 算法**：

- lap 的产品形态是多库 + 相册树归属 + 同内容多副本合法存在（原片/精选分目录）；hash 主键需引入 `file_locations` 多对多表，rating/收藏从「每路径一份」变「每内容一份」（语义变更），删除级联链全部重定义。
- 增量扫描靠目录 mtime 快照加速，hash 识别「同内容新路径」必须读文件内容（TB 级库一次重 IO）；计数/时间线/排除搜索查询从单跳 join 变双跳。
- 对 fork 维护是结构性破坏：afiles 是所有 t_* 模块交汇点，重写即与上游永久分叉。

**中间路线（推荐，未实施）——hash 当认领线索、不当身份**：扫描新文件时算 hash 查 `file_hashes`（dedup 已有表），若同 hash 旧行存在且其路径已失效 → relink：改旧行 `folder_id/name` + `relocate_for_thumb_keys` 迁缓存，衍生数据原地保留。改动面天级，不动身份模型，正好补上「文件系统层搬家」这一个缺口。

## 7. appdata 文件清单

| 路径（`%LOCALAPPDATA%` 下） | 内容 | 说明 |
|---|---|---|
| `com.julyx10.lap\` | 安装版（release，identifier 无后缀） | |
| ├ `app-config.json` | 应用配置（含 VL 网关/嵌入源/并发等） | **api_key 只写不回读** |
| ├ `libraries\default.db` | 真实图库（本机实测仍为 schema v18） | 迁移前必须备份 |
| ├ `default\` | 库 id 命名的缩略图缓存根 | 可删，重扫重建 |
| ├ `models\` | 运行时下载的模型（multilingual、bge-local） | §5 |
| ├ `EBWebView\` | WebView2 用户数据目录 | **release/debug 共用**（按 identifier 派生） |
| ├ `video_cache\` | 视频缓存 | 可删 |
| └ `vl-api-key` | VL 网关密钥 sidecar（`t_config.rs`） | 明文本机，不进库不进配置 |
| `com.julyx10.lap.debug\` | debug 构建（便携版用），结构与上同 | 数据与安装版完全隔离 |

## 8. 一张照片的生命周期走查

| 步骤 | 落点 |
|---|---|
| 添加相册并扫描 | `albums`/`afolders` 行 + `afiles` 行（EXIF/元数据入行，XMP sidecar 此时导入） |
| 生成缩略图 | `athumbs` 行（key/错误码）+ 缓存文件 `<cache>/<lib>/<album>/<key[0:2]>/<key>.jpg` |
| AI 理解（VL） | `vl_runs`/`vl_calls`（原始返回、token、耗时）→ `vl_annotations`（result_json 按模板版本） |
| 标签回写 | `atags` 子树「AI理解」+ `afile_tags` 关联；同时触发 `axmp_local_tags_INSERT` 置导入保护位（见 §2 说明） |
| 语义向量 | `caption_embeds(file_id, model='bge-local-<指纹>', embeds)` |
| 全文索引 | `files_fts` 文档（name/people/meta/tags/caption） |
| 保存元数据到 XMP | sidecar `.xmp` 写到**源文件旁**（唯一随文件走的层） |

每一步都只挂 file_id——这就是 §4 身份断裂的机制根源。

## 9. 灾难恢复矩阵

| 丢失物 | 丢什么 | 恢复方式 |
|---|---|---|
| 缩略图缓存（`default\`、video_cache） | 仅缓存 | 自动重建（按 mtime/key 重生成） |
| `models\`（appdata） | 可选模型 | multilingual 设置页重新下载；bge-local 拷回或用包内 resource 副本兜底（§5） |
| `vl-api-key` | 网关密钥 | 设置页重新填写（另见环境变量 `LAP_VL_API_KEY`） |
| `app-config.json` | 配置 | 重建默认；网关/模型选择需重配（api_key 本就不在其中） |
| `libraries/default.db` | **目录层+全部衍生层**（标注/标签/向量/人脸/收藏） | 无自愈；只能靠事先备份。**备份必须用 sqlite backup API**（活跃库裸拷贝会漏 WAL 内容，实测踩过：`docs/dev/seed-debug-data.py` 即此模式） |
| XMP sidecar | 已导出的描述/评分/标签 | 重新扫描自动导回（唯一能从文件系统恢复的衍生数据） |
| 缩略图 BLOB 与缓存文件双缺 | 缩略图 | 需源文件在线重生成（离线盘上该文件无图） |

## 10. 代码索引（写作时核对过的出处）

- 身份与 schema：`src-tauri/src/t_sqlite.rs`（afiles/athumbs schema、`build_thumb_key`、`image_bytes_on`、`relocate_for_thumb_keys`）
- 扫描与离线：`src-tauri/src/t_utils.rs`（`build_nodes`、scan_complete 防污染、相册可达性）
- 相册与可达性命令：`src-tauri/src/t_cmds.rs`（`add_album`/`edit_album`/`check_album_accessibility`/`original_unavailable` 硬闸）
- VL/向量/标签：`src-tauri/src/t_vl.rs`、`t_embed.rs`、`t_vl_tags.rs`、`t_tags.rs`
- XMP：`src-tauri/src/metadata/xmp.rs`（sidecar 导入、`axmp_local_tags_*` 触发器）
- 配置与密钥：`src-tauri/src/t_config.rs`（app-data 目录、`vl-api-key` sidecar、resource models 目录）
- 前端可达性：`src-vite/src/common/availability.ts`
- 模型资产下载契约：`scripts/download_models.sh`（SHA-256 校验）
