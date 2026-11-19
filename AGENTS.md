# lappp 项目规约（AI 协作者向）

> 面向在本仓库干活的 AI 协作者与新同事：**只写项目特有的事实**——分支与红线、可执行命令、架构边界、已踩过的坑。
> 通用工程纪律见全局 `AGENTS.md`；机器事实（工具链版本、路径）见 `~/.zcode/machine-profile.md`。
> 数字与锚点核对于 2026-11-19（提交 `e87aa3d1`）。上游一更新锚点就会漂移，**以代码为准**，发现对不上就改本文。

---

## 0. 这是什么 / 分支怎么用

- 本仓库是 [julyx10/lap](https://github.com/julyx10/lap) v0.4.0 的 fork，产品名 **lappp**。
- `main` = **上游镜像**，不在其上做任何自有提交；`develop` = **所有自有开发**；功能分支从 `develop` 切出。
- `origin` = `github.com/viccom/lappp`，`upstream` = `julyx10/lap`。
- 当前：`main` = `3e265ce5`（上游 v0.4.0 + 7 个提交：状态栏字段可定制、相似图按需计数、切视图不闪背景、智能相册/合集排序移入面板菜单等），`develop` 领先 `main` **160 个提交**（含上游合并）。
- 产物三件套：`lappp.exe`（厚版/内嵌）、`lappp-client.exe`（薄客户端）、`lappp-server.exe`（无头服务端），由 `scripts/build-windows.ps1` 产出。
- crate：`lappp`（Tauri 壳）/ `lappp-core`（Tauri-free 内核）/ `lappp-server`（axum）。

---

## 1. 红线

1. **GPL-3.0-or-later**：fork 整体受此约束，分发构建物须开源；上游署名不得移除。
2. **InsightFace buffalo_s（`det_500m` / `w600k_mbf`）是「仅限非商业研究」许可**（`THIRD_PARTY_NOTICES.txt:416`）。**分发带人脸功能的构建物前必须先处理模型许可**（替换或取得授权）。自用无碍。
3. **不得改 Tauri 标识符 `com.julyx10.lap`**（`src-tauri/tauri.conf.json`）：它指向真实图库所在的数据目录，改了用户的数据就失联。代价是 WebView2 缓存与已安装版共用——这是已知取舍，不是 bug。
4. **VL 网关密钥（`vl-api-key`）是用户私有资产**：不提交、不打印、不写进日志或错误信息；后端已用 `#[serde(skip_serializing)]` + 脱敏保护，改动时不得破坏。
5. **不自动更新**：in-app updater 已关闭（`src/common/updatesPolicy.js`，`UPDATER_FEED_IS_FORK_OWNED = false`）——上游签名的产物会覆盖本 fork。在 fork 拥有自己的发布端点与 minisign 密钥前不得开启。
6. **不破坏用户资产**：真实图库（2,773 项）、`D:\LapPortable` 便携目录、`%LOCALAPPDATA%\com.julyx10.lap[.debug]` 都是用户资产，覆盖前先备份并说明替换了什么。
7. **不改上游既有行为**，除非功能方案里明确列为「适配点」并有测试保护。
8. **只在被要求时 commit/push**。

---

## 2. 事实基线

| 维度 | 现状（2026-11-19） |
|---|---|
| 版本 | `0.4.0`（`src-tauri/Cargo.toml:4`、`src-vite/package.json:4`） |
| DB schema | 迁移 **v31**（`crates/lappp-core/src/t_migration.rs:374`）；上游 v23/v24 占位，fork 自有迁移重编号 v25–v30，v31 是 v0.4.0 同步的补偿迁移。**新迁移从 v32 起** |
| 连接 | WAL / `synchronous=NORMAL` / `foreign_keys=ON` / `busy_timeout=5s`（`t_sqlite.rs:11522`） |
| 命令 | `#[tauri::command]` 共 262 处，**全部已注册**：桌面 `generate_handler!` 257 条（`src/main.rs`）+ 客户端 5 条；服务端桥接白名单 **234** 条（`crates/lappp-server/src/bridge_gen.rs`，另有断言校验三者严格划分） |
| 前端规模 | `src/common` 48 个逻辑模块、`src/components` 74 个 `.vue`、`src/views` 7 个路由视图、`src/locales` 17 种语言（en/zh-CN 各 1676 个 leaf key，其余 1660，ko 1634） |
| Rust 测试 | `cargo test --workspace` → **690 passed / 0 failed / 12 ignored**（core 605+11 ignored、壳 12、server 71、drag 2+1 ignored） |
| 前端测试 | 52 个 `*.test.mjs` 共 **547 用例**，分 **36 个 `test:*` 域**，聚合 `npm run test` 全绿 |
| 类型门禁 | `npm run typecheck:gate` 基线 **0**（`src-vite/typecheck-baseline.json`）——已经是硬零错门禁 |
| 支持格式 | **76** 个扩展名：图像 32（常规 14 + FFmpeg 14 + Affinity 4）、RAW 25（`x3f` 临时禁用）、视频 19（`crates/lappp-core/src/t_common.rs`） |
| 推理 | ONNX Runtime `2.0.0-rc.10`，**仅 CPU**（全仓无任何 execution provider 配置）；嵌入维度由运行时探测（CLIP ViT-B/32 实测 512），不硬编码 |
| 内置模型 | 5 个：`text_model.onnx`、`vision_model.onnx`、`tokenizer.json`、`det_500m.onnx`、`w600k_mbf.onnx` → `src-tauri/resources/models/`；多语言塔（540MB）按需下载到 `<appdata>/models/multilingual`；`bge-local` 描述嵌入模型为可选 |

---

## 3. 门禁命令（改完必跑，顺序即成本递增）

```bash
# 前端（src-vite/）
npm run typecheck:gate      # vue-tsc 基线门禁：只许减少不许新增（--update 重新基线化）
npm run test                # 聚合跑完 35 个 test:* 域
npm run test:crossrefs      # 命令 / i18n key / 快捷键 id 三方契约对账
npm run build               # 桌面 bundle（src-vite/dist）

# Rust（src-tauri/）
cargo test --workspace
cargo test -p lappp --no-default-features --features client   # 薄客户端裁剪腿

# 真机冒烟（重建+起 GUI+CDP，会临时注入调试端口并自动回退）
npm run smoke:desktop

# Windows 三件套
pwsh -File scripts/build-windows.ps1 -BuildProfile release   # 默认 release
```

`npm run test` 已含 `test:crossrefs`；`smoke:desktop` **不在** `npm run test` 内（它要重建并起窗口），属同步/发布门禁。

**退出码纪律**：不要用 `cmd | head` 之类的管道判成败（会吞真实退出码）；失败时贴原始 stderr。

---

## 4. 架构地图：改哪先读哪

| 目标 | 去哪 |
|---|---|
| 桌面命令（`#[tauri::command]`） | `src-tauri/src/t_*.rs`（壳）→ 实现落在 `src-tauri/crates/lappp-core/src/t_*.rs` |
| 内核逻辑（SQLite / 索引 / AI / 文件操作） | `src-tauri/crates/lappp-core/src/`（**Tauri-free**，见下） |
| HTTP 服务端（桥接 / SSE / 鉴权 / 媒体 / 静态站点） | `src-tauri/crates/lappp-server/src/`（`bridge_gen.rs` 是命令白名单真源） |
| 前端可测逻辑（纯函数层） | `src-vite/src/common/`（48 个模块，**必须可测**） |
| 前端 UI | `src-vite/src/components/`（74 个）、`src-vite/src/views/`（7 个） |
| 前端状态 | `src-vite/src/stores/`：`configStore`（全局设置→localStorage）、`libraryStore`（每库状态→后端 `saveLibraryState`）、`uiStore`/`viewerSessionStore`（纯运行时，不落盘） |
| 传输缝（桌面 ⇄ Web 切换点） | `src-vite/src/common/transport/`（`mode.js` 决定 tauri/http；`capabilities.js` 决定原生能力可见性） |
| 导航→查询决策 | `src-vite/src/common/queryDispatch.js`（纯函数，Content.vue 只执行不决策） |

**边界规则**：`lappp-core` **不允许出现 `#[tauri::command]`**——`crates/lappp-core/src/guard.rs` 是构建期守卫，会扫每个源文件。命令只能在 `src-tauri/src/` 里定义，再注册进 `generate_handler!`；服务端要暴露则加进 `bridge_gen::COMMANDS` + `EXPOSURES`（read/write/admin）。

**巨石文件**：`t_sqlite.rs` 约 1.4 万行、`t_utils.rs` 约 5.8 千行、`Content.vue` 约 1.28 万行。**新代码放新文件**，不要继续加厚它们。

---

## 5. 编码规范

### Rust

- 模块按 `t_<域>.rs` 平铺，新功能建新文件，不往 `t_cmds.rs`/`t_sqlite.rs` 里堆。
- 错误风格：`Result<T, String>`（仓库一致风格，**不用** thiserror/anyhow）。
- schema 变更**必须**走 `t_migration.rs` 的版本化迁移（新库从 v32 起），不要手改既有库。
- 后台任务：`State` 持 `AtomicBool` 取消标志 + `Mutex` 状态，进度走 `app.emit`。
- 并发纪律：同相册扫描单实例；动相册 DB 前等扫描结束；库级操作持库锁。
- 单元测试与实现同文件 `#[cfg(test)] mod tests`；新逻辑必须带测试；需要真实外部依赖的用 `#[ignore]` 并写清复现命令。

### 前端

- 所有后端调用封装进 `src/common/api.js`（222 个导出函数），组件不直接 `invoke`。
- 状态：全局设置进 `configStore`；每库 UI 状态进 `libraryStore`。
- 文案**必须**走 i18n：新增 key 至少 `en` + `zh-CN` 同步加，其余语言可后补但 key 要先占位；命名空间现状：en/zh-CN 共 31 个，其余 30 个。
- 测试：`tests/*.test.mjs`（`node --test`）只测**纯逻辑模块**；可测逻辑抽到 `src/common/` 再测，不测 `.vue` 单文件。**新增 `tests/*.test.mjs` 必须挂到 package.json 的某个 `test:*` 脚本**，否则孤儿守卫（`tests/orphanGuard.test.mjs`）会让门禁变红。
- 类型：`.vue`/`.ts` 里 **JSDoc 断言被忽略**（`/** @type {X} */ (v)` 不生效），必须用 TS 语法（`null as number | null`、`{} as Record<string, any>`）；`.js` 文件里 JSDoc `@param/@returns` 有效，但**不要给匿名解构参数加 `@param`**（会劣化推断）。
- 组件放 `src/components/`，独立全屏功能用 `WebviewWindow` + router 新路由（见 `src/common/transport/window.js` 的 `webRouteForWindowLabel`）。

### 新增测试的固定要求（历史踩坑）

1. 读源码文件必须归一化 CRLF（本机 `autocrlf=true`）：`.replace(/\r\n/g, '\n')`。
2. 锚点不得依赖跨行 `\n` 形态、位置索引（`menu.value[2]`）或注释挂位巧合。
3. 尽量 `import` 纯模块直接测；必须用 `new Function(ts.transpile(...))` 时先转译再执行。
4. 涉及文件系统时间戳的测试，变更前 `sleep ≥ 25ms`（Windows ~15.6ms tick 合并）。
5. 断言行为，不断言实现细节。

---

## 6. 已知陷阱（事故沉淀，动手前扫一遍）

1. **悬空 action id 会静默毁掉整个视图**：上游删了 `file.moveToFolder` 动作，菜单里还引用着它的快捷键；`getShortcut` 在渲染期抛错，Vue 把组件根替换成注释节点 → 宫格出现空白格、双击看不到原图。**规则**：任何 `shortcut('…')` / `getShortcutLabel(s)` 的 id 必须在 `SHORTCUTS` 注册；标签查询必须**全函数**（未知 id 返回空，不抛）。`scripts/check-cross-refs.mjs` 与 `tests/shortcutsLayout.test.mjs` 已把这条钉死。
2. **计数必须取自被计对象自己的查询**。侧栏主题计数曾复用「当前环境查询」，把无约束的库查询当成文本腿并集 → 每个主题都显示库总数（2,773）。**规则**：被计对象是主题/智能体时，计数走它自己的查询（`getSubjectCountAndSum`）。
3. **聚合量是运行时量**：`counts` / `activateTick` / `subjectCounts` 不落盘、加载即清（`src/common/libraryState.js`）。否则错值会跨会话存活（本次就残留了 8 个 2,773）。
4. **JS store 的类型陷阱**：字面量不拓宽（`'1' === '5'` 恒假）；`null` 默认值让守卫之后收窄成 `never`，一次放大几十个 TS2339；`persist:` 选项在 `pinia-plugin-persistedstate` 的类型增强未加载时会让 `defineStore` 重载解析崩塌（`configStore.js` 顶部的 `import 'pinia-plugin-persistedstate';` 就是为此，**别删**）。
5. **类型门禁的 key 必须归一化**：诊断消息里内嵌整段 store 类型串，直接拿原文当 key 会把无关的类型改动误报成新错；现按 `file :: TScode :: 归一化消息` 取键（`scripts/typecheck-gate.mjs`）。
6. **WebView2 的 CDP 只能从 `additionalBrowserArgs` 注入**：wry 总会自己传浏览器参数，此时 WebView2 **忽略** `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 环境变量。发布用的包绝不能带 `--remote-debugging-port`（`scripts/smoke-desktop.mjs` 在 `finally` 里逐字节还原配置并重建干净二进制）。
7. **`src-vite/dist` 是桌面包与 Web 包共用的 outDir**，`build` 与 `build:http` 会互相覆盖且 `emptyOutDir` 清空。交付给 `lappp-server --web-dir` 前**最后跑 `build:http`**。
8. **多语言 CLIP 塔不在 `resources/models` 里**：它按需下载到 `<appdata>/models/multilingual`，调试构建从 exe 目录解析资源（`target/debug/models`），缺模型时主题查询会直接报错「Image search model files are missing」。
9. **debug 与 release 数据目录不同**（`com.julyx10.lap.debug` / `com.julyx10.lap`），`--data-dir` **原样使用不追加后缀**；测真实库必须显式指定，别让测试跑到另一个库上。
10. **云端文件保护**：`is_cloud_only` 检查贯穿哈希/嵌入/缩略图/XMP 每条路径，新功能触碰文件内容前必须沿用。
11. **门禁本身要有牙**：聚合 runner 曾用 `spawnSync(npm_execpath, …, {shell:true})` 拉起各域，在 Windows 上 cmd.exe 根本不执行 `npm-cli.js`，**每个域都秒退 0**，门禁长期报「35/35 全绿」而一个测试都没跑（是 2026-11-19 上游合并时被真机/单文件跑法对出来的）。现在 runner 直接 `spawn(process.execPath, args)` 跑 `node …`，并由 `tests/aggregateRunner.test.mjs`（`test:gate`）钉住「失败域必须让 runner 退出 1」「域脚本必须真的被执行」。**新增任何门禁脚本时，先证明它能红。**
12. **两个 lappp 实例无法并存**：WebView2 的用户数据目录由 Tauri identifier（`com.julyx10.lap`）决定，与 `--data-dir` 无关；第二个实例的 webview 起不来（带调试端口时卡在 `setup-ready` 之前、CDP 无目标）。所以冒烟前必须先关掉用户自己的实例——**并且只能用 `taskkill /PID <pid>`，绝不能用 `taskkill /IM lappp.exe`**，后者会把用户的实例一起杀掉。

---

## 7. 上游同步流程

**必须**走 [`docs/dev/SOP-上游同步升级.md`](docs/dev/SOP-上游同步升级.md)（含基线记录、跨文件契约对账、七道门禁、真库冒烟断言、部署/回滚矩阵、事故索引）。要点：

```bash
git fetch upstream
git checkout main && git merge --ff-only upstream/main && git push origin main
git checkout develop && git rebase main     # 或 merge；冲突只可能出现在自有提交
```

- 自有改动纪律：**每个功能一个小提交系列**，改动集中在新增文件 + 少数标记过的既有文件锚点，不重排上游代码——rebase 时冲突面才小。
- 上游删除符号后必须全仓搜残留引用（`scripts/check-cross-refs.mjs` 只覆盖命令/i18n key/快捷键三类，其余靠 §3.1 的人工清单）。
- 上游可能带着**自己没跑过的红测试**（v0.4.0 的 `offlineCatalog` 就是），先在干净上游工作树复现再决定隔离还是修复——**不要默认上游是对的，也不要默认自己错了**。
- 门禁不过、或清单里存在「未确证」项，**不得合入**。

---

## 8. 部署与回滚

| profile | 二进制 | 数据目录 | 用途 |
|---|---|---|---|
| debug | `build\windows\debug\*` | `%LOCALAPPDATA%\com.julyx10.lap.debug` | 调试 / 真库实测（用户的 2,773 项库在此） |
| release | `build\windows\*` | `%LOCALAPPDATA%\com.julyx10.lap` | 产品形态；直接换 exe 会看到另一个库 |

- 覆盖 `D:\LapPortable` 前**必须**时间戳备份（`backup-<ts>/` 目录或 `.bak-<ts>` 后缀），替换后核对：内嵌 bundle hash 与本次构建一致、`grep -a -c remote-debugging-port` = 0。
- `lappp-server.exe` 不内嵌前端（运行时读 `--web-dir`/`resources/web`），前端变更时无需更新它。
- 回滚：代码 `git revert`；部署把备份改回原名；同步本身不改库数据，但上游含 SQLite 迁移时回滚前先备份 `libraries\`。

---

## 9. 协作方式（用户明确要求）

- **前端问题先解剖代码，不要反复重启二进制**：Vue 渲染期抛错会把组件根换成注释节点（`subTree === Symbol(v-cmt)`），从 vnode/组件树层查起；探针只读不改语义。
- **用户的实测就是事实**，不要先怀疑「是不是误报」再去找解释；先复现到同一条代码路径上。
- 报告要带**实际命令与真实输出**，不写「应该可以 / 理论上」。
- 破坏性或对外动作（覆盖用户目录、删数据、推送发布）先确认目标并回显。

---

## 10. 开放事项（接手者可查证后再动）

1. ~~`tests/imageNavigator.test.ts`、`montageLayout.test.ts` 未挂到任何 `test:*` 脚本~~ **已修（2026-11-19）**：两个文件移植为 `.mjs`（经 `tests/helpers/ts-module.mjs` 加载 TS 模块），分别接入 `test:navigator` / `test:montage`；孤儿守卫同时拒绝 `.test.ts`。
2. ~~SOP §3.2 写着「待建 `scripts/smoke-desktop.ps1`」、「34 域」~~ **已修**（脚本是 `.mjs` 且已落地；域数现为 **36**）。
3. ~~`scripts/README.md` 未收录 fork 新增脚本、且称 PS 模型下载脚本只做存在性检查~~ **已修**（五个 fork 脚本已补录，SHA-256 的说法已更正）。
4. ~~`main` 落后 `upstream/main`~~ **已修**：`main` 已快进到 `3e265ce5` 并推送；7 个上游提交已合并进 `develop`（merge，非 rebase——不改写历史、不需要强推）。合并只冲突 `README.md` 一个文件（上游加了一张预览截图），其余自动合并；**上游新带进来 1 个类型错**（`statusBarFields: null` 默认值把类型收窄成 `null`），已用 JSDoc 标注修掉。
5. 根 `README.md` 是本 fork 的说明；上游英文原文见 [upstream](https://github.com/julyx10/lap#readme)，`i18n/README.*.md` 是上游译本（上游这次同步了 16 份译本）。
6. 仍未做：`contrib/aur` 配方还钉在上游 `lap` 0.3.2，fork 未维护；`docs/.vitepress` 文档站仍指向上游（发布页 base=`/lap/`）。
