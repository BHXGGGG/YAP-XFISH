# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

**主要用户：中文 Windows 用户，需要在本机跑 sing-box 上外网。**

公开产品，任何 Windows 用户都可能下载使用，因此用户分两拨，必须同时服务：

- **老手**：知道订阅、节点、机场、链式前置、TUN、分流规则分别是什么，冲着"低内存、无 Electron、单 exe 便携"来的。他们从 v2rayN / Clash Verge / NekoRay 一类客户端迁移过来，会对比内存占用。
- **新手**：只知道"我要能上网"，从社区帖或搜索引擎进来，拿到一个订阅链接。他们不认识"规则 / 直连 / detour"这些词，第一次打开时通常还没有任何订阅。

**成功的样子**：装好、导入订阅、选中一个能通的节点、代理跑起来 —— 全程不需要命令行，也不需要在设置里猜。

## Product Purpose

Windows 上的 sing-box 图形客户端：Rust 后端 + 本地 Web 管理面板 + 系统托盘。

存在的理由很具体：**替掉 Electron 系客户端的内存开销**。启动即拉起 sing-box，面板默认在 `http://127.0.0.1:9677`，托盘常驻。发布形态是 `yap-xfish.exe` + `sing-box.exe` 两个文件加一个 `data/` 目录，绿色便携，删目录即卸载。

## Positioning

**原生 Rust + 本地 WebUI，用浏览器当界面，不背 Chromium。**

竞品堆一个 Electron/Tauri 运行时换来的界面能力，这里用系统自带浏览器换掉，代价是常驻内存从几百 MB 降到几十 MB 量级。这个取舍不是视觉风格，是技术栈级的选择 —— 邻居产品无法在不重写整个前端宿主的前提下照抄。

配套的第二个差异点：**订阅自动更新**（默认 6 小时）是其参考项目 UIF 没有的能力。

## Operating Context

- 面板由托盘左键或托盘菜单拉起，在系统浏览器里打开；用户不会长时间把它当独立应用挂着。
- **使用场景是观察型**：用户会打开看实时延迟、30 天流量、日志。仪表盘是主面板，不是欢迎页。
- 典型进场状态是"带着一个机场订阅链接来的"，机场可能拦非浏览器 UA。
- TUN 需要管理员权限，未提权时会告警 —— 这是常态路径，不是异常路径。
- 数据目录 `<exe 同目录>/data/` 就是整个应用状态：`app_config.json`、`profile.json`、`subscriptions.json`、渲染出的 `config.json`、日志。移动目录即迁移。
- 同一台机器只能跑一个实例（命名互斥锁 `YapXfish_SingleInstance_Mutex`），与数据目录无关。

## Capabilities and Constraints

**界面模块**（`webui/src/views/`）：仪表盘、订阅管理、路由规则、链式代理、设置、实时日志。侧栏常驻：连接状态 + 当日与 30 天流量。

**已确认能力**：本地代理启停；订阅增删改与定时更新（manual / 30 分钟 / 1 小时 / 6 小时）；节点卡片选中；批量测速（复用运行核心的 Clash delay API，核心未运行时创建临时核心）；规则编辑与预设；模式切换（全局 / 规则 / 直连）；链式代理前置（detour）；系统代理（写 WinINET，退出时关闭）；TUN；开机启动；检查 GitHub 更新。

**协议**：HTTP(S) 代理 / SOCKS5 / SS / VMess / VLESS（含 REALITY + utls）/ Trojan / Hysteria2 / TUIC。

**固定默认值**（用户可改，改动后需重启的会标注）：Web 面板 `9677`、混合代理 `10020`、Clash API `9999`。测速默认并发 50（可调 1–200），超时 5 秒，测试地址 `https://www.gstatic.com/generate_204`。

**技术约束（后续任何界面工作都不能绕开）**：

- 前端是 Vue 3 + Vite + TypeScript，**无 UI 框架、无前端路由、无状态库**。视图切换靠 `App.vue` 里的 `<component :is>`，状态是手写的 `store.ts`，样式是 `style.css` 里的 CSS 变量加各视图的 `<style scoped>`。
- 构建产物由 `rust-embed` 打进 exe。改前端必须 `vite build` 再 `cargo build` 才能生效；**不能引入需要额外构建步骤或运行时依赖的方案**。
- 后端是 Axum，提供 REST + WebSocket，前端所有请求同源发往 `127.0.0.1:<web_port>`。
- 无前端测试套件。README 明确项目以端到端 ad-hoc 验证为主。

**已知缺口**：`webui/src/views/Profiles.vue`（节点管理，195 行）存在但未挂到侧栏 tab；`webui/vite.config.ts` 没有配 API proxy，开发态跑 `vite dev` 拿不到 `/api/*` 数据，需要连带启动 Rust 后端。

## Brand Commitments

- **名称**：YAP-XFISH（包名 `yap-xfish`）。品牌字标在侧栏渲染为 `YAP` + `-XFISH`，后半用主色。
- **界面语言：中文，永久。** 不规划 i18n，文案按中文习惯写，不必为英文长度预留空间。
- **托盘状态点颜色是既定约定**：系统代理开启 = 右上亮紫色圆点；TUN 开启 = 左上亮黄色圆点。
- **图标资产**：`assets/app.ico`、`assets/tray_32.rgba`（X-FISH 32×32 托盘底图）。
- **既定口号**（README）：「少占一截内存，多留一截流畅。」「把重量留给线路，不留给壳子。」
- **许可**：MIT。sing-box 为外置二进制，不随仓库分发。

## Evidence on Hand

- `README.md`：完整功能表、端口表、数据目录说明、订阅与测速规则、系统代理与 TUN 行为、FAQ、构建步骤。这是产品事实的主要来源。
- `release/`：v0.1.22 起的多个历史发布包（当前 `Cargo.toml` 版本 0.1.44），可提取真实版本演进。
- `design/`：订阅页的静态 HTML 原型（`subscriptions.html`、`subscriptions-expanded.html`）与截图，未纳入 git。属探索稿，不是权威视觉规范。
- `assets/`：应用图标、DPI manifest、托盘底图。

**不存在的**：没有用户证言、评测、对比基准数据、下载量或 star 数记录，没有真实营销素材。后续任何界面**不得编造**这些内容。README 里的内存优势目前是论断，没有实测数字支撑。

## Product Principles

1. **内存与包体是一等功能需求。** 任何界面增强若显著推高常驻内存或产物体积，让步。
2. **面板是遥控器，不是门面。** 用户来是为了完成动作（换节点、开代理、看延迟），一步到位优先于表达欲。
3. **面向陌生人设计。** 公开分发意味着第一次打开可能是零订阅的空状态；空状态、错误、提权失败都要自己解释清楚，不能假设用户读过 README。
4. **中文优先，说人话。** 老手看得懂术语，新手看不懂；术语首次出现要能自己立住。
5. **便携性不可破坏。** 不往 `data/` 之外写文件，不要求安装程序，不依赖系统级服务。

## Accessibility & Inclusion

未建立产品级无障碍标准。已知的硬约束：Windows 高 DPI（程序启用 Per-Monitor DPI V2）下，面板与托盘在 100%–200% 缩放下必须可读、不裁切。键盘可达性与屏幕阅读器支持目前未作要求。
