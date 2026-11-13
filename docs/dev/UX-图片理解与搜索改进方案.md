# 图片理解与搜索体验改进方案

日期：2026-11-13（2026-11-14 补充二次拍板、BUG 修复与 VL 标签回写批次）
状态：**实施完成**（AGENTS §1–§8 已实现并跑门禁，见 §9bis / §9ter 为实测后修订）

用户已拍板：
1. 图片理解搜索与 F2 主视图方案合并，但作为**独立侧栏入口**；官方搜索 UI 与原查询行为完全保留。
2. 主视图右键/多选直接发起 VL 任务，不跳实验台；设置内增加任务监视页。
3. 图片理解设置必须支持 Anthropic Messages 与 OpenAI Chat Completions。
4. VL 标注状态采用派生标记，不给 `afiles` 加列。
5. 任务历史需要按 run 分组持久化。

事实基线：[ARCHITECTURE-FACTS-2026-11-13.md](./ARCHITECTURE-FACTS-2026-11-13.md)
F2 合并关系：[F2-主视图搜索集成方案.md](./F2-主视图搜索集成方案.md)（只保留新入口适用的设计；改造官方管线的部分不执行）
协议参考：[D:\MyCodes\AI-album\vl-gw](D:\MyCodes\AI-album\vl-gw)

## 0. 总览与架构边界

| # | 需求 | 最终形态 | 主要触面 |
|---|---|---|---|
| 1 | 新图片理解搜索入口 | 侧栏新增 `IconSparkles` 项、独立三档搜索面板 | Home、queryDispatch、Content、新逻辑模块 |
| 2 | 设置面板增强 | 网关/云端模型/嵌入模型/模板分区；模型列表在线刷新；双协议 | Settings、t_vl、t_embed、config、locales |
| 3 | 图像属性查看 VL 结果 | FileInfo 新增折叠组 | FileInfo、vlApi、locales |
| 4 | 手动发起 VL + 监视 + 标注筛选 | 主视图直接排队；设置页看持久任务；Library 标注工作队列 | fileMenu、Content、Library、QueryParams、VL Store |
| 5 | 缩略图标注状态 | 读取 AFile 查询投影的派生 `vl_annotated`，独立角标 | t_sqlite、Thumbnail |

- **官方搜索零行为改动**：保留 `ImageSearch.vue`、`getUnifiedSearchFileList`、LIKE + CLIP。CLIP 继续支持以图搜图与 subjects 智能相册；新 AI Search 不调用 CLIP 图像向量。
- **VL 标注状态派生**：`EXISTS(vl_annotations WHERE file_id=a.id)` 作为 AFile 输出字段；筛选用 `EXISTS/NOT EXISTS`。不加 afiles 列、不加标注钩子、不为标注状态单独迁移。
- **任务历史持久化**：v27 新增 `vl_runs`，`vl_calls` 增 nullable `run_id` 与索引；run ID 使用 UUID string。Run 固定启动时的 library 上下文。
- **RRF 不引入**：用户已拍板收益有限，不再提议。

## 1. 独立 AI Search 入口

### UI / 侧栏顺序

- 新增 `SIDEBAR.AI_SEARCH = 9`，保持现有 index 稳定；`Home.vue` 新增 `AiSearchPanel` / `IconSparkles`。
- 当前 `visibleButtons` 按 `index` 排序。增加可选 `order` 并按 `order ?? index` 排序；AI Search 使用 `order: 3.5`，显示在 Search 下方、Tag 上方。不要重编号已有 sidebar 项（其 index 已持久化）。
- `AiSearchPanel.vue` 复用官方输入交互，但使用**独立的搜索历史**（`libConfig.search.aiSearchText / aiSearchHistory / aiSearchHistoryIndex`），与官方 `searchHistory` 分开存储，互不污染（2026-11-14 用户实测后改拍板，见 §9.9）。

### 查询计划与执行

- 新建 `common/aiSearch.js`：`AI_SEARCH_MODES = ['precise','semantic','all']`、`planAiSearch(mode) -> {fts,captions}`。
- 不复用官方 `searchMode.js::planSearch`：官方 all/semantic 的 vector 是 CLIP；新入口 all 是 FTS + caption embedding。档位存 `config.settings.aiSearchMode`，不能改官方 `searchMode`；新增设置需进入 Content 查询刷新 watch。
- `precise` 调 `search_fts`；`semantic` 调 `search_captions`（默认阈值 0.45）；`all` 两路并行、按 file id 去重并分组展示。
- 搜索命中 IDs 取回 AFile 后必须按 hit 顺序重建，不依赖 SQL `IN` 返回顺序。
- 新入口复用 Content 的客户端 search-result path，但需检查所有 sidebar-index 专用逻辑：历史计数回填、删除后重查、分组/排序、地图、count、`currentSearchFileIds`。
- 搜索排除目录必须在 FTS/caption 的 Top-N 截断前应用；同时落实 fileType、favoritesOnly 和普通相册可见规则。`album_visible` 不等于 `is_excluded_from_search`。
- Content 的官方 Search 分支和对拍基线维持原样。

## 2. 设置：模型发现与双协议

### 区块与模型选择

Settings → 图片理解分成：**网关**（地址、API Key、协议）、**云端模型**（图片理解模型、重排模型、能力/限制、刷新）、**嵌入模型**（local/cloud、cloud embedding model）、**提示词模板**。

模型控件允许保留/手输不在服务端列表里的当前值。VL 模型列表支持三态能力 `Supported/Unsupported/Unknown`：
- magpie：`modalities.input` 含 `image`，上下文键 `context_length`；
- vl-gw：`capabilities` 含 `vision`，上下文键 `context_window`；
- 未知 provider/字段标为 Unknown，分区显示并允许用户显式选择，不静默过滤掉。

`GET /v1/models` 使用异步 `reqwest::Client`，独立 10–15s 超时。鉴权跟随 protocol：Anthropic 必须 Key、发 `x-api-key` + `anthropic-version`；OpenAI Key 可空，非空才发 Bearer。OpenAI 空 Key 下若 models list 返回 401，显示“列表要求鉴权，可手动输入模型”，但不可把 Chat 服务判为未配置。

模型限制展示 context/output metadata。VL `max_tokens` 的实际配置和请求映射要与展示一致；已知 output 上限时默认值不超过该上限，未知时保留默认值。

`/v1/models` 不包含 embedding 模型（实测），cloud embedding 名仍可手输并保留当前配置。

### ProtocolConfig 合同

`VlConfig.protocol = anthropic | openai`，旧配置默认 anthropic。统一 ProtocolConfig 传递到 VL、rerank、cloud embeddings、model discovery 的 endpoint/header/body/response parser。

- Anthropic：`/v1/messages`；顶层 system；base64 image source；解析 `content[]` 与 thinking blocks。
- OpenAI Chat Completions：`/v1/chat/completions`；system role message；data URL `image_url`；解析 `choices[0].message.content`（字符串或 parts）、`reasoning_content/reasoning`、`prompt_tokens/completion_tokens`；rerank 不发 Anthropic 专有 `thinking`。
- OpenAI 无 Key 允许 VL/chat 与 listModels 尝试；list 401 单独提示，不阻断 chat。Anthropic 仍要求 Key。
- Cloud embeddings 同样按协议鉴权：OpenAI 用 Bearer，Anthropic 按现有网关约定用 `x-api-key`。
- URL join 必须按 URL path 解析并保留 reverse-proxy prefix：base 无 `/v1` 时加完整 `/v1/...`；base 已以 `/v1` 结尾时只加 endpoint suffix；base 已含完整 endpoint 不重复；invalid URL 清晰报错。分别测试 messages、chat/completions、models、embeddings、rerank，禁止简单字符串 replace。
- vl-gw 只作协议结构参考；Lap Rust 侧自行构造请求，不复制其转换库。
- 修复全部 17 个 locale 中缺失的 `settings.vl.embed_source_local/cloud`。

## 3. FileInfo 图片理解分组

- `FileInfo.vue` 地点组之后、源文件元数据之前新增折叠组；默认折叠；展开后调用 `get_vl_annotations(fileId)`，折叠时不请求。
- 同一文件多模板标注按 template/version 分组；`response_spec` 必须通过 `findTemplateVersion(templates,id,version)` 取标注当时版本，不能用当前最新版。
- 根据 result JSON / response_spec 顺序展示 caption/summary/description 等主要描述与结构化字段；同时显示模型、模板版本、时间、复制描述入口。
- 无标注状态展示明确空态或隐藏整组（实际实现选定一种、保持一致）；VL finish 事件后刷新当前文件信息。

## 4. VL 主视图任务、监视与标注筛选

### 4a. 主视图发起任务

- `fileMenu.ts` 单图与多选菜单新增「图片理解（云端）」；多选菜单纳入 batchActions。
- Content 弹模板/force/数量模态；模板选已启用最新版本并保存上次选择；未配置网关或无可用模板时提示去 Settings。
- 确认后 `run_vl_analysis(fileIds, templateId, force)`；toast 监听 progress/finished。finish 时刷新当前文件列表的派生标记、FileInfo 标注与 Library 全局计数。
- 取消为协作式：当前 HTTP 请求结束或 timeout 后停止后续项，UI 不声称即时中断。
- 在“未标注”视图成功标注的文件会从列表消失，这是预期行为；显示提示并修正 selected index/空态。

### 4b. 设置内 VL 任务监视

- 以持久化 `vl_runs` 为 run 元信息/状态/进度权威，`vl_calls.run_id` 查询实际调用明细。实时事件驱动 UI；任务页在存在 queued/running 任务时按 4s 轮询，离开该页即停。支持取消当前 session 的活跃 UUID run（含 queued 态，取消即从队列移除并终态化）。
- 独立的设置页「VL 任务」（`VL_TASKS_TAB = 11`），与「图片理解」并列：每个任务显示本地化状态（排队中/执行中/已完成/取消/失败/中断）、进度条、成功/失败/跳过计数、开始/结束时间，展开可看逐条 call 明细（`list_vl_run_calls`：照片名、成功/失败、模型、耗时、token、原始返回或错误）。
- 历史跨已配置 libraries 汇总并显示 library 名称；已结束任务可查看不可取消。应用退出时仍 running/queued 的 run 下次访问/恢复时标为 interrupted，不伪报完成。
- **并发上限**：run 进入 FIFO 队列，同时最多执行 `maxConcurrency` 个（默认 2，1–16 可配，存 `VlConfig.max_concurrency`），等待中的 run 状态为 `queued`。默认 2 是因为不同服务商/模型/时段对并发请求的限制不同（2026-11-14 用户拍板，见 §9.10）。

### 4c. Library VL 标注筛选

- Library 左栏评分组后、culling 组前增加「VL 标注」折叠组，含已标注/未标注两个独立 library items。当前 item 是单选，不自动叠加 rating/culling。
- `LIB_ITEM` 两份定义、`planLibraryQuery`、`buildNavigationState`、Content query watcher、`getFileList` 参数解析全部透传 `vlAnnotated`。
- `QueryParams` 增 `#[serde(default)] vl_annotated: Option<bool>`（camelCase IPC `vlAnnotated`）；`build_search_query_parts()` 加 EXISTS/NOT EXISTS，使普通列表、count/sum、分页/分组同条件。
- Library 全局 `get_library_visible_counts_on()` 是单独 SQL；沿用 search exclusion、live photo companion、album filter 基准，单次聚合算 total/annotated，unannotated=total−annotated。同步 `LibraryVisibleCounts`、Pinia defaults/writeback、菜单数量显示与 finish 后刷新。
- AFile 查询派生字段在 `build_base_query` SELECT 尾端追加 `EXISTS(...) AS vl_annotated`，`from_row` 同步追加索引；明确 bool，不引用不存在的 `a.vl_annotated` 列。

## 5. Thumbnail 标记

- Thumbnail 读取 AFile 派生字段 `file.vl_annotated`，使用独立 VL 图标 badge，不和星级/元数据 badge 混成一项。
- 验证与 offline/cloud/dedup 现有 badge 的遮挡、可访问标签与 hover title。
- 不产生数据库 migration 或 VL annotation 写入钩子。

## 6. VL run 持久化和跨 library 隔离（v27）

### Schema / ID / 查询

- 每个 library DB 新增 `vl_runs` 表；run 主键 `TEXT UUID v4`（当前 AtomicI64 每次重启重用，不可作为持久 ID）。
- `vl_runs` 快照：`library_id/name`、`template_id/version/name`、force、state、total/processed/succeeded/failed/skipped/current_file、started/finished、error summary。模板元信息不设可能抹除历史的 cascade FK。
- `vl_calls.run_id TEXT NULL REFERENCES vl_runs(run_id) ON DELETE CASCADE` + `(run_id,id)` 索引；迁移前 call 行保持 NULL。
- 新命令 `list_vl_runs`、`list_vl_run_calls(run_id)`；run list 跨 library 聚合。每库 run history 与 library DB 共生：**永久删除 library 时历史随 DB 删除**，不承诺外部归档。删除仍有活跃 run 的 library 必须拒绝/先取消并等待 run 终态，不能继续后台访问/重建被删 DB。

### Lifecycle / recovery

- `run_vl_analysis` 服务端捕获当前 library，创建 run 并将 library context 注入 Store 与 Source；所有模板、annotation、calls、prompt context、tags/people、thumbnail、原图读取都按该 library ID 短连接定位。前端 libraryId 不具权威性。
- 不持 DbConnectionLease/SQLite transaction 跨 gateway HTTP 等待；`AThumb::fetch_for_library` 当前内部仍用 current-library open_conn，需补显式库访问。
- 每个文件完成后短事务写 call/annotation 与更新 run counter（若不同 DB transaction 分步，`vl_runs` counter 是恢复权威，call 是证据）；网络请求绝不包事务。
- Skipped 没有 call，单独写 skipped count；cancelled 时 pending 项通过 total−processed 体现；错误、HTTP client 构造失败、store error、panic/JoinError 都必须终态化并发 finish event。
- 启动时对当前库与跨库列表涉及的旧库做 schema check/迁移；遗留 running 标记 interrupted。任何库迁移失败单独显示 unavailable，不吞其他库列表。
- 内存 run/cancel map 只保存活跃任务并有界清理；跨重启历史可查看不可取消。无需 `vl_run_items`，本次不承诺逐文件恢复 skip/pending 清单。
- 全面改 UUID string：Rust RunStatus/commands/events/VlCallDraft/VlCall；前端 `vlApi.js`、`VlLab.vue`、`VlPanel.vue` 全部去除 `Number(runId)` 并做字符串比较；tests 同步。

## 7. 共享查询边界与实现切分

- 搜索排除目录不是 `album_visible`。FTS 与 captions 必须在 Top-N 截断前排除 `is_excluded_from_search` 文件；实现可在 crate 内提供可搜索 ID/predicate helper，并保留原排名顺序。
- AI Search 接入 `currentQuerySource='search'` 时需复核 sidebar-index、排序分组、地图、历史计数、删除重查、fileType/favoritesOnly、`currentSearchFileIds` 所有专用分支。
- `vl_annotated` predicate 同时服务 list 与 count/sum；全局 Library counts 独立处理，不能只加 QueryParams 后漏侧栏 count。
- 协议适配、AI search planner/result assembler、library-bound VL Store/Source 应抽独立逻辑/测试边界；原有官方文件只做最小锚点修改。

## 8. 实施前质量门禁

**Backend migration/tests**：v26→v27、迁移重跑、旧 calls NULL、run/call FK/索引、run UUID 持久查询、calls 分组；所有当前 `user_version == 26` 测试断言更新（审查发现不止早前估的 3 处，实施时 grep 全部匹配）；SQL fixtures (`VL_SCHEMA_SQL`/t_embed) 同步。

**Run 测试**：skip 无 call 但计数增加；取消在途/取消 pending 语义；client 初始化失败/DB 错/panic 终态；重启 running→interrupted；UUID roundtrip；切库期间同一 run 仍只访问启动库；删除 active library 被挡住；历史 library 删除行为明确。

**Protocol tests**：Anthropic/OpenAI request body、system prompt、image data URL、rerank 无 thinking 字段、response content 两种形态、usage/reasoning、empty key、models 401 fallback、URL base 无 `/v1`/有 `/v1`/带 reverse proxy prefix/已有 endpoint。

**Search tests**：三档 planner，官方 Search 对拍零变化；排除目录在 Top-N 前生效；fileType/favoritesOnly/album filters；hit order / dedup；history count / delete refresh / map / grouped rows。

**Frontend tests/E2E**：Library query arg 和 counts 全链路；UUID 不 Number 化；VL finish 刷新 Thumbnail/FileInfo/count；主视图单选/多选发起、监视页取消/恢复；Settings model capability unknown 状态与 locale 全覆盖。

代表性图库执行 `EXPLAIN QUERY PLAN` 和计时派生 EXISTS；无实测数据不在文档写固定延迟估值。

---

## 9. 拍板记录（2026-11-13 用户裁决）

1. 新搜索入口与 F2 方案合并，独立入口，官方面板/管线零改动。
2. ~~新入口复用官方搜索历史。~~ **已被 §9.9 覆盖**（改为独立历史）。

---

## 9bis. 二次拍板记录（2026-11-14 用户实测反馈后裁决）

9.9 AI 搜索历史与官方搜索历史**分开存储**（各自独立的 text/history/index），互不污染。
9.10 VL 任务引入**并发上限**设置（设置→图像理解，默认 2，范围 1–16），run 入 FIFO 队列，超出者显示「排队中」。理由：不同 LLM 提供商/模型/时段/API 类型对并发的限制不同。
9.11 修复 FileInfo 图片理解组的时间显示 BUG（`formatRelativeTime` 缺第二参数导致渲染中断）。
9.12 修复并补全 17 个 locale 中缺失的 `settings.vl.embed_source_local/cloud` 及本批次新增键。

---

## 9ter. 三次拍板记录（2026-11-14 VL 标签回写）

用户要求：VL 理解完成后，云端返回的 `tags` 直接设置到源图片。拍板 **1A / 2A / 3A**：

- **1A 标签挂载点**：所有 AI 标签挂在统一根标签 `AI理解` 下（子标签按名去重、跨文件共享）。根标签是来源标记，便于识别与批量清理，不污染用户标签树。
- **2A 写入时机**：VL 任务成功即自动写入（无需用户点按）。
- **3A 重跑语义**：force 重跑仅删除该文件**挂在 `AI理解` 子树下**的旧标签，再写新标签；用户手写标签永不触碰（符合「AI 输出不覆盖用户手写事实」）。空结果**不删**已有 AI 标签（防止一次无标签返回清空历史）。
- 标签字段取自 `response_spec` 声明的 `tags`（缺省回退 `keywords`→`labels`→值中出现者）；单文件上限 50，去重按大小写不敏感，含 `,;|` 或空白的重名直接丢弃。写入后发 `tags-changed`，标签面板自动刷新。
- **源文件 XMP 不自动写**：沿用本仓库「XMP 手动导出」纪律；AI 标签进入待导出队列（既有 `axmp_local_tags_INSERT` 触发器），由用户手动「保存元数据到 XMP」落到文件旁。
- 实现：`t_vl_tags.rs`（纯函数提取 + 受库约束的连接写入）、`VlStore::apply_tags`、`analyze_file` 成功路径写入、`VlFileOutcome::Annotated { tags }` 计数。真实网关 e2e 验证：10 个标签全部落到 `AI理解` 下并关联文件。
3. 用户确认 A：VL 标注用派生字段，不给 afiles 加列。
4. 缩略图 VL 标记独立角标。
5. 协议下拉必须做：Anthropic Messages + OpenAI Chat Completions。
6. VL 理解任务从主视图直接发起，不跳实验台；设置增加任务监视页。
7. 用户确认 B：任务历史必须按 run 分组持久化，采用 `vl_runs` + `vl_calls.run_id`（非单列方案）。
8. FileInfo 图片理解组放地点之后、源文件元数据之前。

---

## 10. 风险与需核验边界

- `/v1/models` 的 OpenAI 空 Key 请求可能 401；保留手填模型，不改变 chat configured 状态。
- 图片能力未知 provider 用三态呈现，不以 bool 隐式丢失模型。
- 每库持有 run history；删除 library 则 history 随 DB 删除。此行为在 UI 删除确认中应清楚可见。
- `ensure_fresh` 首次搜可能补齐大量 captions embeddings；真实库实测后决定限批/异步索引构建。
- 终态 run 保留策略/容量上限需实施时定值并测试，不能无限增长，也不能清理仍有 calls 的 run。

---

## 11. 三轮深度审查记录

### 第一轮：现状核验

- 官方 Search 是 LIKE 7 列 + CLIP，全局 `searchMode` 没 UI；CLIP 以图搜图/subjects 保留。
- `afiles.id` 是路径身份，hash 只用于 dedup；同内容不同路径为不同 ID。
- `/v1/models` magpie 有 modalities/context/output；vl-gw 是 capabilities/context_window 两类 schema。

### 第二轮：方案对代码审查

- 阻断：sidebar 顺序必须 order；新 Search 不可复用官方 planSearch/CLIP。
- 补齐 query source 专用分支、模型列表异步/协议、空 Key、本地 endpoint、system/response/embedding auth、搜索排除、run history 与 call 归组。
- A 选派生标记：AFile projection 尾端追加、QueryParams `EXISTS/NOT EXISTS`、global counts 独立扩展。

### 第三轮：持久化/多库/协议终审

终审发现的合同均已并入 §2/§6/§8：model list 鉴权与 401 fallback；UUID consumers；跨库旧 schema migration 与删除语义；run/call 外键和历史清理；client/panic/error/cancel 终态；固定启动 library；UUID 从前端 Number 转换彻底移除。

**终审结论**：按本文件 §1–§8 实现可行；必须把 §8 测试作为合并门禁。当前文档修订未改程序代码、未运行测试；仓库其他未提交改动保持原状。
