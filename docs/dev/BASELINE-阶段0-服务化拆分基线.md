# BASELINE · 阶段 0 测试基线（服务化拆分）

日期：2026-11-15
分支：`develop`（基线提交见文末）
用途：[ARCH-前后端分离与服务器化方案.md](./ARCH-前后端分离与服务器化方案.md) 阶段 0 验收物。后续每阶段收尾必须复跑本文命令，**失败集合相对本基线无新增**。

## 1. 基线结果

| 门禁 | 结果 | 复现命令 |
|---|---|---|
| Rust 全量 | **561 passed / 0 failed / 9 ignored**（exit=0） | `source docs/dev/build-env.sh && cd src-tauri && cargo test --bin Lap` |
| 前端全量 | **41 文件 / 425 tests / 425 pass / 0 fail** | `cd src-vite && node --experimental-vm-modules --test tests/*.test.mjs` |
| 前端构建 | exit=0（仅 chunk 体积警告） | `cd src-vite && pnpm build` |

- 已知 flaky 池：**清空**（原三成员全部根因修复，见 §2）。
- 9 个 ignored 均为需真实数据/网关/样本的手动测试，与基线无关：`t_dedup` 外部样本 keep rules、`t_fts` 真实库检索探针、`t_image` 离线 RAW 预览 ×3、`t_sqlite` 镜头回填、`t_vl` 真实网关 ×3。
- 注：基线计数曾记 556，后因人脸优化批（`be3a180c`/`3c39bf21`）新增 5 个门控/对齐探针测试升为 **561**（0 删除、0 新增失败、ignored 仍 9）。**阶段 0 的验收标准是「失败集合无新增」，该标准仍成立**；后续阶段以 561 为对照值。

## 2. 本阶段修复（全部有红→绿证据）

### 2.1 Rust `t_storage::checkpoint_db`（上游 #354 遗留平台 bug）
- 症状：`copy_failure_keeps_source_and_cleanup_failure_is_reported` 在 Windows 确定性失败（`assert!(checkpoint_db(...).is_err())` 拿到 Ok）。
- 根因（本机 rustc 探针实证）：Windows 对「文件当目录」路径报 `ERROR_PATH_NOT_FOUND(3)`，std 映射为 `NotFound`，`try_exists` 折叠成 `Ok(false)` → `checkpoint_db` 误判「库不存在」静默跳过。Linux 是 `ENOTDIR` → `Err`，故该测试只在 Linux 语义下成立。
- 修复：`Ok(false)` 分支增加父目录校验（父目录是文件 → 报错），两平台行为统一为 Err；**上游测试断言零改动**通过。

### 2.2 Rust `metadata::files::transfer_batch` × 2（FS 时间刻竞态）
- 症状：热机连续失败（快照后注入的新文件检测不到：conflicts 返回 `[]` / xmp NotFound），冷机偶发通过——原「flaky 池」成员。
- 根因：`Cache::ensure_directory`（files.rs:187）用**目录 mtime 戳**做缓存失效；Windows 文件时间在 ~15.6ms tick 内合并，测试在同一 tick 内注入外部变更 → 戳不变 → 目录列表被缓存命中。
- 修复：测试在每次注入外部变更前 `sleep(25ms)` 跨过 tick（仓库已有先例：t_config.rs:617、t_cmds.rs:335）。产品逻辑不动——真实用户操作间隔远大于 tick。

### 2.3 前端 7 文件 27 测试（CRLF 工作区 vs LF 锚）
- 根因：本机 `core.autocrlf=true`，工作区文件全部 CRLF；而测试的 import 剥离正则（4 种写法）**全部以 `\n` 结尾**，在 CRLF 下失配 → `new Function` 遇到未剥离的 import 直接 SyntaxError（即 F0 记录的 5 个加载失败 + 各文件内部失败）。上游 CI 是 LF 检出且从不跑前端测试，故从未暴露。
- 修复：读取处统一追加 `.replace(/\r\n/g, '\n')`（与上游 LF 检出行为完全一致）。涉及：afAreaPersistence、albumRecovery、cloudFileRefresh、offlineCatalog、xmpWriteback、fileInfoMetadata、filterSelection。

### 2.4 前端 offlineCatalog 1 项（位置索引漂移）
- `menu.value[2].disabled` 断言：测试写作时（9173707f）`[2]` 是 external-app 分组；我们 f707fc68 新增的 `vl-analyze-selected` 把它挤到 `[1]`。
- 修复：改按子项 `open-external-app:editor` 反查分组，顺序无关。产品行为不变。

### 2.5 前端 filterSelection 3 项（切片锚点意外，worktree 考古定位）
- 考古链：e2471062（上游 11-02 测试）全绿 → **f707fc68（我们 11-13）起 5/3 失败**（26 个 rev 逐点实测）；f707.diff 中切片相关函数零改动 → 复刻测试切片提取对比：`watch-note` 切片从 1400 字符塌缩到 31。
- 根因：该切片靠「`/// watch for file list changes` 注释恰好挂在 query-context watcher 上方 + 结束锚 `\nwatch(\n  () => [` 匹配不到 `() => JSON.stringify`」的巧合吞入 1400 字符；f707 把注释移到数组 watcher 上，切片只剩注释 → 装置丢失 `pendingScopeSelectionChange` 设置逻辑。**产品无回归**（watcher 在 Content.vue:6690 正常运行）。
- 修复：切片锚点改为意图明确的 `/// watch for query-context changes` → `\n/// watch for file list changes`。

## 3. 新增测试规则（沉淀进 AGENTS.md）

1. 前端测试读源码文件：**必须在读取处归一化 CRLF**（`.replace(/\r\n/g, '\n')`）。
2. 锚点不得依赖：跨行 `\n` 精确形态、菜单/列表位置索引、注释恰好挂在某代码上方的巧合。
3. 涉及文件系统时间戳判定的测试，注入变更前跨 tick（`sleep ≥25ms`）。

## 4. 基线提交

- `95184892` fix(face)（人脸批修复，其全量结果即本基线前的 553/3 状态）
- `cae90115` docs(dev) 服务化方案三件套
- `913faf4d` fix(test) 本阶段全部修复（基线即以该提交状态测得）
