# SOP：lappp 上游同步升级（upstream sync）

> 版本：v1（2026-11-18，依据 v0.4.0 同步事故复盘制定）
> 适用：把 upstream `julyx10/lap` 的新版本合入本 fork 的 `develop`。
> 事故样本：v0.4.0 同步（07a074c5）保留了上游已删除动作的菜单引用
> （`file.moveToFolder`），菜单构建在 Thumbnail/MediaViewer 的 render 里执行，
> Vue 把 render 异常吞成空注释节点 → 宫格空缺 + 查看器白屏，且无任何错误提示。
> 全过程见《排查-2026-11-18-宫格空缺与原图白屏.md》。

## 0. 原则（红线，违反任何一条不得合入）

1. **门禁不过不合入**；测试清单上存在「未确证」项 = 门禁不过（v0.4.0 的 E2E 清单
   自己写着「大图渲染未确证」，仍然放行，用户实测即炸）。
2. **「构建通过」≠「能跑」≠「渲染正确」**：必须用真实图库做真机冒烟。
3. **git 不替你发现语义冲突**：上游删除的符号/id/key，merge 无文本冲突也可能在
   fork 侧留下悬空引用——同步后必须做**跨文件契约对账**（§3.1）。
4. **错误日志里没有「噪音」，只有未分类信号**：冒烟/排查时逐条归类 console 错误，
   不得按关键词过滤掉（本次事故的根因异常一直可见，却被当噪音滤掉）。
5. **数据目录随构建形态走**：debug → `%LOCALAPPDATA%\com.julyx10.lap.debug`，
   release → `%LOCALAPPDATA%\com.julyx10.lap`。换 profile 部署前先确认用户库在
   哪个目录，否则用户看到的库会"消失"。

## 1. 同步前（准备与基线）

- [ ] `develop` 干净，无未提交改动；确认部署窗口（用户没在用 portable 版）。
- [ ] 读上一份 `SYNC-*.md` 评估与本 SOP；上游 release notes / 变更摘要过一遍。
- [ ] **记录基线**（同步前必须知道"绿"长什么样，否则之后分不清是同步引入还是本来就红）：
  - [ ] 前端各域 `npm run test:*`（在 `src-vite/`）逐域记录 pass/fail 数；
  - [ ] `cargo test --workspace` 记录 passed/failed/ignored；
  - [ ] `bash docs/dev/…` 既有冒烟脚本可跑通。
- [ ] 快照当前数据目录大小（同步不应改库数据；若有 schema 迁移，先备份库目录）。

## 2. 同步操作（merge）

按 `AGENTS.md`「分支与同步策略」：

```bash
git fetch upstream
git checkout main && git merge --ff-only upstream/main && git push origin main
git checkout develop && git merge main        # 或 rebase，按当时评估文档定
```

**冲突裁决纪律**：

- 上游**删除**的导出符号/类型成员/常量/id —— 解决冲突时采纳删除后，必须
  **全仓 grep 残留引用**（本 fork 自有代码可能还在用）。这是本次事故的精确形态：
  shortcuts.ts 采纳了上游删除，fileMenu.ts 的引用漏网。
- 命令模板（merge 完成后立刻执行）：

```bash
# 列出本次 upstream 范围内被删除的行里「像导出符号/id」的候选
git diff --diff-filter=M HEAD~1 -- src-vite/src src-tauri/src \
  | grep '^-' | grep -oE "id: '[a-zA-Z.]+('|[A-Za-z]+')" | sort -u
# 对每个候选 id / 符号名，全仓搜残留（示例）
grep -rn "file.moveToFolder" --include="*.ts" --include="*.vue" src-vite/src
```

- fork 侧改动与上游冲突时，按 `SYNC-*.md` 评估文档的裁决表处理，不现场发明。

## 3. 同步后门禁（按顺序执行，全绿才算过）

| # | 门禁 | 命令 / 方法 | 状态 |
|---|---|---|---|
| 1 | 跨文件契约对账 | **`npm run test:crossrefs`**（`scripts/check-cross-refs.mjs`：invoke 命令↔generate_handler、i18n 字面量/动态前缀↔en.json、shortcut id↔SHORTCUTS 三类闭包，任一悬空即红） | **已具备**（含在聚合门禁内；快捷键另有 test:shortcuts 双保险） |
| 2 | 类型检查 | 全量 `npm run typecheck`（vue-tsc --noEmit）；**同步门禁用 `npm run typecheck:gate`**（基线制：不许新增，存量见 `src-vite/typecheck-baseline.json`，清零后 `--update` 收缩） | **已具备（基线制）**；快捷键契约另有微型类型检查跑在 `test:shortcuts`；全绿为最终目标 |
| 3 | 前端全量测试 | 各域 `npm run test:*`；**聚合 `npm run test`**（34 域顺序执行、汇总、任一红即失败）；孤儿守卫 `tests/orphanGuard.test.mjs` 保证任何测试文件必须挂在某个脚本上 | **已具备**（本地聚合 + CI `frontend-gate` job） |
| 4 | Rust 测试 | `cargo test --workspace` | 已具备 |
| 5 | 前端构建 | `npm run build`（src-vite/） | 已具备 |
| 6 | 三件套构建 | `scripts/build-windows.ps1`（默认 release；`-BuildProfile debug` 出 `build/windows/debug/`） | 已具备 |
| 7 | 真机功能冒烟（真库） | §3.2 | **待固化脚本**（素材已齐，见下） |

### 3.1 跨文件契约对账（本 SOP 的灵魂）

对账清单（上游动了哪类，就查哪类；逐项执行并留输出）：

| 契约类型 | 注册处 | 引用处 | 对账方法 |
|---|---|---|---|
| 快捷键动作 id | `common/shortcuts.ts`（SHORTCUTS + ShortcutActionId） | `shortcut('…')` / `getShortcutLabel` / `matchesActionShortcut` 全仓字面量 | 回归测试已钉 fileMenu；全仓扫描脚本化（`scripts/check-cross-refs.mjs`，待建，今天手搓版：正则收 31 个字面量 → 逐个 `getShortcut` 验证） |
| i18n key | `locales/*.json` | `t('…')` / `localeMsg…menu…` 字面量 | 上游增删 key 后全仓对账（v0.4.0 曾出 17 语言包漏改，af894bbf） |
| Tauri 命令白名单 | lappp-server `SKIP`/bridge 注册表（257 命令） | 前端 `invoke('…')` 字面量 | 已有 `bridge_whitelist_covers_the_registration_exactly` 测试；前端侧字面量对账待脚本化 |
| 事件名 / 协议名 | `t_events` / SSE / `lapd.lock` 等约定常量 | 字面量引用 | 上游动过就 grep |
| 数据目录/标识符 | `t_config.rs get_app_data_folder_name` | 部署脚本、文档里的硬编码路径 | 同步后 grep 一遍（改名事故：Lap→lappp 后脚本里残留旧名） |

### 3.2 真机功能冒烟（最小断言集，缺一不可）

1. 以**用户实际数据目录**对应的 profile 启动真机（portable 用 debug；见 §4 矩阵）。
2. CDP 断言（参考 `docs/dev/ui-smoke.mjs` 的连接方式与 2026-11-18 排查中的探针）：
   - [ ] 打开库后 `[data-thumbnail-container]` 数量 > 0；
   - [ ] 向下、向上各滚动一次后数量仍 > 0（本次事故的坏格特征：cell 内只有 `<!---->`）；
   - [ ] 真实双击（`Input.dispatchMouseEvent` clickCount=2）后 viewer 出现且存在
         `img.naturalWidth > 1500` 的原图；
   - [ ] 截图存档到 `docs/dev/` 或任务记录。
3. **console 错误全量归类**：`Runtime.consoleAPICalled(error)` + `exceptionThrown`
   逐条给结论（已知无害要给出依据），不许按关键词丢弃。
4. 注意 `ui-smoke.mjs` 的警告：`--remote-debugging-port` 会杀运行时创建的子 webview
   窗口——只测主窗口流程；子窗口用 UIA。**冒烟用的构建不得带该参数发布**
   （tauri.windows.conf.json 的 `additionalBrowserArgs` 用完必须回退）。

> 待建：`scripts/smoke-desktop.ps1` 固化上述断言（临时参数自动注入/回退）。

## 4. 部署（D:\LapPortable，便携测试形态）

| profile | 二进制 | 数据目录 | 说明 |
|---|---|---|---|
| debug | `build/windows/debug/*` | `com.julyx10.lap.debug` | 现行便携形态（用户 2,773 项库在此） |
| release | `build/windows/*` | `com.julyx10.lap` | 产品形态；直接换 exe 用户会看到另一个库 |

- 覆盖前**必须**时间戳备份：`cp lappp.exe lappp.exe.bak-$(date +%Y%m%d-%H%M%S)`
  （备份仅用于回滚，验证通过后可删；不需要它复现 bug——源码在 git 里）。
- 部署后校验：内嵌 bundle hash 与本次构建一致、`grep -a -c remote-debugging-port` = 0。
- `lappp-server.exe` 不内嵌前端（运行时读 `--web-dir`/`resources/web`），
  前端变更时无需更新它。

## 5. 回滚

- 代码：`git revert -m 1 <merge-commit>`（merge 提交）或 revert 具体修复提交。
- 部署：`.bak-<ts>` 改回原名。
- 数据：同步本身不改库数据；仅当上游含 SQLite 迁移时，回滚前先备份
  `%LOCALAPPDATA%\com.julyx10.lap[.debug]\libraries\`。

## 6. 事故案例索引（每条教训对应本 SOP 的一条规则）

| 事故 | 教训 → 规则 |
|---|---|
| 2026-11-18 宫格空缺/查看器白屏（悬空 shortcut id） | §0.3 对账、§0.4 错误归类、§3.1 快捷键对账、§3.2 冒烟断言 |
| 2026-11-18 E2E「大图渲染未确证」仍放行 | §0.1 未确证项不得合入 |
| SW 毒化（9b48f8e7） | §3.2 真机冒烟的由来：构建≠能跑 |
| 改名残留（Lap→lappp：脚本/文档/关于页/updater） | §3.1 最后一行：改名类同步全仓 grep |
| 排查期反复重启二进制无效 | 前端问题先解剖 vnode/组件树（读 `subTree`、包 `setup` 捕 render 异常），探针只读不改语义 |

## 7. 待建资产清单（按优先级）

1. ~~`ShortcutActionId` 改为从 `SHORTCUTS` 派生~~ **已落地（2026-11-18）**：
   `type ShortcutActionId = (typeof SHORTCUTS)[number]['id']`，SHORTCUTS 去掉
   宽化注解、VIEW_BACKGROUND 的 `satisfies` 移除（否则 TS2456 循环）。
   契约由 `tests/shortcutActionIdType.test.mjs` 钉死（shortcuts.ts 0 诊断、
   探针 @ts-expect-error 拒收未注册 id、类型↔注册表双向相等、防塌缩成
   string），已接入 `npm run test:shortcuts`。
2. ~~`vue-tsc --noEmit`：加 devDep + `npm run typecheck`，进 §3 门禁与 CI~~
   **已落地（2026-11-18，基线制）**：`vue-tsc 3.3.14` devDep + `npm run typecheck`（全量）+
   `npm run typecheck:gate`（`scripts/typecheck-gate.mjs`，key=file::code::message，
   只许减少不许新增；注入新错实测红、还原实测绿）。存量盘点：**311 错 / 42 文件**——
   TS2339×94（JS Pinia store 对 TS 不可见）、TS2322×54、TS2367×41（**疑似真 bug**，
   如 Calendar.vue 字面量 '1'==='5' 比较）、TS2345×38、TS7006×37、TS2503×8
   （NodeJS 命名空间误入前端，应 `ReturnType<typeof setTimeout>`）、零星其余；
   重灾文件 Content.vue×147、Settings.vue×31、Image.vue×17。
   清零策略：先修 TS2367/TS2503 疑似真 bug → configStore.js 等补类型（一次消 ~94）→
   按文件烧余量；每批结束 `--update` 收缩基线。CI 接入归 §7-③。
   **第 1 批已落地（2026-11-18）：311 → 250**——TS2367×41 全部是
   `sidebarIndex === SIDEBAR.X`（configStore.js 的 JS 不拓宽字面量污染，非真 bug；
   修复 = state 字段 JSDoc 标注 `Sidebar` 联合）；TS2503×8 = `NodeJS.Timeout` →
   `ReturnType<typeof setTimeout>`；附带 12 个连锁消失 + 1 个真赋值错误
   （Home.vue clickSidebar 参数补 `Sidebar` 类型）。教训：门禁 key 的消息必须
   **归一化砍掉内嵌类型串**（`on/to type 'Store<…>'`），否则无关类型改动会
   把健康修复误报成新错。
   **第 2 批已落地（2026-11-18）：250 → 222**——TS2339 的 store 不可见大类，
   根因不是"JS store 缺类型"，而是 `configStore.js` 的 `persist:` 选项在
   **插件类型增强未加载**时让 pinia defineStore 重载解析崩塌（对照实验：
   mini store + persist → 推断死；+ `import 'pinia-plugin-persistedstate'`
   → 恢复）。修复 = store 顶部一行 side-effect import（零行为变更）。
   剩余 TS2339×70 已换根因：36 个 `on type 'never'` 级联 + ImportMeta/env
   等零星，留后续批次。
   **第 3 批已落地（2026-11-18）：222 → 189**——never 级联三源：
   ① `uiStore.countUpdateRequest: null`（null 推断 → 守卫排空 → never，
   一次放大 27 错）：补 **判别联合** typedef（按 `source` 收窄，消费端
   switch 自动获得精确载荷）；② Settings.vue `ref(null)` 同型（8 错）：
   ref 泛型标注；③ `configStore.externalApps` 默认 `apps: []`（JS 里
   `never[]`，毒化按 kind 索引）：JSDoc cast。教训：**JS store/JS 模块的
   `null`/`[]` 默认值是 never 级联的头号源头**，新写 state 一律带 JSDoc
   类型或判别联合。
3. ~~26 个孤儿测试接入 `test:*` + `npm run test` 聚合脚本；CI 加测试步骤~~
   **已落地（2026-11-18）**：20 个绿孤儿接入 16 个新域脚本；聚合 `npm run test`
   = `scripts/run-frontend-tests.mjs`（跑全部 `test:*`、排除隔离区、汇总，
   **31/31 绿**）；`tests/orphanGuard.test.mjs` 守卫"任何测试文件必须挂在
   某脚本或隔离区"（放入未登记文件实测红）；CI（pr-build.yml）新增独立
   `frontend-gate` job = `typecheck:gate` + `npm run test`
   （**独立并行不加 needs**——`needs` 会让 workflow_dispatch 按平台跳过时
   连锁跳过构建 job）。
   **隔离区已清空并删除（同日续）**：6 文件 67 测试全部修复转入正域
   （现 **34 域 / `npm run test` 34/34 绿**）。修复定性（无一为 fork 产品
   bug，但实锤两类上游债）：
   ① **CRLF 坑**：上游 offlineCatalog 的 import 剥离正则只锚 `\n`，
   Windows 检出带 `\r` → import 残留 → 整文件加载失败（上游 CI 全 LF
   从不暴露；即便取上游新测试也要补 `\r?`）；
   ② **上游自破测试**：上游 a2cf51ec 更新过 offlineCatalog，但取其测试
   在 upstream 干净树对照跑仍 **24/6 红**——上游不跑前端测试；folderDeletion/
   afAreaShortcut 等断言停留在已删除的旧流程（`comment`/`rotate` 菜单项、
   A→Alt+A 绑定、warmup 云检、失败对话框不再有 retry），逐项按现行契约重写
   并注明依据提交号；
   ③ 其余为锚点漂移（`export` 剥离单发 replace 漏第二处、fixture 缺 fork
   新增的 transportMode/signMediaUrl 桩）。
   另修 appearance/locale 两域漏 `--experimental-vm-modules` 旗标。
   教训沉淀：**从上游同步测试 ≠ 拿来就能绿**——上游自己的前端测试不在
   CI 里，取回后必须在本地全量跑一遍再合入。
4. ~~`scripts/check-cross-refs.mjs`：§3.1 全仓对账脚本化~~ **已落地
   （2026-11-19）**：`npm run test:crossrefs` 进聚合（35 域）。三类闭包
   全绿：262 注册命令 ↔ 241 invoke 引用、1676 en.json 键 ↔ 全部字面量与
   动态前缀、62 shortcut id ↔ 47 引用（现存零悬空——本次是建防线，非还债）。
   实现要点：invoke 匹配须容忍泛型 `invoke<T>('cmd')`；i18n 的
   `t('prefix.'+x)` / ``t(`prefix_${x}`)`` 按**前缀命中任一真实键**判定，
   不是节点判定（首版把 `settings.raw.source_*`/`album.edit.tab_*` 误报，
   实际键全在）；shortcut 字面量字符类要点号在后（`[a-zA-Z][\w.]*`）。
   咬合验证：注入假命令/假键/假动作 id 三类均红并点名 file:line。
5. `scripts/smoke-desktop.ps1`：§3.2 断言固化。
6. `docs/dev/launch-portable-check.ps1` 里硬编码的 `Lap.exe` 更新为 `lappp.exe`。
