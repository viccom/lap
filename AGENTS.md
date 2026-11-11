# lap 项目说明（viccom/lap fork · 功能演进基线）

> 本仓库是 julyx10/lap v0.3.3 的 fork，在 `develop` 分支上做自有功能演进；`main` 跟随上游。
> 代码事实来自 2026-11-11 对 v0.3.3 源码的深读（file:line 锚点），上游更新后锚点会漂移，以代码为准。

## 分支与同步策略（跟随官方更新的关键）

| 分支/远程 | 用途 |
|---|---|
| `main` = upstream | 镜像官方 julyx10/lap 的 main，**不在上面做任何自有提交** |
| `origin` | github.com/viccom/lap（fork） |
| `develop` | **所有自有开发在这里**；功能分支从 develop 切出，PR/merge 回 develop |

**同步上游**（保持"我们的改动全部在 develop 的独立提交里"是低摩擦合并的前提）：
```bash
git fetch upstream
git checkout main && git merge --ff-only upstream/main && git push origin main
git checkout develop && git rebase main        # 或 merge；冲突只可能出现在自有提交
```
自有改动纪律：**每个功能一个独立小提交系列**，改动集中在新增文件 + 少数标记过的既有文件锚点（见下文「允许触碰的既有文件」），不重排不重构上游代码——rebase 时冲突面就小。

## 红线

- **GPL-3.0**：fork 整体仍受 GPL-3.0-or-later 约束；分发构建物须开源。
- **InsightFace buffalo_s（det_500m/w600k_mbf）是非商业研究许可**（THIRD_PARTY_NOTICES.txt:416）：分发带人脸功能的构建物前必须先处理模型许可（替换或取得授权）；自用无碍。
- 不改动上游代码的既有行为，除非功能方案里明确列为"适配点"并有对应测试保护。

## 上游代码规范（新增代码必须匹配）

从现有代码提炼，贡献前先看相邻文件：

**Rust（src-tauri/src/）**
- 模块按 `t_<域>.rs` 命名平铺；新功能建新文件（如 `t_vl.rs`），不往 `t_cmds.rs`/`t_sqlite.rs` 里堆。
- command 定义在功能模块内（`#[tauri::command]`），在 `main.rs` 的 `generate_handler!` 注册（main.rs:208-454）；按注释分组插入。
- 错误风格：`Result<T, String>`（仓库一致风格，无 thiserror/anyhow）——跟随，不自创错误框架。
- DB：模型与 SQL 用关联函数挂在类型上（`AFile::`/`Album::`），`open_conn()` 取池连接；schema 变更**必须走 `t_migration.rs` 版本化迁移**（当前 v22，新表从 v23 起追加，基于 `user_version`）；连接 PRAGMA 已设 WAL/NORMAL/foreign_keys。
- 后台任务模式：`State` 持 `is_scanning/cancel_flag(AtomicBool) + status(Mutex<...>)`，进度走 `app.emit`（参照 t_dedup.rs:77-120、t_similar.rs:45-49）。
- 并发纪律：同相册扫描单实例（`AlbumScanGuard`）；动相册 DB 前 `AlbumRemovalGuard` + 等扫描结束；库级操作持 `FILE_REFRESH_LIBRARY_LOCK`。
- 单元测试：同文件 `#[cfg(test)] mod tests`（仓库 334 个测试皆此风格）；新逻辑必须带测试。
- 托管 State 在 `main.rs` `.manage()` 注册；进程级配置锁参照 `CONFIG_IO_LOCK`。

**前端（src-vite/）**
- invoke 调用统一封装进 `common/api.js`（2,271 行主封装层）；新命令按域追加 export 函数。
- 状态：`configStore`（全局设置，persist 到 localStorage）/ `libraryStore`（每库 UI 状态，经 `saveLibraryState` 存后端）；新设置键进 configStore 的 `settings` 对象并给默认值。
- UI：Tailwind + daisyUI 类名；文案**必须**走 i18n——17 个 locale 文件同步加 key（至少 en + zh-CN，其余可后补但 key 要先占位）；组件放 `components/`，独立全屏功能（如拼贴）用 WebviewWindow 新窗口 + router 新路由。
- 测试：`tests/*.test.mjs`（node --test）测纯逻辑模块；可测逻辑抽成独立模块再测，不测 .vue 单文件。跑法：`npm run test:<域>`，按 package.json 追加 script。

**i18n / 构建 / 依赖**
- 支持格式/扩展名常量在 `t_common.rs`；构建脚本 build.rs 负责第三方 C++ 静态编译，不轻易动。
- 新依赖需说明理由并锁版本（Cargo.toml + package.json 现状即范例）。

## 架构速查（改哪先读哪）

- **两级模型**：`AppConfig.libraries`（每库独立 `{db_dir}/{library_id}.db`）→ 库内 `albums`（相册 = folder_path，唯一性/防重叠是应用层校验 t_sqlite.rs:407-432）。
- **连接池**（非单写连接）：`CONN_POOL` + WAL（t_sqlite.rs:10138, 10234-10246）。
- **索引流水线** `index_album_worker`（t_utils.rs:4145 起）：并发预算 `cores*0.7`、embedding 闸门 `Semaphore(1)`；**mark-and-sweep 只在 scan_complete**（取消/失败不删记录，t_utils.rs:4454-4524）。
- **AI 推理全程 CPU**：`ort` 未配置任何执行提供者；向量 512 维 f32 LE 存 `afiles.embeds`；维度不匹配静默 0 分（t_sqlite.rs:7366-7374）。
- **搜索入口**：统一搜索 = 文件名/EXIF LIKE（`get_query_files`，t_sqlite.rs:5339-5355）∥ 向量搜索（`search_similar_images`，全表扫描打分）并行，编排在前端 Content.vue:8051-8145 与 8303-8581。
- **云文件保护**：`is_cloud_only` 检查贯穿哈希/嵌入/缩略图/XMP 每条路径，新功能触碰文件内容前必须沿用（t_utils.rs:483-500）。
- 巨石文件警示：`t_sqlite.rs` 11,726 行、`Content.vue` 439KB、`t_cmds.rs` 4,277 行——新代码放新文件，尽量不增大它们。

## 本地环境

- 程序 `D:\Lap`（安装版）；开发库数据 `C:\Users\viccom\AppData\Local\com.julyx10.lap\`（debug 构建带 `.debug` 后缀，与安装版数据隔离）。
- 构建：Node 20+/pnpm/Rust stable/tauri-cli ^2/cmake/nasm；`git submodule update --init --recursive` → `scripts/download_models.ps1` → `cd src-vite && pnpm install` → `cargo tauri dev`。
- 测试基线（改动前后都要绿）：`cargo test`（src-tauri 内，334 个）+ `cd src-vite && npm run test:*`（37 个测试文件，按域脚本）。
- 调试手法：Python `sqlite3` 只读查库（`LENGTH(embeds)/4` 看向量维度等）。

## 自有功能规划（详见 docs/dev/F1-图片理解技术方案.md 等）

- **F1 图片理解**：云端 VL（走局域网 vl-gateway）+ 不可变提示词模板 + 带溯源标注存储。
- **F2 搜索扩展**：FTS5 精确档（中文双字切分）+ 语义档强化，与现有搜索并行。
- **F3 多图叙事**：多图 + 模板 → 云端讲故事，依赖 F1 基建。
