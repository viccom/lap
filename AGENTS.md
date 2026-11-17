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

### 前端测试基线（2026-11-15 阶段 0 修复后全绿）

**41 个测试文件 / 453 tests 全部通过**（Rust 同批全绿：647/0/10 ignored；证据与复现命令见 [docs/dev/BASELINE-阶段0-服务化拆分基线.md](docs/dev/BASELINE-阶段0-服务化拆分基线.md)）。历史上的「8 个失败文件」已在阶段 0 全部根因修复，真实根因与 F0 初判不同：

| 真实根因 | 影响面 | 修复 |
|---|---|---|
| **CRLF 工作区 vs LF 锚定正则**：本机 autocrlf=true，import 剥离正则以 `\n` 结尾全部失配 → `new Function` SyntaxError | 7 文件 27 测试（afAreaPersistence / albumRecovery / cloudFileRefresh / offlineCatalog / xmpWriteback / fileInfoMetadata / filterSelection 的加载与内部失败） | 读取处统一 `.replace(/\r\n/g, '\n')` |
| 菜单位置索引漂移（`menu.value[2]`，vl-analyze-selected 挤位） | offlineCatalog 1 项 | 按子项 actionName 反查分组 |
| 切片锚点巧合被 f707fc68 打破（注释移位 → query-context watcher 掉出装置；产品无回归） | filterSelection 3 项 | 锚点改为意图明确的注释对 |

**新测试必须遵守**：①读源码文件必须归一化 CRLF；②锚点不得依赖跨行 `\n` 形态、位置索引、注释挂位巧合；③TS 源码避免 `new Function(ts.transpile(...))` 模式（能 import 模块就直接 import，或用 `vm.SourceTextModule`）；④涉及文件系统时间戳的测试注入变更前 `sleep ≥25ms`（Windows ~15.6ms tick 合并，t_storage/metadata 的两个 transfer_batch 测试因此修过）。

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
- **本机编译环境要点（F0 实测）**：
  - MSVC BuildTools 2022 + Windows SDK 已装；**cmake 用 VS 自带的那份**（`Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin`，不在全局 PATH）；**nasm 需另装**（scoop `nasm`，libjpeg-turbo 需要）；`rc.exe` 在 Windows Kits `10.0.26100.0/x64`。
  - **cargo 走 Clash 代理必须关 multiplexing**：`CARGO_HTTP_MULTIPLEXING=false CARGO_HTTP_VERSION=HTTP/1.1`，否则 sparse index 挂起（进程有连接、CPU≈0、crate 缓存 0 文件）。
  - 模型可直接从安装版 `D:\Lap\models` 复制到 `src-tauri/resources/models`（脚本会校验 SHA-256，5 个文件全部一致）。
- 测试基线（改动前后都要对照，失败集合不得新增）：`cargo test --workspace`（src-tauri 内，**652 过/0 败/10 ignored**：lap-core 573 + Lap 壳 9 + lapd 66 + drag vendor 2+2；阶段 4 批次 4.6 限流拆分 + 批次 4.10「embed_status 降级 + lapd 资源根与桌面一致」测试计入）+ `cd src-vite && node --experimental-vm-modules --test tests/*.test.mjs`（41 文件 **462** 测试全绿；阶段 4 传输层/签名/门控 + 批次 4.5 `afterNextPaint` + 批次 4.6 限流状态 + 批次 4.8「二级页退出入口」+ 批次 4.9「跨页会话/视频按 id/守卫补动态 import」+ 批次 4.10「嵌入状态行缺模型提示 / Web 路径复制 / Web 隐藏 native-only 菜单入口」测试计入；跑 `node --experimental-vm-modules --test tests/`（含 2 个 `.ts` 测试文件）为 488 全绿；基线详情见上方「前端测试基线」）。前端 Web 构建：`cd src-vite && npm run build:http`（`VITE_LAP_TRANSPORT=http`，同源托管于 lapd `--web-dir`）。**`dist/` 是两个构建共用的 outDir**：`npm run build`（桌面）与 `build:http`（Web）互相覆盖，交给 lapd 托管前必须最后跑一次 `build:http`。
- 调试手法：Python `sqlite3` 只读查库（`LENGTH(embeds)/4` 看向量维度等）。

## 自有功能规划（详见 docs/dev/F1-图片理解技术方案.md 等）

- **F1 图片理解**（✅ 已实现并通过真实网关验收）：云端 VL（走局域网 magpie 网关，**Anthropic Messages 协议**）+ 不可变提示词模板（prompt 与 system 分离）+ 带溯源标注存储。
  - 后端：`src-tauri/src/t_vl.rs`（模型/客户端/编排/10 个 command）+ 迁移 **v23**（`vl_templates`/`vl_annotations`/`vl_calls`）+ `t_config.rs` 的 `VlConfig`。
  - 前端：`src-vite/src/common/vlApi.js`（薄封装）+ `vlLogic.js`（纯逻辑，可测）+ `components/VlTemplateManager.vue`（设置页模板管理）+ Settings 新标签页。
  - **入口模型（2026-11-11 起）**：F1/F2 的功能界面集中在**独立实验台窗口** `views/VlLab.vue`（路由 `/vl-lab`，独立 WebviewWindow，capabilities 已授权该窗口 label）。唯一入口 = 设置 → 图片理解 → 「打开实验台」。FileInfo.vue / ImageSearch.vue 的嵌入挂点已撤（与上游零 diff）；`VlPanel.vue` 已于 2026-11-14 删除（零引用死代码，git 历史即素材）；`searchMode` 三档逻辑保留在 Content.vue 但无切换 UI（恒为默认 all，与上游行为一致）。**验证通过后再谈融合回原有页面**。
  - 密钥：`api_key` 存 **sidecar 文件** `<appdata>/vl-api-key`，`#[serde(skip_serializing)]` 保证 **webview 永远拿不到**；错误信息经 `redact()` 脱敏。
  - 手动验收工具（`#[ignore]`，需真实网关）：`cargo test -- --ignored real_gateway_probe --nocapture`（直连一张本地图）、`real_library_e2e`（走真实库 → 缩略图 → 网关 → 标注入库）。
  - 实测：**缩略图路径 5.3 秒，原图路径 95 秒**（vision token 差一个量级）——多图场景必须走缩略图。
- **F2 搜索扩展**（✅ 已实现并验收）：FTS5 精确档（**中文双字切分**，自研 `tokenize`，无新依赖）+ 三档搜索（全部/精确/语义，默认 all=现状）。
  - 后端：`src-tauri/src/t_fts.rs` + 迁移 **v24**（`files_fts`，rowid=file_id，自动回填）+ **20 处写入钩子**（「写入即检索」；完整核对表在 `docs/dev/F2-搜索扩展技术方案.md` §7.2——**rebase 上游时按它逐一复核**）。
  - 前端：`common/searchMode.js`（三档纯逻辑）+ `common/queryDispatch.js`（**导航→查询决策从 Content.vue 抽出**，Content.vue 只剩执行/展示）。档位切换 UI 已从 ImageSearch 撤下（回上游零 diff），精确检索的界面出口在实验台 `VlLab.vue` 的 F2 区块（含 `fts_status` 索引一致性显示与「重建索引」按钮——`fts_reindex` 的唯一 UI 出口）。
  - 实测：真实库 404 文档全回填，搜「葡萄」命中 F1 标注的照片（F1×F2 闭环）。
  - 已知边界：CJK 扩展 B 生僻字不覆盖；单字 CJK 查询走 LIKE 兜底（慢）；FTS5 列一次定死，加列只能 DROP+RENAME。
- **F3 多图叙事**（未开始）：多图 + 模板 → 云端讲故事，依赖 F1 基建；协议上 Anthropic 单请求已支持多图，**不需要新端点**。
