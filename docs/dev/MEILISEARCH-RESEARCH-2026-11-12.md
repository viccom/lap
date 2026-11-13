# 引入 Meilisearch 作为底层搜索服务的研究

日期：2026-11-12 ｜ 对象：[meilisearch/meilisearch](https://github.com/meilisearch/meilisearch) v1.56.2（源码克隆 `D:\MyCodes\AI-album\meilisearch`，commit `c25aa858b`）｜ 问题：引入它作为 lap 的底层服务，对精准本地搜索是否有改善 ｜ 环境：Windows + MSVC，debug 库 404 张照片、40 条标注语料、113 条评测查询（与 [SEARCH-RESEARCH-2026-11-12.md](./SEARCH-RESEARCH-2026-11-12.md) 同一套）

## 0. 结论摘要

1. **不引入，但要分两条通道说清**。同一语料、同一 113 条查询、同一 v1.56.2 二进制（§3.2、§3.4）：
   - **关键词通道**（`q`）：R@1 0.355 / R@3 0.409，110 条正例 **60 条落空**。根因是排序模型**强制查询第一个词命中**（源码 `query_graph.rs:346` + 双向受控实验），而 lap 的自然句首词（想找 / 找一张 / 帮我 / 摘…）几乎从不出现在照片描述里；无内置开关可解。
   - **向量通道**（`userProvided` + `zhipu/embedding-3` 2048 维）：R@1 **0.927** / R@3 **1.000** / **0 落空**。描述查询被完全修好——但这是**喂进去的 embedding 模型**的能力，与离线 dense 逐位相同；**不优于产品 FTS5 BM25（0.936）**。
   - **混合通道**（semanticRatio 0.5）：R@1 0.927，本语料下与纯向量逐条相同。描述查询的关键词分只有 0.16–0.33，早退条件不成立，排序由向量主导；短关键词查询（葡萄 0.939、蛋糕 0.894）则触发早退直接返回关键词结果。它是"关键词够好就用关键词，否则退到向量"，不是分数级融合增益。
   → 所以"Meilisearch 做不了语义搜索"是错的；准确说法是：**它的语义能力等于外部 embedding 模型的能力**，而同一个模型在本语料上并没有超过现有 BM25（dense 0.927、RRF 0.955，SEARCH-RESEARCH §4.1）。
2. 排开向量通道（那部分能力我们自己也能算）后，它自身"内容不同"的成分——charabia/jieba 词级中文分词——单独抽出实测为**负收益**（R@1 0.882 vs 产品 0.955，§3.1）；关键词排序是分桶序数规则（words/typo/proximity…逐层放宽，无词频、无 IDF），面向电商关键词式查询，不是文档相关度排序。
3. 它真正的强项（多租户、横向扩展、faceted 电商过滤、typo 容错的英文长词、云端 AI embedder 编排）全部不是 lap 的需求；它没有解决 lap 的已知痛点（负例拒绝、无词面重叠的硬改写——前者由可选 LLM 描述筛选承担，后者属于语义检索）。官方 Flickr 图像搜索 demo 的"视觉理解"来自 **AWS Nova Embeddings**，Meilisearch 负责向量存储与 ANN 检索（§3.4）；lap 的 CLIP 语义搜索是同一架构的本地版，差别只在规模。
4. 成本清单（§4）：336.8 MB 官方二进制 sidecar 进程 + 常驻内存（实测工作集 93 MB / 私有提交 2.5 GB）+ SQLite↔LMDB 双存储引擎同步 + 异步任务一致性 + 版本迁移；milli 作为嵌入式库依赖不可行（`publish = false`、无 semver 承诺、API 随大版本漂移）。
5. 许可：核心引擎 MIT；EE 部分（BUSL-1.1，**生产使用需商业授权**）只覆盖网络化/联邦搜索/动态排序规则等模块，默认 feature 不启用——lap 用不到，但引入整个服务时应明确只用社区版二进制。
6. 复评触发条件见 §5：语料上万且出现词面召回瓶颈时，按 SEARCH-RESEARCH §6.5 的顺序先试本地 embedding + RRF（复用 search-eval 回归），仍不够再评估独立搜索引擎。

## 1. 项目概况（v1.56.2，2026-11-12）

纯 Rust、单二进制、REST 服务。工作区结构与职责（`Cargo.toml` + `crates/`）：

| crate | 职责 |
|---|---|
| `crates/meilisearch` | HTTP 服务（actix-web），路由、认证、搜索编排 |
| `crates/milli` | **检索引擎本体**：索引管线、LMDB schema、排序规则、过滤/facet、向量、geo |
| `crates/index-scheduler` | 异步任务引擎：所有写操作（建索引/写文档/改设置）都是任务，排队、批处理、执行 |
| `crates/charabia`（外部依赖 0.10.0） | 分词：中文 jieba 词级切分（jieba-rs 0.10.3）、日文 lindera、语言检测 whatlang |
| `crates/heed` + LMDB | 存储层，内存映射，默认索引 map 上限 2 TiB（`option.rs:117`） |
| `arroy` 0.9 + `hannoy` 0.3 | 向量索引（LMDB 内，HNSW/树） |

部署形态：默认 `localhost:7700`；`production` env 强制 master key（≥16 字节），`development` 免；遥测默认开（`--no-analytics` 关）；索引内存上限默认取物理内存 2/3（`option.rs:1103`），索引线程默认取一半核心；Windows 构建不用 jemalloc（`cfg(not(windows))`，`crates/meilisearch/Cargo.toml:114`）。

许可拆分（`LICENSE` / `LICENSE-EE`）：**MIT AND BUSL-1.1**。BUSL-1.1 只作用于显式标记 "Enterprise Edition" 的文件——git grep 确认为：动态搜索规则、联邦/网络搜索、代理、S3/网络任务分发等（`crates/*/src/**/enterprise_edition*`、`proxy/`、`dynamic_search_rules*`）。单机检索内核（milli、charabia、REST 单机路径）均为 MIT。

## 2. 检索模型：与 lap FTS5 BM25 的本质差异

### 2.1 排序：分桶序数，不是 BM25

默认规则链 `["words", "typo", "proximity", "attributeRank", "sort", "wordPosition", "exactness"]`（`crates/milli/src/criterion.rs:121`，`crates/meilisearch-types/src/settings.rs:269`）。每条规则把候选集切成有序桶（如 typo 数、邻近代价、匹配词数），逐层收窄；**全文没有任何 TF-IDF/BM25 词频-文档频加权**（代理深读 + 本地复核 `milli/src/search/` 均未发现）。最终 `rankingScore` 是各规则序数 `rank/max_rank` 的混合进制折叠，归一到 [0,1]（`score_details.rs:670`、`:192`）。

`words` 规则：按"匹配查询词个数从多到少"分桶；`matchingStrategy` 决定放宽顺序——`last`（默认，从最后一个词开始丢）、`frequency`（先丢文档频最高的词）、`all`（全词强制，且不再加 words 规则）。**关键语义：无论 last 还是 frequency，查询第一个词永远是强制项**——首词在放宽计划中移除代价最大、排在末位，随即被 `res.pop()` 弹出（`crates/milli/src/search/new/query_graph.rs:346-405`；受控实验见 §3.2）。短语词强制保留；`-词` 为负词，直接从结果集减去。

lap 侧对照：FTS5 BM25（双字+单字+拉丁词混合 token，caption×5 列权重，短查询保护），评测 R@1 0.936 / R@3 1.000 / MRR 0.964 / ~7ms（SEARCH-RESEARCH §4.2）。

### 2.2 中文分词：jieba 词级 + 语言检测

- charabia 0.10.0，meilisearch 默认 feature `all-tokenizations`（`crates/meilisearch/Cargo.toml:143`）启用 chinese/japanese/korean/thai 等；中文走 **jieba 词典词级切分**（Cargo.lock 锁定 jieba-rs 0.10.3，亲验）。
- 语言来源：查询参数 `locales` / 索引设置 `localizedAttributes`（按属性模式配语言），否则 whatlang 自动检测。
- 可选拼音归一化 feature（默认关）。

lap 侧对照：自研双字+单字+拉丁词混合 token。中文 IR 文献与本项目实测都表明 bigram 与词级效果相当（SEARCH-RESEARCH §1.2；本报告 §3.1 用 lap 语料复核：词级更差）。

### 2.3 typo 容错与前缀

- 默认词长 ≥5 字符容忍 1 typo、≥9 容忍 2 typo，首字母错按 2 个计（`index.rs:48-49`、`compute_derivations.rs:101`，均亲验）。**推论：中文词 2–4 字 → 默认 0 typo 容错**，typo 容错对中文查询基本不生效；对 ≥5 字符的英文词有效。
- 前缀：只有查询**最后一个词**做前缀匹配（预建 `word_prefix_docids`）。lap 的单字 token 在中文里天然提供了逐字前缀效果。

### 2.4 向量与混合检索

- embedder 源：`openAi`（默认）/`huggingFace`/`ollama`/`rest`/`userProvided`/`composite`（`vector/settings.rs:1582`）；`userProvided` 可完全离线（文档带 `_vectors`，查询带 `vector`）。
- hybrid：`semanticRatio` 融合，文本分 ×(1-r) 对语义分 ×r（`hybrid.rs`）；文本侧分数 ×(1-r) ≥ 0.45 时直接早退，不请求向量（`hybrid.rs:389`，亲验）。
- lap 侧对照：稠密向量/RRF/精排在同一语料上已实测无增益（dense R@1 0.927、RRF 0.955 = BM25 0.955、rerank 0.945，SEARCH-RESEARCH §4.1）——这给任何 hybrid 方案在这套语料上的上限画了线。

### 2.5 能力面（供完整性）

过滤语法（`filter-parser`）、facet 分布/搜索、sortableAttributes、geo（R-tree + GeoJSON cellulite）、文档 rhai 脚本编辑（实验特性）、`maxTotalHits` 默认 1000（`settings.rs:31`，亲验）、`searchCutoffMs` 默认 1500ms（超时降级返回部分结果并标记 degraded）。

## 3. 实测

### 3.1 单成分探针：jieba 词级分词（"借想法，不借服务"）

Meilisearch 对中文真正的差异点是 jieba 词级 token。把它单独接入现有评测（`jieba_probe.py`，复用 run_eval 的 BM25 与 evaluate，同一 113 条查询）：

| 方法 | R@1 | R@3 | R@5 | MRR@10 | nDCG@10 | misses | ms/查询 |
|---|---|---|---|---|---|---|---|
| bm25_jieba（纯词级） | 0.882 | 0.964 | 0.973 | 0.925 | 0.939 | 0 | 0.14 |
| bm25_jieba_char（词级+单字+拉丁） | 0.936 | 0.991 | 1.000 | 0.966 | 0.975 | 0 | 0.23 |
| bm25_jieba_bigram（词级+双字+单字+拉丁） | 0.955 | 0.991 | 1.000 | 0.973 | 0.980 | 0 | 0.3 |
| **bm25_mixed（产品现状：双字+单字+拉丁）** | **0.955** | **1.000** | **1.000** | **0.974** | **0.981** | **0** | 0.21 |

结论：**词级分词在 lap 语料上是负收益**。纯词级 R@1 掉到 0.882（分词把 `小孩/男孩`、`藤架/葡萄棚` 这类词面差异固化成不同 token，反而失去双字重叠的容错）；任何组合都追不上现状的 R@3 = 1.000。这与中文 IR 文献"双字与词级相当、短查询下混合更优"一致（SEARCH-RESEARCH §1.2），也再次验证产品现有 token 策略已在局部最优点附近。

### 3.2 整体回放：meilisearch v1.56.2 vs 产品 FTS5

从源码构建官方同版本二进制（全部分词 feature，无 mini-dashboard），40 篇语料建索引（searchableAttributes = text, name），113 条查询原样回放（`meili_replay.mjs`）：

官方 release 二进制（`meilisearch-windows-amd64.exe`，336.8 MB，`--version` = 1.56.2）起本地服务（仅回环、development env、关遥测），40 篇语料建索引（searchableAttributes = text, name），113 条查询原样回放（`meili_replay.mjs`）。为排除 charabia 语言自动检测的影响，测了三档配置：A 全自动（whatlang）、B 索引侧 `localizedAttributes = ['*']→zho`、C 再加查询侧 `locales = ['zho']`：

| 方法 | R@1 | R@3 | R@5 | MRR@10 | nDCG@10 | misses | ms/查询 |
|---|---|---|---|---|---|---|---|
| meili A：全自动 | 0.355 | 0.409 | 0.409 | 0.384 | 0.397 | **60** | 2 |
| meili B：索引侧固定 zho | 0.355 | 0.409 | 0.409 | 0.384 | 0.397 | 60 | 2 |
| meili C：查询侧也固定 zho | 0.355 | 0.409 | 0.409 | 0.384 | 0.397 | 60 | 2 |
| **产品 FTS5 BM25（同查询集回放）** | **0.936** | **1.000** | **1.000** | **0.964** | **0.973** | **0** | 7 |

三档完全一致——问题不在语言检测。按类型（三档相同）：keyword R@1 0.688（9 miss）、paraphrase 0.438（11 miss）、sentence **0.031（31/32 全部落空）**、hard 0.100（8 miss）、user-synonym 0（1 miss）、user-keyword 0.5。三个负例全部 0 命中——但这是首词恰好不命中的副作用，不是语义拒绝（见下）。

**根因：默认 `matchingStrategy: last` 下，查询第一个词是强制项。** 源码证据（`query_graph.rs:346-405`）：移除代价 `cost = 1 + last − term_idx`，首词代价最大、排在放宽计划末位，随后 `res.pop()` 把这一项弹出——放宽永远到不了首词。受控实验（构造索引 tok-probe）双向验证：

| 查询 | 返回 | 说明 |
|---|---|---|
| `桌上 剪刀` | [2] | 只含「剪刀」的文档 1 被**整体排除** |
| `剪刀 桌上` | [2,1] | 换序后文档 1 出现——首词决定谁能进结果集 |
| `16 nonexistent`（拉丁，lap 语料） | 2 篇 | 首词命中即可 |
| `nonexistent 16`（拉丁） | **0 篇** | 首词不命中 → 全部落空 |

对照 lap 评测集：sentence 查询首词是「想找 / 找一张 / 帮我 / 有没有」、负例「摘葡萄的小女孩」首词是「摘」——这些词在照片描述里不存在，命中集合直接为空；关键词查询「男孩 剪刀 绿葡萄 葡萄大棚」只剩首词「男孩」命中的 16 篇，目标文档（分词为「小男孩」整词，不含独立「男孩」，但含「剪刀」「绿葡萄」）被整体排除。拉丁对照证明这与 CJK 无关，是排序模型的固有语义；`matchingStrategy: all` 要求全词命中（更严），`frequency` 共用同一个 `res.pop()` 同样保护首词。让全部词可选只有客户端改写查询一条路（去句首功能词、重排词序），meilisearch 无内置开关。

探针（C 档）：前缀「葡」→[1,2] ✓；中文短词 typo「葡荀」→[]（2 字词默认 0 typo，§2.3）✓；词序「草地 男孩」→目标仅排第 5；文件名带错字「C (166).jpg」→ c/jpg 词面广泛命中。负例（三档一致）：三个负例全部 0 命中。

> 结论：meilisearch 的查询模型面向「首个词即核心词」的关键词搜索（电商用户这么输入）；lap 的查询形态是中文自然句 + 口语化关键词混合，二者不相容，且这是排序模型层面的不匹配，调参不可解。

### 3.3 资源占用（本机 Windows，40 篇语料）

| 项目 | 实测值 |
|---|---|
| 官方二进制体积 | 336.8 MB（353,019,392 字节，含 mini-dashboard 与全部分词 feature） |
| 启动到健康检查通过 | 73 ms |
| 建索引（40 篇，含任务轮询） | 61–247 ms |
| 搜索延迟（113 查询均值，HTTP 回环往返） | ~2 ms |
| 进程工作集（回放完成后） | 93.1 MB |
| 进程私有内存（提交量） | 2583 MB（LMDB 映射提交行为所致；物理占用以上一行 93 MB 为准） |
| 索引磁盘体积 | 1.1 MB（40 篇文档） |

注：lap 进程内 FTS5 的 ~7ms 是纯 SQL、无进程间往返；上表 2ms 是本机 HTTP 回环的最优情形，Tauri sidecar 场景还要叠加拉起 / 健康等待 / 关闭。

### 3.4 补测：向量与混合通道（回应官方 Flickr 图像搜索 demo）

官方 demo（[flickr.meilisearch.com](https://flickr.meilisearch.com)）宣传的是「自然语言描述 → 图片」。需要先澄清分工：**理解视觉内容的是 Amazon Nova Embeddings（AWS 外部多模态模型），Meilisearch 只负责存向量、ANN 检索和 similar API**。所以正确的对照方式是：用与 lap 评测同一批向量，测它的向量/混合通道。

配置：`userProvided` embedder，2048 维（`zhipu/embedding-3`，与 SEARCH-RESEARCH §4.1 的 dense 基线同一模型），40 篇文档带 `_vectors` 建索引（407 ms），113 条查询原样回放（`meili_vector_replay.mjs`）：

| 通道 | R@1 | R@3 | R@5 | MRR@10 | nDCG@10 | misses | ms/查询 |
|---|---|---|---|---|---|---|---|
| meili keyword（§3.2） | 0.355 | 0.409 | 0.409 | 0.384 | 0.397 | 60 | 2 |
| **meili vector（semanticRatio 1.0）** | **0.927** | **1.000** | **1.000** | **0.964** | **0.973** | **0** | 3 |
| meili hybrid（semanticRatio 0.5） | 0.927 | 1.000 | 1.000 | 0.964 | 0.973 | 0 | 3 |
| 产品 FTS5 BM25 | 0.936 | 1.000 | 1.000 | 0.964 | 0.973 | 0 | 7 |
| 离线 dense（同一模型 + 余弦，SEARCH-RESEARCH §4.1） | 0.927 | 1.000 | 1.000 | 0.964 | 0.973 | 0 | 9 |

三条结论：

1. **向量通道确实修好了描述查询**：R@1 0.927 / 0 落空，sentence 类从 0.031 变成 1.000。此前仅凭关键词通道的 0.355 否定"语义搜索"是方法不完整，已修正。
2. **它只是把外部 embedding 的结果原样排出来**：与离线 dense 逐位相同（直接用余弦算就是这个数）。Meilisearch 的增量是 ANN 索引与 REST 封装，不是理解能力。用 `similar` API 做"以图搜图"同理——前提是已经有图像向量。
3. **混合通道在本语料等于纯向量**：semanticRatio 0.5 下，描述查询的关键词分只有 0.16–0.33，`score×(1−ratio) ≥ 0.45` 的早退条件不成立，排序由向量主导；反过来，短关键词查询（"葡萄"0.939、"蛋糕"0.894）会触发早退、直接返回关键词结果，向量不参与。即它的 hybrid 是"关键词够好就用关键词，否则退到向量"，不是真正的分数级融合。

负例（三条）在向量/混合通道下都会返回 3 条"最像"的照片（如「摘葡萄的小女孩」→ C (17)/C (16)），不做拒绝——与 BM25 一致，是排序检索的固有语义。

对 Flickr demo 的正确解读：它证明的是 **10^8 量级 ANN 检索 + 外部多模态 embedding 的服务化组合**，不是"Meilisearch 自带图像理解"。lap 的语义搜索是同一架构的本地版（CLIP 多语言文本编码器 + 图像向量 + 余弦），但实测差距很大：lap 的 CLIP 语义在中文 110 条上 R@1 仅 **0.255**、英文 40 条 0.400（SEARCH-RESEARCH §4.5），而同语料的 BM25 是 0.936。差距来自模型代际（CLIP ViT-B/32 2021 vs Nova）、语料规模（404 张 vs 1 亿）与查询形态（20–60 字中文组合描述 vs 4 词英文场景）。差别还体现在规模：404 张用暴力余弦是毫秒级，10^5–10^6 张时 ANN 才有意义；且把照片送到 AWS 做 embedding 与 lap 的本地优先前提冲突。

## 4. 集成路径评估

### 路径 A：meilisearch 作为 sidecar HTTP 服务（Tauri app 内拉起子进程）

成本（全部为 lap 侧新增，且无对应收益）：

1. **进程生命周期**：app 启动拉起、健康等待、崩溃后的孤儿进程回收、退出时优雅关闭；端口固定 7700 会撞其他软件，动态端口又要传配置。
2. **双存储引擎同步**：标注/标签/人名的唯一事实源在 SQLite；每条变更（增删改、重理解、模板切换后的全量重建）都要推送 HTTP 任务并等待完成（meilisearch 写入是异步任务）；崩溃时序、重试、对账都要自己写。
3. **升级与迁移**：meilisearch 大版本间 DB 格式迁移由它自己管理，跨版本降级要 meilitool；嵌入桌面 app 后每次升级引擎都要考虑用户数据迁移。
4. **体积与内存**：官方二进制 336.8 MB 起步；进程工作集 93 MB、私有提交 2.5 GB（LMDB 映射行为，§3.3），外加 actix/tokio 常驻。
5. **查询路径变长**：进程内 FTS5 一次 SQL（~7ms）变成 HTTP 往返 + JSON 序列化。
6. **向量通道的额外依赖**：Meilisearch 不产生 embedding。要么把图片/描述送到外部模型（官方 demo 用 AWS Nova，照片离开本机），要么在 lap 侧自己算好再写 `_vectors`——后者等于保留现有 CLIP 管线，只把本地余弦换成一次 HTTP 调用。

收益核对（逐项对照 lap 已知问题）：排序质量——§3.2 实测无超益；中文分词——§3.1 实测负收益；typo——中文默认不生效；前缀——单字 token 已覆盖；faceted 过滤——lap 用 SQL 已有；hybrid 向量——SEARCH-RESEARCH 已证 dense/RRF/rerank 在本语料无增益；负例拒绝与硬改写——词面引擎（含 meilisearch）都解决不了，已由可选 LLM 描述筛选与语义搜索承担。**结论：不采用。**

### 路径 B：milli 作为嵌入式 Rust 依赖

`crates/milli/Cargo.toml` 标 `publish = false`——只能 git 依赖固定 commit；公开 API（`Index`、`update::*`、`SettingsDelta`…）无 semver 承诺，随大版本漂移；`fuzzers/src/bin/fuzz-indexing.rs` 证明了脱离 HTTP 单用 milli 可行，但任务调度、设置变更、重建策略全要自己长出来。等于**把一个搜索引擎的运维复杂度搬进 lap 进程内**，换来的是 §3.1/§3.2 已证伪的排序差异。仍会引入 LMDB 双存储。**结论：不采用。**

### 路径 C：不引入，保持现状（推荐）

FTS5 BM25（召回排序）+ 图像向量（语义搜索）+ 可选 LLM 描述筛选（负例拒绝）覆盖了现有失败模式；§3.1 同时否决了最有吸引力的"借鉴点"（jieba 词级 token）。

## 5. 什么情况下值得复评

按触发条件排序，触发前先用 `docs/dev/search-eval/` 回归，不凭感觉换引擎：

1. **语料显著变大**（标注 >5k 篇）且 FTS5 查询延迟或索引维护出现实际问题时——先做本地 embedding + RRF（SEARCH-RESEARCH §6.5），这是纯 Rust crate 级改动，不动架构。
2. **出现英文/拼音查询的真实需求**（文件名、外文人名变多）时——meilisearch 的 typo 容错（≥5 字符词）与前缀 db 才有意义；可以先在 FTS5 上做拉丁词的编辑距离近似。
3. **产品形态变成多库/多设备/服务化**（索引共享、多端写）时——meilisearch 的异步任务与多索引编排才开始对价。
4. **图库达到 10^5–10^6 张**且暴力余弦成为实测瓶颈时——这时才需要 ANN（Meilisearch/Arroy 或其他向量库）；在那之前，CLIP + 本地余弦的复杂度可以接受。

无论是哪一条，都先用 `docs/dev/search-eval/` 回归；向量通道的补测脚本 `meili_vector_replay.mjs` 可直接复用（只需改 embedder 配置）。

## 6. 复现

```bash
# 1) 启动官方二进制（development env 免 master key；仅绑回环；关遥测）
D:/LapPortable/meilisearch-windows-amd64.exe --db-path <临时目录>/data --http-addr 127.0.0.1:7700 --no-analytics --log-level ERROR

# 2) 回放评测集（建索引 + 113 条查询 × 三档 locale 配置 + 指标对比 + 探针）
cd D:/MyCodes/AI-album/lap/docs/dev/search-eval
node meili_replay.mjs --url http://127.0.0.1:7700   # 写出 meili_ranks.json

# 3) jieba 单成分探针（需 pip install jieba）
python jieba_probe.py

# 4) 首词强制语义受控实验（tok-probe 索引，构造 {id,text} 文档）
#    对比 `桌上 剪刀` 与 `剪刀 桌上` 的返回差异，见 §3.2
```

## 7. 来源

- 源码克隆 `D:\MyCodes\AI-album\meilisearch`（v1.56.2，commit `c25aa858b`，2026-11-12）：排序规则 `crates/milli/src/criterion.rs:121`、score 折叠 `crates/milli/src/score_details.rs:670`、首词强制 `crates/milli/src/search/new/query_graph.rs:346-405`、typo 阈值 `crates/milli/src/index.rs:48`、hybrid 早退 `crates/milli/src/search/hybrid.rs:389`、默认 feature `crates/meilisearch/Cargo.toml:143`、中文分词 charabia-0.10.0 `src/segmenter/chinese.rs`（jieba cut 无 HMM + 词典贪心双/三字切分，单字兜底）、EE 文件清单（git grep "Enterprise Edition"）
- `LICENSE`（MIT AND BUSL-1.1）与 `LICENSE-EE`（"Production use … requires a commercial license"）
- 官方 release v1.56.2 Windows amd64 二进制（GitHub Releases；实测所用二进制由用户提供于 `D:\LapPortable\meilisearch-windows-amd64.exe`，`--version` 确认 1.56.2 / commit 8c7bfcfa）
- 官方图像搜索 demo：[Image Search 文档](https://www.meilisearch.com/docs/resources/demos/flickr)（Flickr 1 亿张；Amazon Nova Embeddings 提供多模态向量，Meilisearch 负责向量存储、ANN 与 similar API）
- 相邻结论：[SEARCH-RESEARCH-2026-11-12.md](./SEARCH-RESEARCH-2026-11-12.md)（BM25/dense/RRF/rerank 在同一语料的实测）
