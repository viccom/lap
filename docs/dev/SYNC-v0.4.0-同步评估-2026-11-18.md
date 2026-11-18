# lappp ← julyx10/lap v0.4.0 同步评估（2026-11-18）

> 结论先行：**推荐 merge 方式同步**（非 rebase），工作量约 1~2 个全天（AI 辅助 3~5 小时会话）；
> 两个最高风险点是 **SQLite 迁移版本撞号（v23/v24，同号不同义，git 不提示的语义冲突）** 与
> **14 个被挪进 `crates/lappp-core/` 的上游改动文件需手工移植**。动手前必须备份测试库。

## 1. 背景

- 上游 [julyx10/lap v0.4.0](https://github.com/julyx10/lap/releases/tag/v0.4.0)（2026-11-17 发布）。
- fork 基线：本地 `main` = `7d982d1e`（= v0.3.3 + 2 个 i18n 提交）；`develop` 在其上有 **124 个自有提交**（阶段 0~7：服务化拆分 + F1 图片理解 / F2 搜索扩展 / F3 多图叙事 + 薄客户端 + 重命名 lappp）。
- 同步目标：吸收 v0.4.0 全部修复与特性，保持 fork 四个自有演进成果与 lappp/lappp-server 架构不变。

## 2. 量化的冲突面（证据）

| 维度 | 数据 | 严重度 |
|---|---|---|
| 上游增量 | 32 commits / 153 files / +8960−4078 | — |
| fork 自有 | 124 commits / 305 files | — |
| 文件重叠 | 58（17 个 locale 机械合并 + 41 个实质重叠） | 高 |
| Rust 挪移 | 上游改动的 14 个文件已被 fork 挪进 `crates/lappp-core/`：`t_migration` `t_sqlite` `t_tags` `t_dedup` `t_similar` `t_image` `t_common` `t_utils` `t_affinity` + `metadata/{files,merge,schema,xmp,apple_sidecar}` → 全部 modify/delete 冲突 | **最高** |
| 命令面工序 | 上游新命令需过 4 道工序：lappp-core 实现 → 桌面 `#[tauri::command]` 壳（`src-tauri/src/t_cmds.rs`）→ `main.rs generate_handler` → lappp-server HTTP 路由/白名单 | 高 |
| `Content.vue` | 上游 +766/−391 vs fork +1089/−324，双边大改 | 最痛手工点 |
| `main.rs` | 上游仅 +6/−3 | 低 |
| 新依赖 | http-range / tauri-utils / percent-encoding（轻量，无新外部二进制） | 低 |
| **迁移撞号** | 上游 v23 = "Record tag creation source"（空 SQL 行为迁移）、v24 = `idx_afolders_excluded_album` 索引（IF NOT EXISTS）；fork v23 = VL_SCHEMA、v24 = FTS_SCHEMA、v25 = 字符分词重建、v26 = 嵌入向量表、v27 = VL runs 持久化、v28 = VL 任务队列 | **最高（静默坏库风险）** |

## 3. 方案裁决

| 方案 | 评估 | 结论 |
|---|---|---|
| A. rebase（AGENTS 默认） | 124 提交逐个重放；14 个挪移文件的冲突在阶段 4 拆分系列的每个相关提交重复爆发 | **否决** |
| B. `git merge main` → develop | 冲突一次暴露；rename 检测对高相似度文件（t_migration 主体等）可自动映射到 crates/ 新路径；`t_cmds.rs` 已被 fork 重写为壳、必然手工；AGENTS 明示"或 merge" | **采用** |
| C. v0.4.0 重建基线重放功能 | 最干净但等于重做集成 | 备选（仅当 merge 后基线烂掉） |

## 4. 必做专项修复

1. **迁移撞号处理**（独立提交，最高优先；2026-11-18 二次评估后方案简化）：
   - 前提证据（已逐项验证）：
     - 上游 v23 = 给 `atags` 加 `source` 列，**自带 `table_has_column` 守卫**（幂等）；v24 = `IF NOT EXISTS` 索引（幂等）。
     - fork 侧全部幂等：VL_SCHEMA_SQL 5 个 CREATE 全带 `IF NOT EXISTS`（5/5）；FTS `CREATE VIRTUAL TABLE IF NOT EXISTS`；embed `caption_embeds` 带 `IF NOT EXISTS`；v25/v27/v28 为空 SQL 数组项（runner 无副作用，仅版本前进）。
     - 上游 runner 对高版本库**跳过**低版本迁移（`user_version=28 > 24` → 上游 v23/v24 不会在存量 lappp 库执行）。
   - 方案：fork v23~28 **重编号为 v25~30**；新增 **v31 幂等补偿迁移**（重放上游 v23 加列守卫逻辑 + v24 索引 SQL）；fork runner 的 `version == N` 行为分支链同步对齐编号。
   - 测试先行：Rust 单测构造"fork v28 存量库"→ 跑迁移 → 断言 `user_version=31`、VL/FTS/embed 表齐全、`atags.source` 存在、上游索引存在。
2. 上游"存量库一次性元数据重读"在同步后首启触发（`metadata_pending`，测试库 404 张无碍，真实库需预期）。
3. fork 的 FileInfo「图片理解」折叠组需人工挂进上游**重建后的** File Info 面板结构（独立提交，可单独 revert）。
4. 17 个 locale：脚本辅助取两边 key 并集（上游补了越南语/捷克语缺失键）。
5. 同步后更新 `lap/AGENTS.md` 基线段（上游迁移版本 v22→v24、测试数、同步日期）。

## 5. 执行计划（分步，每步可验证）

1. 预备：`git branch develop-pre-v040` 保护分支；备份 `%LOCALAPPDATA%\com.julyx10.lap.debug`；开启 rerere。
2. `main` ff 至 `upstream/main`（v0.4.0）并推 origin。
3. `develop` 上 `git merge main`：先解 locales/package.json 等机械冲突，再逐个处理 41 个实质文件。
4. Rust 移植：14 个挪移文件的上游改动逐一移植到 `crates/lappp-core/src/` 对应文件；新命令过 4 道工序。
5. 前端热点手工合并：`Content.vue`、`FileInfo.vue`、`Settings.vue`、`configStore.js`、`main.js`（注意保留 SW 修复 9b48f8e7）。
6. 迁移撞号修复（§4.1），用备份的测试库验证升级路径。
7. 回归门禁：全量 `npm run test:*`（17 域）+ `cargo test` + `build-windows.ps1` 三产物 + **lappp.exe 真机启动冒烟**（SW 事故教训：构建≠能跑）+ lappp-server E2E。
8. 文档：更新 AGENTS 基线；本文件补"执行结果"一节。

## 6. 深度风险矩阵与最小化方案（二次评估，2026-11-18）

> 二次评估新增证据：上游迁移 runner 的行为分支带内建守卫（v23 `table_has_column`）、
> fork 全部自有迁移幂等（IF NOT EXISTS 全覆盖）、上游新命令仅 5 个、
> 上游 `main.js` 仅 +9/−8 且与 fork SW 修复不同区域。

| # | 风险 | 概率×影响 | 根因证据 | 最小化手段 | 探测手段 | 回滚 |
|---|---|---|---|---|---|---|
| R1 | 迁移撞号静默坏库 | 低×致命 | 上游 runner 按版本硬编码分支；存量库 user_version=28 跳过上游 v23/v24 | 重编号 v25~30 + v31 幂等补偿迁移；**Rust 单测先红后绿**（构造 v28 库升级断言）；隔离库演练（`--data-dir` 指向备份副本，绝不碰真实库） | user_version/表/索引三重断言 | 迁移独立 commit 可单独 revert；演练用副本 |
| R2 | 14 个挪移文件移植错漏（合计 +2310/−778，大头 t_utils +820、t_dedup +336、t_sqlite +312） | 中×高 | fork 搬家后上游原地改 | 文件级策略：**取上游新版整体搬入 crates/ + 回放 fork 适配点**（fork 对这些文件的改动多为搬家适配，量小）；每文件移植后立即 `cargo check` + 该模块单测；merge 加 `-Xfind-renames=40%` 促自动映射 | cargo test 全绿 + 契约守卫测试 | scratch 分支；单文件独立 commit |
| R3 | 新命令 4 道工序漏注册 | 中×中 | 5 个新命令（reconcile_raw_jpeg_pairs / refresh_album_folders / import_files / edit_file_metadata / dedup_get_group_items）× 4 落点（core/桌面壳/generate_handler/server HTTP） | 5×4 checklist 固化；为 5 命令写 HTTP 可达性断言测试（lappp-server 契约测试体系已有） | 断言测试红即漏 | 单命令独立小 commit |
| R4 | Content.vue/FileInfo 融合遗漏上游行为 | 中×中 | Content.vue 双边大改（上游 +766/−391 配对/混合状态/性能 vs fork +1089/−324 VL 角标/AI 右键/AI 搜索） | **按主题分块合并**（非逐 hunk）；FileInfo 图片理解组独立 commit 重挂；上游 7 个新测试文件（rawDisplay 等）作行为锚 | fork E2E 清单 + 上游新测试双向回归 | 主题块独立 commit 可 revert |
| R5 | SW 修复（9b48f8e7）被合并冲掉 | 低×高 | main.js 双边改动，但上游仅 +9/−8（fileDeleteBehavior 监听）且不同区域 | 合并后必查项：`git show 9b48f8e7 -- src-vite/src/main.js` 核心三行仍在 | 隔离 data-dir 真机冒烟（构建≠能跑，SW 事故教训） | revert merge |
| R6 | Cargo.lock / package.json 冲突 | 高×低 | 双边都改 | 锁文件**不手并**：取任意侧后 `cargo build` 重生成；package.json 核对 scripts 并集 | build 成功 + 测试跑通 | 无需 |
| R7 | 测试基线漂移 | 高×低 | 上游改 7 个测试文件 + fork 加大量测试 | 合并前后全量测试数字对照，写入 BASELINE 文档 | 数字对照表 | 无需 |
| R8 | 流程风险（真库被新 exe 污染） | 低×致命 | SW 事故同源 | 所有冒烟走 `--data-dir` 隔离目录；LapPortable 部署放最后一步 | — | 备份 + 保护分支 |

### 执行策略：四阶段门禁（develop 全绿前不动）

- **P0 预备**：`develop-pre-v040` 保护分支；备份测试库；`rerere` 开启；5×4 命令 checklist 固化。
- **P1 基线验证**：`main` ff → v0.4.0 并推 origin；在 main 上独立 `cargo build` + 测试，确认工具链可编译上游（Windows CI 修复的本地验证）。
- **P2 scratch 分支（develop-sync-v4）merge**：分层解冲突并逐层过门禁——机械层（locales 脚本并集/锁文件重生成）→ Rust 层（14 文件逐个，每文件 cargo check+模块测试）→ 命令层（5×4）→ 前端热点（按主题分块）。
- **P3 迁移专项**：重编号 + v31 补偿，Rust 单测先行（红→绿）。
- **P4 终局门禁**：全量 `test:*` + `cargo test` + `build-windows.ps1` 三产物 + 隔离库升级演练（fresh / v28 副本两路）+ 隔离 data-dir 真机冒烟 → 全绿才 `develop` fast-forward 合入 → push。

## 7. 工作量与风险

- 构成：Rust 移植 ~40%、前端热点 ~30%、回归验证 ~15%、迁移设计 ~10%、locales ~5%。
- 风险排序：迁移语义冲突（静默坏库）> Content.vue/FileInfo 合并遗漏上游行为 > 命令面 4 道工序漏注册 > 上游首启全库重读的耗时惊讶。
- 许可不变：GPL-3.0；InsightFace 非商业限制照旧；updater 仍指上游、不启用自动更新。

## 附录 A：32 个上游提交分类

**feat（10）**：RAW 配对分组浏览/文件操作（#469）、配对媒体单条目化、配对删除范围可选（#555）、目录字段可编辑+写回 XMP、标签来源记录+导入 sidecar 保留、RAW 预览控制/快捷键、Affinity 文档预览（#543）、手动刷新同步文件夹内文件、文件夹操作移入面板菜单、混合照片状态与快捷键提示（#545）。

**perf（3）**：搜索排除按相册建索引（迁移 24，#507）、大库文件夹搜索/网格布局/滚动条提速（#548）、网格冗余布局刷新消除。

**fix（15）**：RAW 配对导航与去重选择保持、配对伴随计入去重审查统计、已删标签防重导入（#541）、删标签后选择/滚动位置保持（#542）、侧栏计数随文件/目录变更刷新（#533）、列表变更时未加载行的选择保持（#532）、云文件防自动下载、智能相册标签路径悬停显示（#536）、禁用文本对比度（#540）、设置/标签面板交互打磨、选择与面板提示简化、缺失翻译键、Windows CI（Cargo 1.100）、文档同步、RAW 配对导航补充修复。

**refactor（2）**：预览与选择动作流线化、文件元数据组织+视图旋转。

**chore（1）**：release v0.4.0（版本号 0.3.3→0.4.0）。

> 注：GitHub release notes 覆盖窗口大于本 delta（部分条目如 #487/#499 已含于 v0.3.3 基线）；
> 以本附录 32 提交为同步的精确依据。
