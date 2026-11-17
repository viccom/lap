# 端到端（E2E）测试清单与复现手册

> 目的：**浏览器版（Web 客户端 / lapd）为主线**的可测功能全清单。日后任何代码改动后，
> 按本清单逐条复跑，即可确认"可测功能都已覆盖并验证通过"，并把失败集合与本文档登记
> 的基线对照（不得新增失败）。
>
> 本文件不是"设计文档"，是**验收脚本 + 结果台账**。每条的"通过判据"必须给出可观察证据
> （页面文本 / 网络请求 / 磁盘字节 / DB 行），不接受"看着对"。
>
> 建立于 2026-11-17（阶段 4 批次 5d）。锚点随代码漂移，以实际代码为准。

---

## 0. 测试环境搭建（每次复跑前）

### 0.1 前置

| 项 | 要求 |
|---|---|
| 构建 | `cd src-vite && npm run build:http`（`VITE_LAP_TRANSPORT=http`；**必须最后跑，dist 是共享 outDir**） |
| 服务端 | `src-tauri/target/debug/lappp-server.exe`（debug 构建；`cargo build -p lappp-server`），或用 `scripts/build-windows.ps1` 产出的 `lappp-server.exe` |
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
./src-tauri/target/debug/lapd.exe --data-dir "C:\\Users\\viccom\\AppData\\Local\\Temp\\lap-e2e\\data" --set-admin-password '1'

# 4) 起服务（端口 17861，托管 Web 构建）
nohup ./src-tauri/target/debug/lapd.exe --port 17861 \
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
lapd --data-dir <temp>\srv --set-admin-password 1
lapd --port 17863 --data-dir <temp>\srv --web-dir <repo>\src-vite\dist &

# 2) lapd 不记请求；需要请求级证据时前置一个日志反代（17862→17863）
python -I logproxy.py 17862 17863 <temp>\proxy.log &

# 3a) 显式连接：桌面 → 反代
Lap.exe --connect http://127.0.0.1:17862 --data-dir <temp>\desk &
# 3b) 自动锁检测：植入指向活 pid + 反代端口的锁，再只带 --data-dir 启动
echo {"pid":<lapd_pid>,"port":17862,"startedAt":1} > <temp>\desk\lapd.lock
Lap.exe --data-dir <temp>\desk &

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
cd src-vite && node --experimental-vm-modules --test tests/*.test.mjs   # 基线 475/0
node --experimental-vm-modules --test tests/                            # 全目录 501/0（含 2 个 .ts）

# 两种构建
npm run build && npm run build:http          # 都须 exit=0；build:http 最后跑

# Rust（改动 src-tauri 时）
source docs/dev/build-env.sh
cd src-tauri && CARGO_HTTP_MULTIPLEXING=false CARGO_HTTP_VERSION=HTTP/1.1 cargo test --workspace  # 基线 654/0/10
# 薄客户端（阶段 6 feature）：servers 单测
cargo test -p lappp --no-default-features --features client  # 5/5
```

**门禁与本文档的关系**：门禁是"单元/契约"层；本文 §1–§8 是"运行时真跑"层。二者
**都过**才算完成——历史多次证明单测全绿而运行时坏（见 §11）。

---

## 10. 基线台账（最近一次全量）

日期：2026-11-17（批次 5d + 阶段 6）｜环境：真机 lapd + IAB + 39 真实媒体（§8.11–8.14 为真机桌面/薄客户端 + 隔离 data-dir）

- 通过：§1 全、§2 全、§3（3.1–3.3,3.5–3.7）、§4（4.1–4.13）、§5（5.1–5.8）、§6（6.1–6.3,6.6,6.7）、§7（7.1–7.3）、§8（8.1–8.5, **8.11–8.14**）
- 未覆盖：3.4 幻灯片逐帧、5.9 重置、6.4 另存、6.5 拼图导出、7.4 吊销、7.5 语言/主题、8.6 限流阈值、8.7 优雅退出逐项
- 本批修复并回归：4.8（状态行刷新）
- 阶段 5-1（8.8–8.10）**已被阶段 6 取代**：连远端改由独立薄客户端构建承担
- 阶段 6 新增：8.11 完整版锁冲突拒启 / 8.12 薄客户端构建+连接 / 8.13 薄客户端多服务器切换+持久化 / 8.14 首次运行本地选择页

---

## 11. 变更影响矩阵（"改了 X，必须复测 Y"）

| 改动区域 | 必测条目 |
|---|---|
| `transport/*`（宿主接缝） | §1、§2、§3、§6、§8 |
| `api.js` 路由 / 命令桥 | §2、§3、§4、§5、§6 |
| `lapd` 资源根 / boot | §4（embed_status）、§5（人脸模型） |
| `content`/`viewer` 渲染路径 | §3（**rAF 类缺陷高发：非合成标签页 rAF 挂死**） |
| `VlLab` / `vlApi` | §4 全 |
| `Person` / `t_face` | §5 全 |
| `ImageEditor` / `Montage` | §6 全 |
| PWA / `main.js` 启动 | §8.1–8.4 |
| `main.rs` 启动链 / `t_client`（锁检测） | §8.10–8.11（**完整版锁冲突须拒启、内嵌不得被误切**） |
| `Cargo.toml` feature / `run_client`（薄客户端） | §8.12–8.14（两 feature 都要 build + test；薄客户端不得链 `lappp-core`） |
| `servers.rs` / `client_cmds.rs` / `t_menu`（thin client） | §8.13–8.14（多服务器切换 + 持久化 + 菜单标记） |
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
