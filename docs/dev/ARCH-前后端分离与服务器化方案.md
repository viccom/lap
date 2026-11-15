# ARCH · lap 前后端分离与服务化方案

日期：2026-11-15
状态：**方案定稿，尚未实施**（方向已拍板；阶段 0 的清单已产出，基线测试未固化）
分支：`develop`
配套文档：[ARCH-命令清单与拆分契约.md](./ARCH-命令清单与拆分契约.md)（机器生成，253 条命令逐条分类）

> 决策记录（2026-11-15，用户拍板）：
> 1. 部署形态 = **独立 `lapd` 服务 + 桌面可选客户端**（桌面保留内嵌模式为默认）
> 2. 暴露范围 = **只局域网 + 令牌**（默认 `127.0.0.1`，显式 `--host 0.0.0.0` 才对外）
> 3. 远程/移动客户端 = **PWA 优先**

---

## 0. 结论摘要

1. **可行，且成本比看起来低**：253 条命令中 **192 条是纯函数**（无任何 Tauri 注入参数），只有 **45 条需适配**、**16 条原生专属**；前端 228 个 `invoke` 调用点只来自 **15 个文件**的同一句 import（209 个集中在 `common/api.js`）；图片/视频 URL 只在 `common/utils.ts` 的 **3 个函数**里构造。模块级核验（`rg 'tauri::'` 全量扫描，见 §3.1）更乐观：**19 个模块零 Tauri 引用**、**4 个模块只用 `tauri::async_runtime`**（换成 tokio 即可），真正要动手术的只有 10 个模块（含 `t_cmds` 巨石）。
2. **最低代价的路线不是"先拆 crate"**，而是"先在原地把命令体改成无 Tauri 依赖，再打包成 crate"。这样路径改写、workspace/锁文件/CI 变更都推迟到代码已经 Tauri-free 之后，由编译器与既有测试兜底。
3. **真正的风险不在传输层**：个人照片库联网后的鉴权与命令面、SQLite 单写者所有权、桌面专属 UX、移动端布局、跨平台打包，才是决定成败的地方。
4. **MVP（局域网 Web 浏览/搜索/查看 + 鉴权）约 2–3 周专注工作**；含手机 PWA 再加约 2 周。
5. **桌面默认行为不变**：服务模式 opt-in，阶段 1 的改动可逆，C 类命令永不进入 HTTP 白名单。

---

## 1. 目标与非目标

**目标**

- 后端可作为无 GUI 的常驻服务运行于 Linux / macOS / Windows（`lapd`）。
- 用户在桌面程序、浏览器、手机（PWA）上，经网络完成浏览、检索、查看原图/视频、基础元数据编辑。
- 桌面单机体验不回退：内嵌模式仍是默认，不强制联网。
- 一套前端代码、两种传输；一套业务内核、两个外壳。

**非目标（本期明确不做）**

- 多用户账号体系与细粒度权限（只做「单用户 + 多设备令牌」）。
- 公网直连与内置证书签发（ACME）；远程访问走 VPN 或反代 TLS。
- 原生移动 App（先 PWA；商店分发后续再议）。
- 多图库并发服务（沿用单 `current_library_id` 模型）。
- 远程数据库、微服务化、换后端语言重写。
- 桌面的文件系统类操作（选择目录、在资源管理器中显示、拖拽出窗、壁纸、更新器）在 Web 端提供替代实现——这些直接隐藏。

---

## 2. 现状事实（均已核实）

| 维度 | 事实 | 证据 |
|---|---|---|
| 命令面 | 253 条 / 9 个模块；`t_cmds.rs` 单文件 4644 行、占 216 条 | `src-tauri/src/main.rs:271-549` |
| 拆分难度 | A 纯内核 192 · B 需适配 45 · C 原生专属 16 | 配套清单文档；脚本 `docs/dev/gen-command-inventory.py` |
| 业务分层 | 30+ 个非命令模块（`t_sqlite` 13.7k 行、`t_utils` 5.2k、`t_image` 2.4k…）本身与 Tauri 无关 | 各 `t_*.rs` |
| 前端耦合 | 228 个 `invoke`，209 个在 `common/api.js`；仅 15 个文件 import `@tauri-apps/api/core`；29 个文件用 event | `src-vite/src` |
| 事件 | Rust 侧 23 个事件名 / 46 个 emit 点；前端 303 emit、110 listen，多数是**多窗口 JS↔JS 总线** | 同上 |
| 媒体通路 | `thumb://` / `preview://` 自定义协议（`t_protocol.rs:129-309`）、`asset://`（`utils.ts:771`）、Windows data-URL 缩略图缓存；Linux 另有 `127.0.0.1:0` 视频 206 服务 | `t_http.rs:23-313` |
| 网络栈 | **没有任何 HTTP server crate**；`hyper` 只是 `reqwest` 的传递依赖；唯一监听器是 Linux 视频服务 | `src-tauri/Cargo.toml`、`Cargo.lock` |
| 安全 | **无鉴权、无 CSP**；asset scope 覆盖 `$HOME/**` 等；命令面含任意 exe 启动与 SSRF | `tauri.conf.json:32-47`、`t_cmds.rs:1573/2562` |
| 数据所有权 | 配置/连接池/迁移守卫全是**进程内全局**；无跨进程锁、无 single-instance 插件 | `t_config.rs:20-22`、`t_sqlite.rs:11179` |
| 跨平台 | CI 已构建 macOS(aarch64/x64)、Linux(x64/aarch64)、Windows；但 `resources/ffmpeg/` 只有 Windows x64 两个二进制 | `.github/workflows/release.yml`、`src-tauri/resources/ffmpeg/` |
| 原生构建 | 4 个 C 子模块（LibRaw / libjpeg-turbo / libheif / libde265）由 `src-tauri/build.rs` 编译，源码在 `src-tauri/third_party/` | `build.rs:22/234/407/474` |
| 构建环境 | `PCRE2_SYS_STATIC=1` 在**仓库根** `.cargo/config.toml`（不在 src-tauri 内），嵌套 workspace 不受影响 | `lap/.cargo/config.toml` |
| 测试 | CI 不跑 `cargo test` 与前端测试；前端有 8 个已知失败测试文件 | `pr-build.yml`、仓库 `AGENTS.md` |

**两条推论**

- 业务内核已经事实上"服务化"了：它只依赖参数、进程内 DB 与文件系统；Tauri 只在最外层（命令签名、事件、路径解析、窗口）。
- 前端已经是"薄客户端"：UI 与后端之间只有一个 `invoke(name, args)` 协议。缺的只是第二套传输，而不是重写业务。

---

## 3. 目标架构

### 3.1 进程与仓库布局

```
src-tauri/                     workspace 根（保留 Cargo.lock / target / CI 路径）
  Cargo.toml                   [package Lap] + [workspace] members = ["crates/lap-core", "crates/lap-server"]
  src/                         桌面壳：main.rs、t_cmds（薄包装）、t_drag、t_menu、t_protocol、t_startup
  crates/lap-core/             业务内核（无 tauri 依赖）
  crates/lap-server/           bin `lapd`：axum、鉴权、媒体、SSE、服务安装
src-vite/                      一套前端，两种传输
```

- 复用 `src-tauri` 作为 workspace 根，**不把 workspace 提到仓库根**：避免 `Cargo.lock`、`target/`、CI 缓存与 `working-directory` 的连锁改动。
- 桌面壳**不需要 `lib.rs`**：它是 bin crate，依赖 `lap-core` 即可；需要共享的是内核，不是壳。
- 只有当模块已经 Tauri-free 后才移入 `lap-core`（见 §4 阶段 2→3 的顺序）。

**模块归属边界**（2026-11-15 以 `rg 'tauri::' src/*.rs` 全量核验，这是迁移的准入判据）：

| 归属 | 模块 | 说明 |
|---|---|---|
| 零 Tauri 引用，**直接进 lap-core**（19 个 + metadata） | `t_config` `t_storage` `t_cache_storage` `t_migration` `t_common` `t_cluster` `t_tags` `t_vl_tags` `t_vl_protocol` `t_batch_rename` `t_motion_photo` `t_raw_display` `t_libraw` `t_jpeg` `t_jxl` `t_heif` `t_embedded_jpeg` `t_lens` `t_ai_png`，及 `metadata/*` | 全文件无一处 `tauri::` |
| 仅 `tauri::async_runtime`（+命令属性） | `t_fts` `t_embed` `t_af_area` `t_montage`；`t_image` 仅多一处 `&AppHandle` | 换成 `tokio::spawn/spawn_blocking` 即可，机械改动 |
| 需真适配（emit / State / Manager / 路径解析） | `t_sqlite`（1 处 emit+handle）`t_dedup` `t_similar` `t_ai` `t_face` `t_vl_schedule` `t_vl` `t_utils` `t_video` `t_cmds`（巨石，另含库切换锁与守卫） | 对应 §3.2 的 EventSink / Paths / 状态注册表 |
| **桌面壳保留** | `main.rs`、`t_cmds` 包装层、`t_drag` `t_menu` `t_protocol` `t_startup` `t_pasteboard` `t_http`（桌面 Linux 视频流） | 窗口 / 菜单 / 自定义协议 / 剪贴板 / OS 集成 |

- **原生构建必须随模块迁移**：`build.rs` 的 4 个 C 构建、`third_party/` 子模块与 `jpeg_shim.cpp` / `libraw_shim.cpp` 要跟着 `t_image`/`t_libraw` 等进 `lap-core`（或独立 `lap-native` crate）——否则 `lapd` 链接不到图像解码。
- **准入守卫**：模块迁入 `lap-core` 的判据写进 CI 脚本——「文件内 `rg 'tauri::'` 为零」；`async_runtime` 例外清单显式列出。

### 3.2 内核去 Tauri 化（本方案的关键低成本决策）

**不做全面依赖注入。** lap 是单进程、单 `current_library_id` 的模型；`lapd` 同样是单进程、单当前图库。因此进程级注入足够：

| 注入点 | 现状 | 目标 |
|---|---|---|
| 路径 | `app.path().resource_dir()/app_cache_dir()` | `OnceLock<PathsConfig>`：桌面由 Tauri 初始化，`lapd` 由 `--data-dir` / exe 旁目录初始化 |
| 事件 | 46 处 `app.emit(...)`，23 个事件名 | `OnceLock<Arc<dyn EventSink>>`；桌面适配到 `emit`，服务端适配到 SSE 广播 |
| 命令状态 | 45 条 B 类命令注入 `State<T>` | 状态结构本身与 Tauri 无关；壳内 `State` → `&T` 传给内核，服务端用进程级注册表 |
| fs scope | `Scopes::allow_directory` | 仅桌面壳需要；服务端以图库根作为唯一路径白名单 |

- 事件名与 payload **保持不变**，前端 `listen(name)` 在两种传输下语义一致。
- 明确不做的：把 `load_app_config`/`CONN_POOL`/迁移守卫改成实例化对象——收益低、改动面巨大，且与单用户模型冲突。
- **`t_cmds.rs` 拆分规则**：`#[tauri::command]` 包装（参数解包、`State` 取用、结果包装）留在壳；命令体抽成无 Tauri 的自由函数按领域归位。`with_current_library` 与 `FILE_REFRESH_LIBRARY_LOCK`（`t_cmds.rs:40/59`）本身不依赖 Tauri，随命令体进内核。
- `tauri::async_runtime` 就是全局多线程 tokio runtime：内核一律直接用 `tokio::spawn / spawn_blocking / block_on`，语义不变（含现有 `block_on` 用法）。

### 3.3 前端传输层

- 新增 `src/common/transport/`：导出与 `@tauri-apps/api/core` **同签名**的 `invoke`/`Channel`，与 `@tauri-apps/api/event` 同签名的 `listen`/`emit`，外加 `assetUrl()` 与 `capabilities()`。
- 迁移动作是**改 15 个文件的 import 一行**（228 个调用点原地不动）；`common/vlApi.js` 已有的可注入 `invoker` 模式继续保留给测试。
- `Channel` 只有 2 条命令使用（缓存迁移、出窗拖拽）；后者是 C 类，Web 端不需要。
- `capabilities()` 返回 `{ native: boolean }`：Web 端隐藏目录选择、资源管理器定位、原生拖拽、更新器、壁纸等入口。
- **多窗口 → 同页路由**：桌面上的 `imageviewer/imageeditor/montage/settings/vl-lab` 是独立 `WebviewWindow`；Web 端改为同一 tab 内的 router 路由。跨窗口事件总线的大头（303 emit / 110 listen）在 Web 端退化为 Vue 状态，SSE 只承载后端进度事件。
- 媒体 URL 仍由 `getThumbUrl` / `getPreviewUrl` / `getAssetSrc` 三个函数产出，内部改走 `assetUrl()`，调用方零改动。
- **平台与区域信息以服务端为准**：现状 `common/utils.ts:22-25` 用 `navigator.userAgent` 推导 `isWin/isLinux/separator`，Web 模式下客户端平台与服务端无关。新增只读 `server_info`（服务端平台、路径分隔符、版本、能力位），Web 传输用它覆盖这些常量；日期区域用浏览器 locale，不再调 `get_system_region_locale`（那是服务端的区域）。
- Windows 的缩略图 data-URL 通路（`getCachedThumbnailDataUrl`）在 Web 模式跳过，直接请求 `/api/thumb`，避免 base64 过网络。

### 3.4 事件与进度（SSE）

- `GET /api/events`（`text/event-stream`）；事件名、payload 与 Tauri 完全一致。
- 断线重连：`Last-Event-ID` + 服务端 30 秒环形缓冲；心跳 15 秒。
- 进度类事件（VL 每文件、索引、去重、人脸、模型下载）在 `EventSink` 内**按 ≥100ms 合并**，避免网络风暴。
- 前端 Web 传输把 `listen(name)` 实现为 EventSource 分发；同 tab 的内部事件直接走 Vue 状态或 `mitt`，不经过网络。

### 3.5 媒体服务

| 端点 | 内容 | 说明 |
|---|---|---|
| `GET /api/thumb/{library_id}/{file_id}?size=` | 缩略图 | 复用现有磁盘缓存与生成逻辑 |
| `GET /api/preview/{library_id}/{file_id}?…` | 预览/编辑用图 | 保留现有 query 语义 |
| `GET /api/original/{file_id}` | 原图 | 支持 `Range`，206 |
| `GET /api/video/{file_id}` | 视频 | 支持 `Range`，206；替换 Linux 专用 `t_http` |

- **只按 DB 的 `file_id` 解析真实路径**，绝不接受调用方传路径——这同时修掉 `t_http.rs` 现有的任意路径读取 + `CORS *`。
- `<img>`/`<video>` 无法携带 `Authorization` 头，因此媒体 URL 使用 **HMAC 签名**：`?exp=…&sig=…`（绑定路径 + 设备 + 10 分钟有效期）。命令接口仍用 Bearer/Cookie。
- 原图/视频走流式输出（`ReaderStream` + `ServeFile` 语义），不 base64、不整块读内存。

前端构造点与端点的对应关系（`assetUrl()` 的内部实现）：

| 前端构造点 | Web 端点 |
|---|---|
| `getThumbUrl`（`utils.ts:575`） | `/api/thumb/{library_id}/{file_id}?size=` |
| `getPreviewUrl`（`utils.ts:597`） | `/api/preview/{library_id}/{file_id}?…`（保留现有 query 语义） |
| `getAssetSrc`（`utils.ts:765`）· 图片 | `/api/original/{file_id}` |
| `getAssetSrc` · 视频 | `/api/video/{file_id}` |

- 桌面 Linux 的 `t_http` **保留但收紧**：仅回环监听（现状即 `127.0.0.1:0`）、去掉 `Access-Control-Allow-Origin: *`、只允许图库内路径；`lapd` 不复用它（`lapd` 用上面的签名端点）。

### 3.6 鉴权与暴露面

- **单用户 + 设备令牌**：首次启动设置管理员密码（Argon2id）；控制台打印 6 位配对码（5 分钟有效）；设备用它换取 32 字节随机令牌，服务端只存 SHA-256。
- Web 端登录后用 `HttpOnly` + `SameSite=Strict` Cookie（TLS 下加 `Secure`）；非浏览器客户端用 `Authorization: Bearer`。
- **命令白名单**：按配套清单的暴露等级（只读 110 / 写入 87 / 管理员 38 / 不暴露 18）生成；默认拒绝，未列名即 403；C 类命令名字根本不出现在白名单。
- 管理员级命令仅接受 localhost 或带 `admin` scope 的令牌；`set_vl_config`（含密钥）归管理员。
- 限流与审计：登录 5 次/分/IP、失败锁定、命令 60 次/分/设备；记录登录与管理员操作。
- Web 构建开启 CSP、关闭 `withGlobalTauri`；桌面构建维持现状（后续单独评估 CSP）。
- **TLS 不在 MVP 内**：文档给出两种推荐部署——Caddy 反代（自动证书）或 Tailscale/WireGuard（零证书管理）。**禁止**把明文 HTTP 直接映射到公网。

### 3.7 数据所有权与生命周期

- **一个图库同一时刻只有一个写进程**：
  - `lapd` 启动时创建 `<data>/lapd.lock`（PID + 端口 + 启动时间）；
  - 桌面内嵌模式启动时检测锁文件，若被占用则提示「已有服务在运行，是否以客户端模式连接」；
  - 桌面**客户端模式绝不直接打开 SQLite**。
- 迁移：`lapd` 启动执行现有 `check_and_migrate`。服务化会新增 `device_tokens` 表（v29）；**release 构建才可指向真实图库**，debug/portable 继续使用隔离数据目录（既有规则不变）。
- 退出：SIGTERM/Ctrl-C 优雅关闭——停止接受新命令、取消可取消的后台任务、flush WAL。
- 日志：滚动文件；密钥沿用现有 `redact()`，不写日志。

**`lapd` 启动序列**（与 `main.rs:176-221` setup 逐项对照，这就是"无 Tauri 的启动"意味着什么）：

| 桌面 setup 步骤 | `lapd` 对应 |
|---|---|
| 启动阶段遥测 / 菜单 / 窗口装饰 | 无（写日志） |
| `init_ffmpeg_path(resource_dir)` | `Paths` 初始化后指向 exe 旁 `ffmpeg/`（沿用便携布局回退） |
| `set_app_identifier(config().identifier)` | 常量注入 `com.julyx10.lap` |
| `create_db()`（打开 + 迁移 + XMP init + VL 收尾） | 相同 |
| `init_video_http_server`（仅 Linux） | 不需要（`lapd` 统一走签名媒体端点） |
| `init_video_cache(app_cache_dir)` | 相同（经 `Paths`） |
| `restore_album_scopes` | 相同 |
| `spawn_scheduler`（VL 计划任务） | 相同 |
| `finish_startup`（前端触发的 AI 模型加载、可访问性刷新） | **启动时自行执行**，不等前端 |

**版本偏斜边界**：迁移只前进（`check_and_migrate` 只应用 version > 当前）。同一数据目录上 lapd 与桌面**不要跨版本先后打开**——新版本写入的新 schema / 新 `user_version` 会让旧端误读。规则：升级时先升 `lapd`（或同时），桌面版本不得低于 `lapd`。

**锁文件在阶段 5 之前的行为**：桌面启动遇锁只能提示「lapd 正在运行，请关闭服务或使用 Web 界面」并退出；「以客户端模式连接」要等阶段 5 实现。

**共享语义边界**（单用户多设备，必须向用户说明的行为）：

| 状态 | 归属 | 后果 |
|---|---|---|
| `current_library_id`、库列表、`save_library_state`（每库 UI 状态） | 服务端全局 | 一台设备切换图库，**所有设备跟着变** |
| `configStore`（主题/语言/布局等 UI 设置） | 各设备 localStorage | 每台设备各自记忆 |
| `isWin`/分隔符/区域等"环境"描述 | **服务端** | 客户端显示以 `server_info` + 浏览器为准（见 §3.3） |

### 3.8 平台与服务化

| 平台 | 形态 | 要点 |
|---|---|---|
| Linux | systemd **user** unit | `systemctl --user enable --now lapd`；需要开机即启时说明 `loginctl enable-linger` |
| macOS | LaunchAgent | 首次运行需授予「完全磁盘访问」才能读取照片目录 |
| Windows | WinSW / NSSM 包装 | 服务账户需对图库路径有读权限；不建议 LocalSystem |

- 默认监听 `127.0.0.1:17860`；参数：`--host`、`--port`、`--data-dir`、`--library`。
- 资源：`resources/ffmpeg/` 需补齐 macOS/Linux（现有下载脚本）；`ort` 动态库随二进制同目录并设置 rpath/`LD_LIBRARY_PATH`。
- 数据目录沿用现有 `dirs::data_local_dir()/com.julyx10.lap`，让服务与桌面共享配置与图库。

### 3.9 Web 端能力边界（C 类命令在 Web 上的表现）

| 桌面能力 | Web 端处置 |
|---|---|
| 选择目录新增相册（`select_folder`） | **首版不支持**（无目录选择器）；管理员的「输入服务端绝对路径」受限入口见未决事项 |
| 拖拽导入 / 剪贴板导入 / URL 导入 | 不暴露（导入整体归管理员面，后续再议） |
| 在资源管理器中显示 / 外部程序打开 / 设壁纸 | 隐藏入口 |
| 自动更新 + 重启 | 不适用（PWA 由浏览器刷新；服务端由包管理器/WinSW 更新） |
| 原生出窗拖拽（拖到其他应用） | 隐藏；浏览器内部拖拽（选择、收藏夹）不受影响 |
| 多窗口（viewer/editor/montage/settings） | 同页路由替代（§3.3） |

> 原则：Web 端**只做浏览、检索、查看与轻量元数据编辑**；文件系统的增删导入归服务端管理员操作。

---

## 4. 分阶段计划

> 每阶段都有独立验收与回滚点；不通过验收不进入下一阶段。

### 阶段 0 · 基线与契约（0.5–1 天）

- 跑并记录基线：`cargo test`（cwd `src-tauri`）、`pnpm build`、各 `test:*`；把 8 个已知失败测试文件修复或显式隔离。
- 产出并提交命令清单（本文档配套）。
- **验收**：基线结果写入 `docs/dev/`；后续每阶段失败集合无新增。
- **回滚**：无代码改动。

### 阶段 1 · 前端传输抽象（2–4 天，不碰 Rust）

- 新增 `transport/`，15 个文件改 import；媒体三函数改 `assetUrl()`；`capabilities()` 门控 C 类入口。
- **验收**：`VITE_LAP_TRANSPORT=tauri` 与 `=http` 都能 `pnpm build`；transport 单测覆盖名称/参数/事件名映射；桌面端冒烟无变化。
- **回滚**：默认传输仍是 Tauri；改动是纯前端、可整体回退。

### 阶段 2 · 原地去 Tauri 化（1–2 周，主成本）

- **先不建 crate**：把浏览闭环（约 60–80 条命令，清单第 4 节列出的前两批）的命令体抽成无 Tauri 的函数，`#[tauri::command]` 包装保留在壳内。
- 引入 `PathsConfig`、`EventSink`、进程级状态注册表；替换 46 个 emit 点位与相关 `AppHandle` 用法。
- 加边界守卫：源码扫描确认「核心模块列表里不出现 `tauri::`」，作为 CI/脚本检查。
- **验收**：`cargo test --bin Lap` 与现有 mock 网关测试全绿；桌面端完整回归（库切换、搜索、缩略图、VL、任务队列）。
- **回滚**：按模块提交，可逐个 revert；C 类与未迁移模块完全不动。

### 阶段 3 · 打包 `lap-core` + `lapd`（1–1.5 周）

- 建 workspace 与 `lap-core`，把已 Tauri-free 的模块迁入（`crate::t_x` → `lap_core::t_x` 机械改写，编译器兜底）。
- **原生构建随模块走**：`build.rs` 的 4 个 C 构建、`third_party/` 子模块与 `jpeg_shim.cpp` / `libraw_shim.cpp` 迁入 `lap-core`（或 `lap-native` crate）；CI 产出三平台 `lapd`（`cargo build -p lap-server`）。
- `lap-server`：通用桥 `POST /api/cmd/{name}` + 媒体端点 + SSE + 鉴权限流 + 锁文件 + 优雅退出。
- 补齐 ffmpeg/ort 的三平台资源；systemd/LaunchAgent/WinSW 安装脚本。
- **验收**：局域网另一台设备用浏览器完成「登录 → 搜索 → 网格 → 原图/视频」；无令牌 401；C 类 403；路径穿越无效；桌面与 `lapd` 同时启动被第二个进程拒绝；真实图库 `user_version` 与完整性检查正常。
- **回滚**：`lapd` 是新增产物，桌面内嵌模式保持可用；内核迁移可按模块回退。

### 阶段 4 · 响应式 + PWA（1–2 周）

- 手机导航/网格/查看器手势；manifest + 最小手写 service worker（只缓存应用壳，不缓存媒体）；不引入 workbox（除非后续确有复杂缓存需求）。
- **验收**：手机视口无横向滚动、可安装、离线打开壳；Lighthouse PWA 可安装项通过。
- **回滚**：PWA 资源与响应式样式独立，可移除。

### 阶段 5（可选）· 桌面客户端模式 / 商店壳

- 桌面「连接服务器」模式；Capacitor 薄壳（仅在需要商店分发/系统级上传时）。

---

## 5. 命令契约（维护规则）

- 配套清单是**唯一契约**：新增/删除命令必须重跑 `python -I docs/dev/gen-command-inventory.py` 并提交差异。
- A/B 由签名自动判定；C 与暴露等级是策展清单，新增命令默认落入分组默认值，**必须人工复核**。
- 阶段 2 起增加注册表守卫：命令总数与清单不一致时测试失败（沿用 `VL_COMMANDS` 的既有模式）。
- 未列入 HTTP 白名单的命令默认 403；C 类命令永不列入。

---

## 6. 风险登记册

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| R1 | 联网后的命令面 = 攻击面（任意 exe、SSRF、任意路径） | **高** | 白名单默认拒绝；C 类不暴露；媒体只按 file_id 解析；`t_http` 收紧为回环 + 去 `CORS *` + 限定图库路径；限流 + 审计 |
| R2 | 桌面与 `lapd` 同时写同一图库（全局连接池/迁移无跨进程锁） | **高** | 锁文件 + 第二进程拒绝；客户端模式不开库；文档明确「一个图库一个写者」 |
| R3 | 事件风暴（VL 每文件 emit） | 中 | EventSink 合并限频；SSE 心跳与环形缓冲 |
| R4 | 命令迁移回归（216 条巨石的路径改写） | 中 | 按浏览闭环分批；先原地改造再打包；530 个 Rust 测试 + mock 网关 |
| R5 | CI 不跑测试，重构无自动化安全网 | **高** | 阶段 0 固基线；阶段 2 把 `cargo test` 与前端域测试接入 CI（至少 PR 必跑） |
| R6 | 跨平台打包（ffmpeg 只有 Windows、ort 动态库、C 子模块） | 中 | 阶段 3 单独做三平台构建与冒烟；CI 已有三平台构建可复用 |
| R7 | 移动端 UX 被低估（现状仅 5 处 media query、固定 1200×800） | **高（工期）** | 先平板/桌面浏览器，手机版单列阶段；先只做浏览/搜索/查看，不做编辑 |
| R8 | 与上游 julyx10/lap 的 diff 扩大，后续同步痛苦 | 中 | 新功能尽量进新 crate/新模块；对上游文件的改动保持最小、加适配层 |
| R9 | 许可（GPL-3.0 无网络条款；InsightFace 模型非商业） | 低（自用）/ 高（对外分发） | 自用不受影响；对外分发前单独处理模型许可与分发合规 |
| R10 | 服务权限过大（Windows LocalSystem、macOS 全盘访问） | 中 | 最小权限运行；文档明确服务账户与目录权限 |
| R11 | lapd 与桌面版本偏斜（同库先后打开，旧端误读新 schema） | 中 | 迁移只前进；升级顺序=先升 lapd 或同时，桌面版本不低于 lapd；写进升级指南 |

---

## 7. 成本估算（专注工作，非日历时间）

| 阶段 | 内容 | 预估 |
|---|---|---|
| 0 | 基线 + 契约 | 0.5–1 天 |
| 1 | 前端传输抽象 | 2–4 天 |
| 2 | 原地去 Tauri 化（浏览闭环） | 5–10 天 |
| 3 | `lap-core` + `lapd` + 鉴权/媒体/服务化 | 5–8 天 |
| 4 | 响应式 + PWA | 5–10 天 |
| **MVP（0–3）** | 局域网 Web 浏览/搜索/查看，单用户带鉴权 | **约 2–3 周** |
| **可用产品（0–4）** | 含手机 PWA | **约 4–5 周** |

不确定项：阶段 2 的区间取决于「浏览闭环」实际牵出的 `t_cmds` 依赖面；阶段 3 的区间取决于媒体端点是否需要在第一阶段就做专用 REST（若通用桥的实测吞吐不足）。

---

## 8. 已否决的选项（及原因）

| 选项 | 否决原因 |
|---|---|
| 桌面 app `--serve` 内嵌服务 | Linux 服务器仍要拖 webkit2gtk；服务生命周期与 GUI 混在一起，无法作为 systemd 服务长期运行 |
| 桌面直接退化为纯客户端 | 破坏单机默认体验，风险与改动最大 |
| 先拆 crate 再改代码 | 路径改写、锁文件、CI 变更会与逻辑改造叠加，回滚面过大 |
| 全面依赖注入（trait 化所有全局） | 与单进程单图库模型冲突，收益低、改动面巨大 |
| 换 Go/Node 重写后端 | 双份业务逻辑，长期维护成本最高 |
| 原生 App（RN/Flutter）优先 | 与「低代价」目标冲突；PWA 覆盖浏览/搜索/查看 |
| 内置 ACME / 公网直连 | 证书生命周期与安全审计成本高；VPN/反代更稳 |
| 把 Tauri IPC 原样透传到网络 | 等于把「任意 exe 启动 + SSRF + 任意路径读取」开放给局域网 |

---

## 9. 未决事项

1. 默认端口是否就用 `17860`（需避开用户其他服务）。
2. `lapd install-service` 是自研子命令，还是只提供 systemd/plist/WinSW 模板脚本。
3. 是否增加 `--read-only` 模式（完全不加载写入面，用于「只给家人看照片」的最小暴露场景）。
4. 设备令牌管理 UI 放设置页的哪个分区（可与现有「图片理解」并列）。
5. PWA 离线壳的范围（仅壳 + 最近搜索历史，还是包含缩略图缓存）。
6. `server_info` 端点的字段集（服务端平台、路径分隔符、版本、能力位——Web 传输的常量覆盖来源）。
7. Web 端新增相册的形态：首版不支持，还是给管理员一个「输入服务端绝对路径」的受限入口。

---

## 10. 决策记录

| 日期 | 决策 | 依据 |
|---|---|---|
| 2026-11-15 | 独立 `lapd` + 桌面可选客户端 | 服务器不拖 GUI 依赖；服务化生命周期清晰；一次到位不返工 |
| 2026-11-15 | 只局域网 + 令牌 | 个人照片库的安全边界；避免内置证书管理的成本与风险 |
| 2026-11-15 | PWA 优先 | 复用同一套 Vue；移动端只做浏览/搜索/查看，成本最低 |
| 2026-11-15 | 先原地去 Tauri 化、后打包 crate | 把路径改写/workspace/CI 风险推迟到代码 Tauri-free 之后 |
| 2026-11-15 | 进程级注入（Paths/EventSink/State），不做全面 DI | 与单进程单图库模型一致，改动面最小 |
| 2026-11-15 | 多窗口 → Web 端同页路由 | 消除跨窗口事件总线的大头改造 |