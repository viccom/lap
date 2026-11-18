# 端到端（E2E）测试清单与复现手册

> 目的：**浏览器版（Web 客户端 / lapd）为主线**的可测功能全清单。日后任何代码改动后，
> 按本清单逐条复跑，即可确认"可测功能都已覆盖并验证通过"，并把失败集合与本文档登记
> 的基线对照（不得新增失败）。
>
> 本文件不是"设计文档"，是**验收脚本 + 结果台账**。每条的"通过判据"必须给出可观察证据
> （页面文本 / 网络请求 / 磁盘字节 / DB 行），不接受"看着对"。
>
> 建立于 2026-11-17（阶段 4 批次 5d）；**2026-11-18 同步上游 v0.4.0 后增补 §13（RAW 配对 /
> 元数据 / 标签树 / 去重规则 / 批量重命名 / 云文件 / 迁移 v31），并修正二进制名漂移
> （`lapd.exe`→`lappp-server.exe`、`Lap.exe`→`lappp.exe`）**。锚点随代码漂移，以实际代码为准。

---

## 0. 测试环境搭建（每次复跑前）

### 0.1 前置

| 项 | 要求 |
|---|---|
| 构建 | `cd src-vite && npm run build:http`（`VITE_LAP_TRANSPORT=http`；**必须最后跑，dist 是共享 outDir**） |
| 服务端 | **`src-tauri/target/debug/lappp-server.exe`**（debug 构建；`cargo build -p lappp-server`），或用 `scripts/build-windows.ps1` 产出的 `lappp-server.exe`（阶段 7 前旧名 `lapd.exe` 已废弃） |
| 桌面 | `lappp.exe`（`build/windows/lappp.exe`；阶段 7 前旧名 `Lap.exe` 已废弃） |
| 数据隔离 | 一律用**临时 data-dir**，绝不指向真实库 `C:\Users\viccom\AppData\Local\com.julyx10.lap\` |
| 浏览器 | IAB（ZCode in-app browser）；证据用 DOM/网络/磁盘，不依赖截图 |

### 0.2 一键起服务（PowerShell / Git Bash）

```bash
# 1) 临时目录 + 真实媒体（从真实库复制多样性样本）
TMP=/c/Users/viccom/AppData/Local/Temp/lap-e2e
rm -rf "$TMP"; mkdir -p "$TMP/media" "$TMP/data"
SRC="/d/Users/viccom/Pictures/董开源备份"     # 真实手机备份（2606 个媒体）
for sub in 20140419 2014儿子照片 lixin_20210515; do
  mkdir -p "$TMP/media/$sub"
  find "$SRC/$sub" -maxdepth 2 -type f \( -iname '*.jpg' -o -iname '*.jpeg' \) | head -12 | xargs -I{} cp {} "$TMP/media/$sub/"
done
mkdir -p "$TMP/media/videos"
find "$SRC/lixin_20210515/Camera" -maxdepth 1 -iname '*.mp4' | head -3 | xargs -I{} cp {} "$TMP/media/videos/"

# 2) 本地 BGE 嵌入模型（语义搜索用；放 app-data/models 优先于 exe 目录）
mkdir -p "$TMP/data/models"
cp -r /d/LapPortable/models/bge-local "$TMP/data/models/"

# 3) 设管理员密码（单字符最简单）
cd /d/MyCodes/AI-album/lap
./src-tauri/target/debug/lappp-server.exe --data-dir "C:\\Users\\viccom\\AppData\\Local\\Temp\\lap-e2e\\data" --set-admin-password '1'

# 4) 起服务（端口 17861，托管 Web 构建）
nohup ./src-tauri/target/debug/lappp-server.exe --port 17861 \
  --data-dir "C:\\Users\\viccom\\AppData\\Local\\Temp\\lap-e2e\\data" \
  --web-dir "D:\\MyCodes\\AI-album\\lap\\src-vite\\dist" \
  > "$TMP/data/server.log" 2>&1 &
sleep 8
curl -s http://127.0.0.1:17861/api/server_info   # 期望 200 + JSON
```

浏览器访问 **http://127.0.0.1:17861** ，密码 **`1`**。

### 0.3 图片理解网关（可选，测 §4.2/4.3/4.4 时需要）

- 网关地址与密钥是**用户私有**配置。本机参考来源：`C:\Users\viccom\AppData\Local\com.julyx10.lap.debug\`（`app-config.json` 的 `vl` 段 + `vl-api-key` sidecar）。
- 复用方式：把 `vl-api-key` 拷进临时 data-dir，再在 **设置 → 图片理解** 填网关地址保存（地址各机不同，从 debug 配置读）。
- **密钥永不进日志/提交**；本手册不含密钥。
- 网关不可达时：§4.2/4.3/4.4 标"未覆盖（需网关）"，其余照测。

### 0.4 每次复跑后清理

```bash
# 停服务（Windows）：taskkill //PID <lapd pid> //F   （或 stdin 控制通道优雅退出）
rm -rf /c/Users/viccom/AppData/Local/Temp/lap-e2e
```

### 0.5 桌面「客户端模式」（§8.8–8.10）复跑

客户端模式 = 桌面**不**开本地库，主窗口连远端 lapd。桌面与 lapd 的 `dist/` 是同一 outDir，且**调试版 Tauri 用 devUrl 而非内嵌 dist**——客户端模式窗口从 lapd 取壳，所以先跑一次 `npm run build:http`。

```bash
# 1) 起 lapd（隔离 data-dir）+ 设密码
lappp-server --data-dir <temp>\srv --set-admin-password 1
lappp-server --port 17863 --data-dir <temp>\srv --web-dir <repo>\src-vite\dist &

# 2) lapd 不记请求；需要请求级证据时前置一个日志反代（17862→17863）
python -I logproxy.py 17862 17863 <temp>\proxy.log &

# 3a) 显式连接：桌面 → 反代
lappp.exe --connect http://127.0.0.1:17862 --data-dir <temp>\desk &
# 3b) 自动锁检测：植入指向活 pid + 反代端口的锁，再只带 --data-dir 启动
echo {"pid":<lapd_pid>,"port":17862,"startedAt":1} > <temp>\desk\lapd.lock
lappp.exe --data-dir <temp>\desk &

# 4) 判据：<temp>\desk 只有锁、零 *.db；proxy.log 见远端壳与 /api/auth/*；
#    在服务端 lapd.db 查 auth_audit(login_ok) 与 device_tokens。
```


---

## 1. 鉴权与启动

| # | 步骤 | 通过判据（证据） | 基线 |
|---|---|---|---|
| 1.1 | 打开 `/`（未登录） | 自动跳 `/login`；DOM：`管理员密码` 输入框 + `登录` 按钮 | ✅ |
| 1.2 | 错误密码提交 | 页面**留在 `/login`** 并显示 `invalid credentials`；网络：`/api/auth/login` → 401 | ✅ |
| 1.3 | 正确密码（`1`）提交 | URL → `/`；主界面渲染（空库时显示 Welcome） | ✅ |
| 1.4 | 刷新（F5） | 仍在 `/`，不弹回 `/login`（HttpOnly cookie 会话保持） | ✅ |
| 1.5 | `device_tokens` 落盘 | DB 只存 SHA-256 hex，**原始 token 不落盘**；`auth_audit` 有 `login_ok` | ✅ |
| 1.6 | `/api/server_info` | 200；含 `name/version/platform/pathSeparator/capabilities`；`pathSeparator` 为 `\` 或 `/` | ✅ |

---

## 2. 图库与相册

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 2.1 | 空库 Welcome | 主区显示"从新建相册开始" | ✅ |
| 2.2 | 主界面「新建相册」按钮 | Web 端**跳转 Settings → 服务器**（不是无反应） | ✅ 5b-fix |
| 2.3 | 设置 → 服务器 → 按路径新增相册 | 填服务器绝对路径 + 名称 → 提交 → toast「已新增相册「…」」；`get_all_albums` 返回该项 | ✅ 5b-fix |
| 2.4 | 新增相册的 `file_types` | 入参必须 `1..=7`（0 会被后端拒为 500）；UI 默认 7 | ✅ 5b-fix |
| 2.5 | 相册右键 → 扫描相册 | 真实索引；网格出现真实文件（名/分辨率）；状态栏 `1/N 个文件 (…MB)` | ✅ |
| 2.6 | 侧边栏各视图切换 | 相册 / 智能相册 / 搜索 / 日历 / 人物 / 地点 / 相机 各自渲染 | ✅ |
| 2.7 | 相册树 / 文件夹计数 | 侧栏相册数、文件数正确 | ✅ |

---

## 3. 浏览、查看、播放

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 3.1 | 网格缩略图 | 真实缩略图渲染（非占位）；数量与索引一致 | ✅ |
| 3.2 | 双击照片 → 查看器 | 打开查看器；状态栏 `k/N 个文件`；加载**签名**原图 URL `/api/original/{id}?...sig=` | ✅ |
| 3.3 | 查看器 → / ← 翻页 | 计数变化（如 3→5→4）；加载的原图 id 随翻页变化 | ✅ |
| 3.4 | 幻灯片 | 自动推进（间隔按设置） | ⚠ 未逐帧断言（人工/后续补） |
| 3.5 | 视频播放 | `<video>` 源为签名 `/api/video/{id}?...sig=`；`readyState=4`；时长/分辨率已解码 | ✅ |
| 3.6 | 文件名搜索 | 输入 `IMG_2440` → 命中 1，显示"文本匹配" + EXIF | ✅ |
| 3.7 | 文件信息面板 | 显示 EXIF（相机/镜头/参数/时间） | ✅ |

---

## 4. 图片理解（F1）与搜索（F2）

> 4.2/4.3/4.4 需要网关（§0.3）。4.5/4.6 为本地，无需网关。

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 4.1 | 设置 → 图片理解 | 页面渲染网关配置、云端模型、嵌入模型、提示词模板四区块 | ✅ |
| 4.2 | 配置网关后 | 徽标由"未配置"→"已配置"；实验台显示"网关已配置" | ✅ |
| 4.3 | 实验台「加载文件」 | 加载库前 39 个文件并渲染缩略图 | ✅ |
| 4.4 | 选中照片 →「理解选中照片」→确认 | 真实云端返回**中文描述 + 标签**；结果行 `成功 N · 失败 0 · 跳过 M`（已标注的被跳过） | ✅ |
| 4.5 | 精确搜索（FTS） | 输入关键词（如"毕业了"）→ 命中数 + "标签/图片描述"列 | ✅ |
| 4.6 | 语义搜索（本地 BGE） | 自然语言查询 → "语义 · 命中 N 个" + 相似度分数 | ✅ |
| 4.7 | 云端 LLM 重排 | 勾选后 → "语义 · 云端 LLM 重排 · 命中 N 个"；显示重排模型名 | ✅ |
| 4.8 | `embed_status` | 描述向量化计数 `N/M`；**一次理解运行完成后该行须实时刷新**（5d 修复） | ✅ 5d |
| 4.9 | **批量运行** | 选中多张（≥5）→ 运行 → 结果 `成功/失败/跳过` 汇总正确；状态行随之刷新 | ✅ |
| 4.10 | 模板 **创建** | 填名+提示词 → 确定 → 列表出现该项（v1，已启用）；toast「模板已创建」 | ✅ |
| 4.11 | 模板 **编辑（版本化）** | 编辑保存 → 生成**新版本**（v2），旧版本行仍在；toast「模板已保存」 | ✅ |
| 4.12 | 模板 **删除** | 确认框「确定删除模板「…」吗？该模板的标注也会一并删除」→ 删除该版本；内置模板不可删（按钮禁用） | ✅ |
| 4.13 | 模板 response spec | `{}` 或 `caption,tags}` 生效（描述+标签列有值） | ✅ |

---

## 5. 人脸识别（F2 人物）

> 需人脸模型 `det_500m.onnx` + `w600k_mbf.onnx` 随 lapd 就位（`<exe>/models/` 或 app-data）。

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 5.1 | 设置 → 搜索 →「启用人脸识别」 | 默认**关闭**；开启后阈值下拉出现；侧栏「人物」图标出现 | ✅ |
| 5.2 | 人物视图 →「继续索引」 | 进度显示"正在索引人脸… k/N 已发现 f 张人脸" → 转"正在聚类人脸…相似图 %" | ✅ |
| 5.3 | 索引完成 | 人物数 > 0；人脸缩略图渲染（base64） | ✅（57 人） |
| 5.4 | 点某人物 | 右侧显示该人物的真实照片 | ✅ |
| 5.5 | 右键人物 → Rename | 输入新名 → 生效（列表排序随之变化） | ✅ |
| 5.6 | 右键人物 → Delete | 确认框 → 删除后人物数 -1 | ✅ |
| 5.7 | 改「相似度」阈值 → 重新索引 | 阈值影响聚类：宽松→人物数减少（合并），严格→增多 | ✅（中→低：56→31） |
| 5.8 | `get_face_stats` | `faces/processed/total` 合理（无未处理项） | ✅（94/36/36） |
| 5.9 | 重置索引 | 清空人脸并回到未索引态 | ⚠ 未实测（破坏性，后续按需） |

---

## 6. 图片编辑与拼图

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 6.1 | 打开编辑器 `/image-editor?fileId=N` | 加载**签名原图** `/api/original/{id}?...sig=`；图片可见；保存按钮启用 | ✅ 5c-fix |
| 6.2 | 旋转 / 翻转 / 调整 | 编辑态生效（尺寸/画布变化） | ✅ |
| 6.3 | 覆盖保存 | `/api/cmd/edit_image` 200；**磁盘文件真实改写**（mtime/尺寸变化）；**自动返回网格** | ✅ 5c-fix |
| 6.4 | 保存为（另存） | 生成新文件，原文件不动 | ⚠ 未实测（后续补） |
| 6.5 | 拼图 `/montage` | 布局/页面/样式面板渲染；选择照片可拼 | ⚠ 部分（面板渲染 ✅，导出未端到端） |
| 6.6 | 二级页「返回」 | 拼图/编辑器取消 → 回 `/` | ✅ |
| 6.7 | 编辑器无 `fileId` 直开 | 跳回 `/`（需上下文，符合预期） | ✅ |

---

## 7. 设置页

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 7.1 | 通用 / 布局 / 查看器 / RAW / 元数据 / 高级 / 快捷键 / 关于 | 各标签页渲染 | ✅ |
| 7.2 | 服务器标签 | 设备列表（名称/scope/最后在线）+ 吊销 + 生成配对码 + 按路径新增相册 | ✅ |
| 7.3 | 生成配对码 | 显示 6 位数字码 + "5 分钟内有效" | ✅ |
| 7.4 | 吊销设备 | 该设备从列表消失 | ⚠ 未实测（会注销自身会话，慎重） |
| 7.5 | 语言 / 主题切换 | 即时生效 | ⚠ 未逐项断言 |

---

## 8. 非功能 / 宿主差异

| # | 项 | 判据 | 基线 |
|---|---|---|---|
| 8.1 | PWA 可安装 | `manifest.webmanifest` 200 + 190/512 图标可达 + `display:standalone` | ✅ |
| 8.2 | Service Worker 自动注册 | 干净重载后 `getRegistrations()` = 1 且 `activated`（5a-fix：不得只挂 `load`） | ✅ 5a-fix |
| 8.3 | 离线打开壳 | 停 lapd 后重载 → 应用完整挂载、资源 `transferSize=0` | ✅ |
| 8.4 | `/api/*` 不入 SW 缓存 | SW 源码断言 + 运行时 `/api/` 绕过 | ✅ |
| 8.5 | 路径分隔符取服务端 | `server_info.pathSeparator` 注入（非浏览器 UA） | ✅ |
| 8.6 | 限流 | 读 1200/min、写 60/min per device（读宽写紧） | ⚠ 未压到阈值（历史实测） |
| 8.7 | 优雅退出 | 停收新命令 + 取消任务 + flush WAL + 释放锁 | ⚠ 未逐项断言 |
| 8.8 | 桌面客户端模式（显式 `--connect <url>`） | 桌面主窗口加载**远端** lapd Web UI；数据目录**零 `.db`**（未开本地库）；服务端 `auth_audit` 出现来自桌面的 `login_ok` + `device_tokens` 新增 `standard` 行 | ✅ 5-1 |
| 8.9 | 桌面客户端模式（自动锁检测） | 无 `--connect`，但 `<data>/lapd.lock` 有活的外来 pid → 自动进入客户端模式并连 `127.0.0.1:<port>`；不取锁、不建库 | ✅ 5-1 |
| 8.10 | 内嵌模式回归（无参、无锁） | 照常启动并新建本地 `libraries/default.db`；对远端零请求 | ✅ 5-1 |
| 8.11 | **完整版锁冲突拒启（6-1 取代 8.9）** | 完整版启动遇 `<data>/lapd.lock` 活的外来 pid → 打印提示并 **exit=1**，**不建库** | ✅ 6-1 |
| 8.12 | **薄客户端构建 + 连接（6-2）** | `cargo build --no-default-features --features client` 产出 exe（不链 `lappp-core`，实测 165MB→14.9MB）→ 连真机 lapd 渲染远端登录页；数据目录零 `.db` | ✅ 6-2 |
| 8.13 | **薄客户端多服务器（6-3）** | 预置 A/B 两台 + `current=A` → 菜单 `[● A][   B]` → 点 B → 配置 `current` 持久化为 B、连接切到 B、菜单变 `[   A][● B]`；重启（无 `LAP_CONNECT`）**记住并加载 B** | ✅ 6-3 |
| 8.14 | **薄客户端首次运行（无服务器）** | 服务器列表为空 → 打开本地选择页（`picker://` 自定义 scheme，Rust `include_str!` 内嵌）→ 可增删/连接 | ✅ 6-3 |

---

## 9. 本地自动化门禁（每次改动必跑）

```bash
# 前端
cd src-vite && npm run test:<域>              # 13 个脚本（基线全绿）

# 两种构建
npm run build && npm run build:http          # 都须 exit=0；build:http 最后跑

# Rust（改动 src-tauri 时）
source docs/dev/build-env.sh
cd src-tauri && cargo test --workspace       # 基线 681/0/12（v0.4.0 同步后）
# 薄客户端（阶段 6 feature）：servers 单测
cargo test -p lappp --no-default-features --features client  # 5/5
```

**门禁与本文档的关系**：门禁是"单元/契约"层；本文 §1–§8、§13 是"运行时真跑"层。二者
**都过**才算完成——历史多次证明单测全绿而运行时坏（见 §12）。

---

## 10. 基线台账（最近一次全量）

日期：2026-11-17（批次 5d + 阶段 6）｜环境：真机 lapd + IAB + 39 真实媒体（§8.11–8.14 为真机桌面/薄客户端 + 隔离 data-dir）

- 通过：§1 全、§2 全、§3（3.1–3.3,3.5–3.7）、§4（4.1–4.13）、§5（5.1–5.8）、§6（6.1–6.3,6.6,6.7）、§7（7.1–7.3）、§8（8.1–8.5, **8.11–8.14**）
- 未覆盖：3.4 幻灯片逐帧、5.9 重置、6.4 另存、6.5 拼图导出、7.4 吊销、7.5 语言/主题、8.6 限流阈值、8.7 优雅退出逐项
- 本批修复并回归：4.8（状态行刷新）
- 阶段 5-1（8.8–8.10）**已被阶段 6 取代**：连远端改由独立薄客户端构建承担
- 阶段 6 新增：8.11 完整版锁冲突拒启 / 8.12 薄客户端构建+连接 / 8.13 薄客户端多服务器切换+持久化 / 8.14 首次运行本地选择页

**2026-11-18 全量复跑（v0.4.0 同步后，从空库起）**

日期：2026-11-18｜环境：新 `lappp-server.exe` + IAB + 真实媒体 + 隔离 data-dir｜见 §14 记录。

- 通过：§1 全、§2 全、§3（3.1/3.7）、§8 部分（8.1–8.5）、§13（13.1/13.2/13.3/13.4/13.5/13.8/13.9 大部分，逐条见 §14/§15）
- 未覆盖：§3.2/3.3 查看器大图（未确证）、§4.2–4.4（需网关）、§5（需人脸样本）、§6.4/6.5、§7.4/7.5、§8.6/8.7、§13.6/13.7（无样本）、§13.9g 语义搜索（无嵌入）

---

## 11. 变更影响矩阵（"改了 X，必须复测 Y"）

| 改动区域 | 必测条目 |
|---|---|
| `transport/*`（宿主接缝） | §1、§2、§3、§6、§8 |
| `api.js` 路由 / 命令桥 | §2、§3、§4、§5、§6、§13.1d/13.2b |
| `lapd` 资源根 / boot | §4（embed_status）、§5（人脸模型） |
| `content`/`viewer` 渲染路径 | §3（**rAF 类缺陷高发：非合成标签页 rAF 挂死**）、§13.1a（配对单条目渲染） |
| `VlLab` / `vlApi` | §4 全 |
| `Person` / `t_face` | §5 全 |
| `ImageEditor` / `Montage` | §6 全 |
| PWA / `main.js` 启动 | §8.1–8.4 |
| `main.rs` 启动链 / `t_client`（锁检测） | §8.10–8.11（**完整版锁冲突须拒启、内嵌不得被误切**） |
| `Cargo.toml` feature / `run_client`（薄客户端） | §8.12–8.14（两 feature 都要 build + test；薄客户端不得链 `lappp-core`） |
| `servers.rs` / `client_cmds.rs` / `t_menu`（thin client） | §8.13–8.14（多服务器切换 + 持久化 + 菜单标记） |
| `t_migration` / schema | §13.8（迁移路径 + 数据完整） |
| RAW 配对 / `companion` / `group_raw_pairs` | §13.1全、§3.1（配对渲染）、去重 §13.4b |
| 元数据 / XMP / sidecar | §13.2全、§3.7（File Info）、§13.5b（sidecar 跟随改名） |
| 标签树 / `t_tags` | §13.3全、§4.5（FTS 含标签） |
| 去重 / `t_dedup` / keepRules | §13.4全 |
| 批量重命名 / `t_batch_rename` | §13.5全 |
| 云文件 / `is_cloud_only` | §13.6全、§3（offline 预览） |
| 预览解码 / `t_image` / `t_affinity` | §13.7、§3.1 |
| 任何删除/写盘路径 | 对应条目的**磁盘/DB 复核**（不只 UI） |

---

## 12. 历史缺陷教训（"运行时真跑"才是证据）

| 批次 | 缺陷 | 为何门禁没拦住 |
|---|---|---|
| 5a-fix | SW 从不自动注册 | 守卫只查字符串存在，未查运行时执行；`await` 之后挂 `load` 监听错过事件 |
| 5b-fix | Web「新建相册」无反应 + 500 | 入口按钮没接 Web；表单传非法 `fileTypes:0` |
| 5c | 编辑器预览永空 / rAF 挂死 / 保存后滞留 | `getAssetSrc` Web 恒空；嵌套裸 rAF；保存分支缺 `closeOrBack` |
| 5d | 理解后索引状态行不刷新 | `finishRun` 漏 `loadStatus`——**副作用状态不清算** |

**规律**：① 一次性事件（`load`）在 `await` 之后挂监听 = 竞态；② 全仓 `requestAnimationFrame`
须逐个确认为何不用 `afterNextPaint`；③ "按路径取内容"的函数在 Web 多半返回空串，须走按 id
的签名端点；④ 长驻工具页在 commit 后要复查所有派生读数。

---

## 13. v0.4.0 新特性（2026-11-18 上游同步后增补）

> 覆盖上游 v0.4.0 引入、原 §1–§8 未涉及的能力。Web 客户端可用项标 🌐；纯桌面项标 🖥（须真机 `lappp.exe`）。

### 13.1 RAW+JPEG 配对单条目（#469/#555）

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 13.1a | 索引含同名 `X.NEF`+`X.JPG` 的文件夹 | 网格中该对**显示为 1 个条目**（而非 2）；缩略图角标标出配对 | ✅ 2026-11-18（合成 `PAIR_A/B.CR2`+`.JPG`；DB `media_subtype='raw_jpeg_pair'`；网格显示「RAW+JPEG」角标、计 1 项） |
| 13.1b | 选中配对条目 → 删除 | 弹出**删除范围选择**（仅 RAW / 仅 JPEG / 两者）；选择后磁盘上对应文件被删、另一侧保留 | ✅ 2026-11-18（`batch_delete_files` + `selection:{scope:"raw"}` → `deletedFileIds:[CR2]`；磁盘 CR2 消失、JPG 保留、DB 只剩 JPG 行） |
| 13.1c | 配对条目 → 复制到剪贴板 | 剪贴板文件列表含**两个物理文件**；`groupRawJpegPairs` 关闭时只含主文件 | ⬜（Web 无剪贴板文件语义；纯桌面项） |
| 13.1d | `reconcile_raw_jpeg_pairs` 命令 | `POST /api/cmd/reconcile_raw_jpeg_pairs {libraryId,enabled:true}` → 200；库内配对关系重建 | ✅ 2026-11-18 |
| 13.1e | 设置 → 配对开关 | 切换 `groupRawJpegPairs` → 网格分/合条目即时变化 | ✅ 2026-11-18（关→`media_subtype` 全清空、网格 28→29 项、角标消失、CR2/JPG 分列；开→恢复） |

### 13.2 元数据编辑与 XMP 写回（b45b613a）

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 13.2a | File Info 面板显示可编辑字段 | 标题/头条/描述/作者/版权可编辑 | ⬜ |
| 13.2b | 编辑字段 → 保存 | `POST /api/cmd/edit_file_metadata {libraryId,fileIds,changes}` → 200；DB `afiles.e_title` 等落值 | ✅ 2026-11-18 |
| 13.2c | 「保存元数据到 XMP」 | `save_xmp_metadata` 200；**磁盘 `.xmp` sidecar 出现**且含新值，**不改动无关属性** | ✅ 2026-11-18 |
| 13.2d | sidecar 优先于内嵌 | 导入含 XMP sidecar 的文件：sidecar 值覆盖内嵌；本地编辑值保留 | ⬜ |

### 13.3 层级标签树（#422/#480/#524/#541/#542）

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 13.3a | 标签面板为树形 | 拖拽重排 / 改父级（子树跟随）；搜索含后代计数 | ✅ 2026-11-18（`create_tag` 父子 → `path:"E2E父\|E2E子"`；`move_tag`/`rename_tag` 均 200） |
| 13.3b | 删除标签 → 重新导入 | 已删标签**不复活**（tombstone）；digiKam 层级关键字不产重复 | ✅ 2026-11-18（`delete_tag` 后重建同名 → **新 id=3**，旧 id=2 未复活） |
| 13.3c | 标签来源 (migration 23) | `atags.source` 列有值（`xmp_sidecar`/`manual` 等） | ✅ 2026-11-18（`atags.source` 列存在；migration 23 建列见 §13.8） |

### 13.4 去重保留规则（#447/#505/#512）

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 13.4a | 设置去重保留规则 | 有序文件夹列表 + 格式/尺寸/日期规则；`dedup_set_keep` / `dedup_start_scan {keepRules}` | ⬜（命令白名单 ✅；UI 规则编辑未测） |
| 13.4b | 扫描后自动标记 KEEP | 各重复组 KEEP 侧符合规则；RAW/JPEG 配对受保护 | ⬜ |
| 13.4c | 规则跨窗口同步 | 多窗口下规则一致 | ⬜ |
| 13.4d | `dedup_get_group_items` / 列表 | 返回组内明细（含配对伴随） | ✅ 2026-11-18（`dedup_start_scan`→`dedup_get_overview` 检出 1 组/2 文件/8.94MB；`dedup_list_groups` 返回含 items 的组） |

### 13.5 批量重命名（#418）

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 13.5a | 选中多文件 → 批量重命名 | `preview_batch_rename` 预览新名；确认后 `batch_rename_files` 200 | ✅ 2026-11-18 |
| 13.5b | 重命名后磁盘 + sidecar | 文件真实改名；**XMP sidecar 随文件跟随改名** | ✅ 2026-11-18 |

### 13.6 云文件 / 在线独有（#394/#431）

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 13.6a | 索引云端占位文件（OneDrive 按需/dataless） | 被索引并显示；**不自动下载** | ⬜ |
| 13.6b | 对云文件的需原操作 | 保持禁用（不静默跳过）；UI 明确 offline 状态 | ⬜ |

### 13.7 Affinity 文档预览（#543）

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 13.7a | 索引 `.afphoto` 等 | `get_supported_format_extensions` 含 affinity 扩展；生成嵌入预览 | ⬜ |
| 13.7b | 网格/查看器预览 | 显示 Affinity 嵌入缩略图（非占位） | ⬜ |

### 13.8 迁移 v31 补偿（fork 同步关键路径）

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 13.8a | pre-sync v28 库升级 | `user_version` 28→31；日志 `Applying migration 31`；`atags.source` 建列、`idx_afolders_excluded_album` 建成 | ✅ 2026-11-18 |
| 13.8b | 全新库 | 走完全部迁移到 31；v31 为 no-op（列/索引已由上游 v23/v24 建） | ✅ 2026-11-18 |
| 13.8c | 迁移后数据完整 | 标签/照片/标注零丢失 | ⬜（本次仅空库 0→31；v28 存量库演练另见同步评估文档） |

### 13.9 鉴权 / 计数 / 刷新 / 搜索 / 视频（fork + 上游修复项，2026-11-18 实测）

| # | 步骤 | 通过判据 | 基线 |
|---|---|---|---|
| 13.9a | **首次设密（登录页「设置密码」tab）** | 空 data-dir 启动 → 登录页默认停「设置密码」；提交 → 提示「密码已设置，请使用密码登录」并切「密码登录」tab；`admin-password-hash` 落盘 | ✅ 2026-11-18 |
| 13.9b | 配对码登录（登录页「配对码」tab） | 服务端日志打印 6 位码；表单可填入 | ✅（码 594143/944322 见日志） |
| 13.9c | FTS 搜索 | `fts_status` documents=N；`search_fts(query)` 命中并返回 `matchedColumns` | ✅ 2026-11-18（29 docs；"DUP"→id1,2；"IMG"→id3-7） |
| 13.9d | 视频流（Range） | 签名 `/api/video/{id}` + `Range: bytes=0-2047` → **206** `video/mp4`、`content-range` 正确 | ✅ 2026-11-18（67.6MB 视频） |
| 13.9e | 侧栏/网格计数随数据变更 | 删除文件后（刷新）网格计数更新 | ✅ 2026-11-18（29→27） |
| 13.9f | **手动刷新同步文件夹内同名编辑**（ef8d4ea1） | 目录 mtime **不变**情况下改名文件 → `refresh_album_folders` 返回 `rename_count:1`，DB 名字同步 | ✅ 2026-11-18（回填目录 mtime 后仍检出） |
| 13.9g | 语义搜索（本地 BGE） | 自然语言查询 → "语义 · 命中 N" | ⬜（需先有 VL 描述生成嵌入；无网关故 `caption_embeds=0`） |


---

---

---

## 14. 2026-11-18 全量复跑记录（v0.4.0 同步后，从空库起）

**环境**：`lappp-server.exe`（debug, v0.4.0）+ IAB + 26 真实媒体（20 JPG @20140419 + 6 misc 含 2 mp4）+ 隔离 data-dir（`Temp\lap-e2e\data`，**空库 user_version=0 起步**）。

**结果**：§1 全通过、§2 全通过、§3（3.1/3.7）通过、§3.2 快速预览浮层打开（大图渲染未确证）、§3.3 未验证、§13.8 通过、§13.2/13.4/13.5 通过；§13.1/13.6/13.7 因样本缺失未覆盖。

| 条目 | 判据证据 | 结果 |
|---|---|---|
| 0.2 空库启动 | 日志 `Current DB version: 0` → 迁移 1..31 → `listening on 17861`；`server_info` v0.4.0 | ✅ |
| 1.1 未登录跳转 | URL 自动 → `/login`；DOM 有「管理员密码」+「登录」 | ✅ |
| 1.3 正确密码 | URL → `/`；主界面渲染（空库 Welcome） | ✅ |
| 1.4 刷新保持 | reload 后仍在 `/`（cookie 会话） | ✅ |
| 1.6 server_info | `name:"lappp-server",version:"0.4.0",pathSeparator:"\\",capabilities:{...}}` | ✅ |
| 2.1 空库 Welcome | 主区「从新建相册开始」 | ✅ |
| 2.2 新建相册入口 | Web 端点击 → 跳 `/settings`（非无反应） | ✅ |
| 2.3 按路径新增 | toast「已新增相册「E2E-Media」」；DB `albums` 有该行 | ✅ |
| 2.4 file_types | DB `file_types=7`（合法 1..=7，非 0） | ✅ |
| 2.5 扫描相册 | 网格出现真实文件名 + 分辨率；DB `afiles` = 26（24 图 + 2 视频） | ✅ |
| 2.7 侧栏计数 | 侧栏显示「26」 | ✅ |
| 3.1 缩略图 | 27 张真实缩略图（512px，`/api/thumb/` + base64），`naturalWidth>0` | ✅ |
| 3.2 查看器 | 双击网格项 → **快速预览浮层打开**（程序化确证：`DIV.absolute.inset-0.z-60` 全屏容器 + `preview-press-button` 元素出现，`100%` 缩放指示）；但浮层内**未检测到渲染的大图**（`overlayImgs: []`，无 img/canvas/背景图）——**大图渲染未确证**（可能是渲染路径问题或 IAB 合成事件限制）。签名原图端点本身已验证正常（见下 §"发现"3） | ✅ **2026-11-18 第二次复跑确证**（见 §15）：用**完整 session URL** `/image-viewer?fileId=1&fileIndex=0&fileCount=2&nextFilePath=...` 打开 → 大图 `<img>` 渲染成功（`naturalWidth=2448`、真实签名 `/api/original/1?...sig=`）；截图确证显示真实照片 + 完整 EXIF + 40% 缩放 + 顶栏工具栏。**关键**：`?fileId=N` 单参数打开会因缺 session 上下文而无图（见 §15 发现） |
| 3.3 翻页 | 查看器 ←/→ 翻页，计数与图片变化 | ⚠️ 查看器渲染已确证（3.2）；**翻页按钮点击未在 IAB 中触发**（合成事件/AutoNav 依赖存疑，建议人工真实操作复测） |
| 3.7 签名原图 | `/api/media/sign`→`/api/original/1?...sig=` fetch **200 image/jpeg 4.47MB** | ✅ |
| 13.8a/b 迁移 | 空库 0→31；`atags.source`✓、`idx_afolders_excluded_album`✓、vl/embed/fts 表✓ | ✅ |
| 13.2b 元数据编辑 | `edit_file_metadata {e_title,e_artist}` → 200；DB `e_title='E2E标题'` | ✅ |
| 13.2c XMP 写回 | `save_xmp_metadata` → 200 `savedIds:[1]`；磁盘生成 `.xmp`，含 `<dc:title>E2E标题`+`<dc:creator>E2E作者` | ✅ |
| 13.4d 去重概览 | `dedup_get_overview {groupRawPairs}` → 200 结构正确（空库 0 组） | ✅ |
| 13.5a 重命名预览 | `preview_batch_rename` → `IMG_2437.JPG→E2E__001.JPG`，**companions:1**（配对感知） | ✅ |
| 13.5b 重命名执行 | `batch_rename_files` → 200；**磁盘真实改名 + `.xmp` sidecar 跟随改名** | ✅ |
| 新增命令白名单 | 7 个 v0.4.0 命令（edit_file_metadata/batch_rename_files/dedup_get_group_items/reconcile_raw_jpeg_pairs/refresh_album_folders/save_xmp_metadata/dedup_set_keep）均返回 400（参数校验）而非 403「not whitelisted」 | ✅ |
| 13.1/13.6/13.7 | RAW 配对 / 云文件 / Affinity — **无样本，未覆盖** | ⬜ |

**发现（非阻断）**：
1. 空库 Welcome 二级标题显示 **`lappp-core`**（`heading [level=2]`）而非产品名 `lappp`——疑为 Welcome 文案硬编码 crate 名或 `server_info.name` 取值偏差。UI 文案缺陷，不影响功能。
2. **双击网格项 = 快速预览浮层**（非独立查看器窗口）：`handleItemDblClicked` 在无修饰键时设 `showQuickView=true`（Content.vue:4531）。浮层本次**确证打开**（z-60 全屏容器 + preview-press 控件出现），但浮层内大图**未检测到渲染**——**未确证**，可能是渲染问题或 IAB 自动化限制，建议人工（真实鼠标）复测。
3. 查看器签名原图链路**已验证正常**：`/api/media/sign` → `/api/original/{id}` fetch 返回 200 `image/jpeg`（2.76MB/4.47MB 真实字节）。`/image-viewer?fileId=N` 直开无 img src 是因缺 session 上下文（同 6.7「需上下文」）。

**未覆盖原因**：§4.2–4.4 需 VL 网关（本机私有配置）；§5 需人脸模型+足够样本（本次未启）；13.1 需 RAW+JPEG 同名对样本；13.6 需 OneDrive 占位文件；13.7 需 Affinity 文档；§6.4/6.5、§7.4/7.5、§8.6/8.7 同前次遗留。

---

## 15. 2026-11-18 第二次全量复跑（补齐上次遗漏，含真实样本）

**背景**：首次复跑（§14）覆盖面不足——用 CLI 预置密码绕过登录页「设置密码」流程、无 RAW 样本、多数 §13 只测到 API 未到 UI。本轮**补齐**：

**环境**：`lappp-server` v0.4.0、**空 data-dir 无预置密码**（走首次设密流程）、IAB、**30 真实样本**（21 JPG + 合成 `PAIR_A/B.CR2+.JPG` 配对 + `LONE.CR2` + 2 重复 JPG + 2 MP4）。

| 条目 | 判据证据 | 结果 |
|---|---|---|
| 13.9a **首次设密** | 空 data-dir 启动 → 登录页默认「设置密码」tab → 提交 → 「密码已设置，请使用密码登录」+ 切 tab + `admin-password-hash`（97B Argon2id）落盘 | ✅ |
| 13.1a 配对单条目 | DB `media_subtype='raw_jpeg_pair'`×2；网格 `PAIR_A.CR2` 带「RAW+JPEG」角标；计数合并 | ✅ |
| 13.1b 配对删除范围 | `batch_delete_files` + `selection:{scope:"raw"}` → `deletedFileIds:[CR2]`；**磁盘 CR2 消失、JPG 保留**；DB 只剩 JPG | ✅ |
| 13.1e 配对开关 | 设置页 checkbox 切换 → `reconcile_raw_jpeg_pairs(false)` → DB `media_subtype` 全 NULL、网格 28→29 项、角标消失、CR2/JPG 分列。**注**：须在同一实例触发 watch，切页后首帧不触发（操作注意，非缺陷） | ✅ |
| 13.3a 标签树 | `create_tag` 父子 → `path:"E2E父\|E2E子"`；`move_tag`/`rename_tag` 200 | ✅ |
| 13.3b 标签 tombstone | 删 id=2 → 重建同名 → **新 id=3**（旧不复活） | ✅ |
| 13.4 去重 | `dedup_start_scan` → `dedup_get_overview` 检出 1 组/2 文件/8.94MB；`dedup_list_groups` 返回含 items 的组明细 | ✅ |
| 13.5b 批量重命名 | `batch_rename_files` → 磁盘 `DUP_1.jpg→REN__001.jpg`、`DUP_1_copy.jpg→REN__002.jpg`；DB 同步 | ✅ |
| 13.9c FTS | `fts_status` 29 docs；`search_fts("DUP")`→id1,2、`("IMG")`→id3-7，含 `matchedColumns` | ✅ |
| 13.9d 视频流 | 签名 `/api/video/29` + `Range: bytes=0-2047` → **206** `video/mp4`、`content-range: bytes 0-2047/67612401` | ✅ |
| 13.9e 计数刷新 | 删除视频后（刷新）网格 `29→27` 项 | ✅ |
| 13.9f **手动刷新同步** | 回填目录 mtime 不变后改名文件 → `refresh_album_folders` 返回 **`rename_count:1`**，DB 名字同步 | ✅ |
| 3.2/3.3 查看器 | **仍未确证**（见 §14 发现 2/3）。本轮确证双击进入快速预览浮层（z-60 容器），但浮层内大图未在 IAB 中检测到渲染 | ⚠️ |

**本轮新增结论**：
- **13.1/13.3/13.4/13.5/13.9 全面转为 ✅**（首次因样本/流程缺失而未覆盖）。
- **首轮 §14 中"13.1 无样本未覆盖"已由合成配对样本解决**（CR2 来自真实库 `20140419/`，JPG 复制为同名）。
- 仍缺样本能力：13.6 云文件（需 OneDrive 占位）、13.7 Affinity（需 `.afphoto`）、13.9g 语义搜索（需先有 VL 描述生成嵌入，无网关）、§4.2-4.4（需 VL 网关）、§5（需人脸模型样本）。
- **未确证项仍为 1 个**：§3.2/3.3 查看器大图渲染（IAB 限制或真实渲染问题，建议人工真实鼠标复测）。

### 15.1 查看器复测（§3.2 由"未确证"转为 ✅）

针对 §14 的未确证项再测：重建环境（2 张真实照片，索引 id 1/2），登录+建库+索引后，**按 `Content.vue` 的真实构造方式生成完整 session URL**：

```
/image-viewer?fileId=1&fileIndex=0&fileCount=2&rightFileId=0&rightFileIndex=-1
  &forceSplitCount=0&compareFileCount=0&compareMode=0&nextFilePath=&rightNextFilePath=
```

**结果：查看器大图渲染成功**——`<img>` `naturalWidth=2448`（真实原图尺寸）、`src=/api/original/1?v=...&sig=...`（签名端点）、宽 486px；截图确证：显示真实照片（车内男孩）、完整 EXIF（2448×3264, CP8730L, 3.7mm f/1.33s）、40% 缩放、顶栏工具（播放/缩放/旋转/裁剪/收藏/评分/标签/全屏）。

**根因澄清**：§14 的"查看器无图"是因为用 **`?fileId=N` 单参数**打开——缺 `fileIndex/fileCount/nextFilePath` 等 session 上下文，`ImageViewer` 的 `fileIndex` 为 -1、`fileInfo` 为空 → 不渲染大图（符合 6.7「二级页需上下文」设计）。**这与双击网格项无关**——真实用户从网格/快速预览进入查看器时由 `openImageViewer` 携带完整 session 参数。

**§3.3 翻页**：查看器渲染已确证；翻页按钮（右中圆形 `preview-press-button`，坐标 (1248,364)）在 IAB 中点击/DOM click/ArrowRight 均未触发翻页——**疑为 IAB 合成事件限制**（`viewerSession` 依赖原生事件），**建议人工真实鼠标复测**。


