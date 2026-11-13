# lap 现状核实结论（CLIP 职责 / 文件唯一性 / 官方搜索页机制 / 网关模型列表）

日期：2026-11-13
方法：源码逐行核实 + debug 库只读探针 + 网关端点实测
用途：后续所有搜索与图片理解改动的**事实基线**；避免重复调研、避免凭印象决策。

---

## 1. CLIP 在 lap 中的职责

### 1.1 模型结构

- 内置两个 ONNX：`resources/models/vision_model.onnx`（图像塔，89MB）+ `text_model.onnx`（文本塔，64MB）+ `tokenizer.json`（OpenAI CLIP 49408 BPE 词表）。
- 二者是**同一个 CLIP ViT-B/32 的两个塔，共享同一向量空间**——这是「文本查询检索图像向量」成立的前提。
- 多语言包（519MB）**只替换文本塔**，视觉塔始终内置；切换时用 `ensure_embedding_dimensions_match` 强制校验「文本维 == 视觉维」，不等则拒绝并回滚（`t_ai.rs:319-332`）。
- 预处理：224×224 + OpenAI CLIP mean/std（`t_ai.rs:489-511`）。

### 1.2 向量存储

- 位置：**`afiles.embeds` 单列 BLOB**（`t_sqlite.rs:10710`），f32 little-endian 平铺。
- **无 model / dims 字段**（对比 `caption_embeds` 有 model/dims）——维度只能从 BLOB 长度反推；维度不符时余弦返回 0.0 被阈值静默丢弃（`t_sqlite.rs:7533-7535`）。
- 写入：后台两条路径（专辑索引 `t_utils.rs:4120`、文件夹 mtime 同步 `t_utils.rs:3126`），信号量串行（`Semaphore::new(1)`）；缩略图优先编码，失败回退原图；**幂等不覆盖**（已有 embeds 直接返回）。
- 失效：文件内容变化（size/mtime）→ 置空并重算（`t_sqlite.rs:4655`）；文件行删除 → 随行消失；**切换文本塔不清空图像向量**（图像向量本就该保留）。

### 1.3 三个实际用途

| 用途 | 入口 | 阈值 | 说明 |
|---|---|---|---|
| **以图搜图（相似照片）** | `Content.vue:8650 enterSimilarSearchMode` → `search_similar_images` | 0.7（`config.similarImageSearchThreshold`） | 图像↔图像，CLIP 强项 |
| **subjects 智能相册** | `queryDispatch.js:175-190 LIB_ITEM.SUBJECTS` → `getImageSearchFileList` | 0.25（`smartTagSearchThreshold`） | 英文 CLIP 提示词分类 |
| **统一搜索 vector 腿** | `Content.vue:8175 getUnifiedSearchFileList` | 0.26–0.32（设置项 `imageSearchThresholds`） | 文本查询 → 文本塔 → 图像向量余弦 |

- 检索机制：暴力全表扫描 + 余弦，**无 ANN、无索引、无 LIMIT**、**不返回分数**（只返回完整 `AFile`）。
- 文本塔**仅在查询串非空时**被调用（`get_query_embedding`，`t_sqlite.rs:7303-7321`）；图像塔只负责入库。
- `generateEmbedding` 在前端是**死代码**（`api.js` 定义 + `Content.vue:796` import，无调用点）。

### 1.4 与 caption 文本向量的边界

- 两张表、两个向量空间、两个命令：`afiles.embeds`（CLIP 图像空间）vs `caption_embeds`（BGE/云端文本空间，`t_embed.rs:35`）；`search_similar_images` 只读前者，`search_captions` 只读后者，**无交叉查询、无分数融合**。
- 主视图目前是把两者「按 id 去重拼接」，不比较分数。

> **结论**：用户约束「完全不使用图像向量」针对的是**文本搜索**。以图搜图与 subjects 智能相册走图像↔图像/图像↔英文提示词，是 CLIP 强项，与该约束不冲突，应完整保留。

---

## 2. 图片文件的唯一性

### 2.1 afiles 的约束（debug 库 sqlite_master 实测）

```
CREATE TABLE afiles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_id INTEGER NOT NULL,
    name TEXT NOT NULL, ... , embeds BLOB, content_identifier TEXT, ...
);
CREATE UNIQUE INDEX uidx_afiles_folder_id_name ON afiles(folder_id, name);
```

- **主键 = 自增 id**；**全表唯一约束只有 `uidx_afiles_folder_id_name`（同文件夹内文件名唯一）**。
- **afiles 里没有 path 列**（路径 = `afolders.path || '/' || afiles.name` 派生），**也没有 hash 列**——结构上不可能用 hash 当 ID。
- `content_identifier`（Live Photo 内容 ID）只有普通索引，非唯一，仅用于图↔视频配对。

### 2.2 四种身份概念

| # | 身份 | 载体 | 唯一性 | 用途 |
|---|---|---|---|---|
| 1 | **数据库行 id** | `afiles.id` | 全局唯一，但**同一内容可有多个 id** | 全系统主身份（网格/标签/人脸/VL 标注/缩略图/FTS/语义向量全部以它为外键） |
| 2 | **内容哈希** | `file_hashes.hash`（blake3 整文件）+ `file_size` | hash 列**无唯一约束**（同 hash 多行）；`duplicate_groups` 用 `UNIQUE(hash,file_size)` 分组 | 仅「字节级完全相同」去重 |
| 3 | **视觉近似** | `afiles.embeds` + `hnsw_rs`（`t_similar.rs`） | 非身份键 | 相似照片分组（阈值 0.97/0.93/0.9/0.85） |
| 4 | **语义向量** | `caption_embeds`（file_id 主键，带 model/dims） | file_id 主键 | 用文字搜图片描述 |

- 哈希**只在用户手动跑去重扫描时写入**（`t_dedup.rs:260`），入库流程从不查它；不跑去重则概念 2 根本无数据。
- 概念 2 与 3 **互斥**：相似扫描排除已在 exact 组内的文件（`t_similar.rs:118`）。
- 无感知哈希（pHash/dHash）实现，完全重复与视觉近似是 `DedupPane` 的两个 tab。
- 前端**不做展示合并**：同内容两份文件在网格里并排显示为两个格子，去重只做角标标记（KEEP/DUP）。

### 2.3 id 稳定性

| 场景 | id 变化 |
|---|---|
| 同文件夹改名 | **保持**（inode / NTFS FileIndex 检测，`t_utils.rs:2603`） |
| 应用内移动（MoveTo） | **保持**（只 UPDATE folder_id） |
| **应用外跨文件夹移动** | **变**（inode 映射按文件夹构建，跨目录匹配不上 → 删旧插新） |
| 同内容不同路径 | **两个 id**（2 行 afiles + 2 行 file_hashes + 1 个 duplicate_group，从不合并行） |

> **对图片理解的含义**：VL 标注与语义向量都挂在 `file_id` 上，文件在应用外跨文件夹挪动后**标注会失联**。这是 lap 官方路径身份架构的固有行为，非本次引入；批量标注前需知晓。

### 2.4 与 homepix 的差异

| | lap | homepix |
|---|---|---|
| 身份 | 路径身份（folder+name → 自增 id） | 内容身份（hash 唯一） |
| 同内容多路径 | 多 id 并存，去重面板人工处理 | 同一资产 |
| 移动 | 跨文件夹 = 换 id，派生数据（标注/向量/FTS/人脸/缩略图）全部失联 | hash 稳定 |

---

## 3. 官方源搜索页的机制（截图实测 + 源码核实）

### 3.1 入口链路

侧栏放大镜图标 → `SIDEBAR.SEARCH` → `ImageSearch.vue` 面板（输入框 + 历史记录）→ `Content.vue:8116 getUnifiedSearchFileList`。

### 3.2 官方界面永远跑 `all` 档

- 全前端 `searchMode` 仅 3 处引用：`configStore.js:206` 定义（`'all'`）、`Content.vue:8119` 读取。**无 setter、无切换 UI**（设置页只有 CLIP 文本塔切换，不是档位）。
- 结论：管线里预埋的 FTS 腿（precise）与 vector 单档（semantic）在官方界面上**从未被执行过**。

### 3.3 `all` 档的两路并行

| 路 | 机制 | 覆盖 |
|---|---|---|
| **词面路** | SQL `LIKE '%text%' COLLATE NOCASE`（`t_sqlite.rs:5500-5514`） | **仅 7 列**：`name`（文件名）、`comments`、`e_title`、`e_headline`、`e_artist`、`e_copyright`、`e_software`——**不含 tags、people、图片描述** |
| **CLIP 向量路** | 查询文本 → CLIP 文本塔（默认英文塔；装了多语言塔且查询含非拉丁字符才切）→ 与 `afiles.embeds` 余弦 | 全库有向量的图片 |

结果按 id 去重后分两组展示（「词面命中」+「语义命中」），自动打开第一张。

> 这解释了中文搜索体感差的完整原因：词面路对中文描述/标签完全盲区；向量路是英文 CLIP（zh R@1 实测 0.255，见 SEARCH-RESEARCH §4.5）。我们做的 BM25 与描述向量语义**都不在这条链上**。

---

## 4. 网关模型列表端点（实测）

`GET {gateway}/v1/models`（**需要 API Key**，无 key 返回 401）：

- 返回 **57 个模型**，每个模型含：`id`、`display_name`、`magpie_label`、`modalities.input`（text/image/video/pdf）、`context_length`、`max_output_tokens`、`reasoning`、`pricing`、`owned_by`、`supported_parameters`。
- **52 个支持图像输入**（可直接作 VL 理解模型），**5 个纯文本**（如 `zhipu/glm-4.6`，当前重排默认值）。
- 端点：`/v1/models`、`/models` 均可用；`/v1/messages/models` 404（magpie 404 提示其服务面为 `/v1/chat/completions`、`/v1/responses`、`/v1/messages`、`/v1/embeddings`、`/v1/rerank` 等）。
- **硬约束：`/v1/models` 列表中没有 embedding 模型**（`zhipu/embedding-3` 不在其中，尽管 `/v1/embeddings` 端点可用）→ 云端嵌入模型**无法**从该端点自动获取，仍需手填 + 默认值提示。
- 探针：`docs/dev/search-eval/gateway_models_probe.py`（不打印密钥）。

---

## 5. 对本次改进的含义

1. **CLIP 保留范围可精确限定**：只换「统一搜索的 vector 腿」；`search_similar_images` 本身服务以图搜图与 subjects，完整保留。
2. **官方搜索页是 `all` 档（LIKE 7 列 + CLIP）**，我们的新入口应作为**独立侧栏项**并存，而不是改造官方面板——这样官方界面「原有功能完全保留」成立。
3. **缩略图 VL 标记最省的实现是给 afiles 加一列**（`vl_annotated`），随文件行下发；同一列还能支撑「VL 已标注/未标注」筛选（需求 4b）与需求 5。
4. **模型下拉可落地**（网关 `/v1/models` 已实测），但**云端嵌入模型除外**。
