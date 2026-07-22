# YAP-XFISH

Windows 上的 **Sing-box 图形客户端**：Rust 后端 + 本地 Web 管理面板 + 系统托盘。  
绿色便携、单文件风格分发（`yap-xfish.exe` + `sing-box.exe`），数据与配置放在 exe 同目录的 `data/` 下。

> 仓库路径历史名可能仍为 `proxy-rs`；**产品名 / 二进制 / 托盘品牌均为 YAP-XFISH**。

---

## 功能概览

| 模块 | 说明 |
|------|------|
| 本地代理 | 启动时自动拉起 sing-box（失败不阻断面板） |
| 管理面板 | 默认 `http://127.0.0.1:9677` |
| 系统托盘 | 左键打开面板；右键菜单：启停 / 订阅更新 / 系统代理 / TUN / 开机启动 / 提权 / 退出 |
| 系统代理 | 写入 Windows Internet 设置（WinINET），退出时关闭 |
| TUN | 可选；需管理员权限 |
| 订阅 | Clash YAML / 节点链接；默认更新周期 **6 小时** |
| 节点 | 4 列卡片；点击选中；⚡ 测速 |
| 测速 | UIF 风格：临时 sing-box + Clash `/proxies/{tag}/delay`；无黑框、测后清理子进程 |
| 协议 | SS / VMess / VLESS（含 REALITY + utls）/ Trojan / Hysteria2 / TUIC 等 |
| 规则 | 全局 / 规则 / 直连；规则编辑与预设 |
| 单实例 | 命名互斥锁 `YapXfish_SingleInstance_Mutex` |

### 托盘状态点

| 状态 | 角标 |
|------|------|
| 系统代理开启 | 右上角 **亮紫色** 圆点 |
| TUN 开启 | 左上角 **亮黄色** 圆点 |

### 默认端口

| 用途 | 默认值 |
|------|--------|
| Web 管理面板 | `9677` |
| 本地混合代理（HTTP/SOCKS） | `10020` |
| Clash API | `9999` |

均可在「设置」中修改（Web 端口变更后需重启程序）。

---

## 截图说明（建议自行补充）

发布时建议在本段插入：

1. 管理面板 — 仪表盘  
2. 节点页（4 列卡片 + ⚡）  
3. 托盘右键菜单  

---

## 快速开始（使用发布包）

1. 从 [Releases](../../releases) 下载 `yap-xfish-v*-windows-amd64.zip`（或自行打包）。  
2. 解压到任意目录，保证同目录有：
   - `yap-xfish.exe`
   - `sing-box.exe`
   - `README.txt`（可选）
3. 双击 `yap-xfish.exe`（Release 为 GUI 子系统，无黑色控制台）。  
4. 浏览器打开：`http://127.0.0.1:9677`  
   或托盘左键 / 菜单「打开管理面板」。

### 数据目录

```
<解压目录>/
  yap-xfish.exe
  sing-box.exe
  data/                 # 配置、订阅、profile、日志、生成的 sing-box 配置
    app_config.json
    profile.json
    subscriptions.json
    config.json         # 渲染后的 sing-box 配置
```

- 删除整个文件夹 ≈ 卸载干净。  
- 移动整个文件夹即可迁移（核心路径优先相对 exe 目录解析）。  
- 首次启动若发现旧版 `%LOCALAPPDATA%\Proxy` 数据，会尝试复制到便携 `data/`（不删除旧目录）。

---

## 从源码构建

### 环境

- Windows 10/11 x64  
- [Rust](https://rustup.rs/)（stable，MSVC toolchain）  
- Node.js 18+（构建 WebUI）  
- 一份 `sing-box.exe`（与 `yap-xfish.exe` 同目录；可从 [sing-box Releases](https://github.com/SagerNet/sing-box/releases) 获取）

### 构建步骤

```bash
# 1. 前端
cd webui
npm install
npm run build
# 或: node ./node_modules/vite/bin/vite.js build

# 2. 后端（嵌入 webui/dist）
cd ..
cargo build --release

# 产物
# target/release/yap-xfish.exe
```

将 `sing-box.exe` 放到 `target/release/` 后运行：

```bash
./target/release/yap-xfish.exe
```

可选：用 `build_release_zip.py` 将 `release/yap-xfish.exe`、`sing-box.exe`、`README.txt` 打成 zip。

### 开发提示

- Debug 构建仍带控制台，便于排错。  
- 运行时数据目录默认：`exe 同目录/data/`。  
- 隔离调试可用环境变量 `PROXY_RS_DATA_DIR`（历史键名保留）；**注意**：单实例互斥锁与数据目录无关，同机只能跑一个实例。  
- 前端修改后需重新 `vite build`，再 `cargo build`，才会被 `rust-embed` 打进 exe。

---

## 项目结构

```
.
├── assets/                 # 托盘/应用图标、DPI manifest
│   ├── app.ico
│   ├── app.manifest        # PerMonitorV2，改善托盘菜单字体
│   └── tray_32.rgba        # 32×32 托盘底图（X-FISH）
├── src/
│   ├── main.rs             # 启动、DPI、自动开代理、系统代理恢复
│   ├── app.rs              # AppConfig / AppState / 事件
│   ├── config/             # 模型、读写、渲染 sing-box JSON
│   ├── core/               # 核心进程管理（CREATE_NO_WINDOW）
│   ├── latency/            # UIF 风格批量测速
│   ├── server/             # Axum REST + WebSocket + 静态资源
│   ├── subscription/       # 拉取、解析、定时更新
│   └── system/             # 托盘、系统代理、开机启动、单实例、DPI
├── webui/                  # Vue 3 + Vite 管理面板
├── release/                # 本地发布目录（勿提交 sing-box 大二进制）
├── build.rs                # 嵌入图标 + DPI 清单
├── build_release_zip.py
└── Cargo.toml              # package name: yap-xfish
```

---

## 订阅说明

- 支持常见 Clash 订阅与部分分享链接。  
- **名称未填**：去掉 `https://` / `http://` 后取前 **7** 个字符作为显示名。  
- **默认更新周期**：`every6_hours`（6 小时）；可在添加订阅时改为仅手动 / 30 分钟 / 1 小时等。  
- 部分机场会拦截 UA：可在订阅设置中填写自定义 User-Agent。  
- 「全部更新」**不会**更新 `interval = manual` 的订阅。  
- **停用**的订阅：节点不出现在节点列表，但更新成功后 **节点数** 仍显示真实数量。

---

## 测速说明

当前实现参考 [UIforFreedom/UIF](https://github.com/UIforFreedom/UIF)：

1. 为待测节点生成临时 sing-box 配置 + `experimental.clash_api`  
2. 无窗口启动临时 core（`CREATE_NO_WINDOW`）  
3. 请求 `GET /proxies/{tag}/delay`  
4. 结束后杀进程并清理临时目录  

协议节点走真实出口延迟；避免残留 `sing-box.exe`。

---

## 系统代理与 TUN

- **系统代理**：勾选后写入  
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings`  
  指向 `127.0.0.1:<proxy_port>`，并通知 WinINET。  
  **退出程序时会关闭系统代理**，避免系统仍指向已退出进程。  
- **TUN**：写入 sing-box 配置中的 TUN inbound；未提权时会告警且可能无法创建网卡。  
  可通过托盘「以管理员身份运行」或设置页提权。

---

## 常见问题

**打不开管理面板 / 连接被拒绝**  
先确认托盘里 `yap-xfish` 是否在运行；地址是否为 `http://127.0.0.1:9677`（若改过 Web 端口则用新端口）。

**VLESS 大面积 `unknown version: 72`**  
多为 REALITY / fingerprint 未正确解析渲染。请重新「更新订阅」以写入 `tls.reality` 与 `tls.utls`。

**测速时弹黑框**  
应已通过 `CREATE_NO_WINDOW` 处理；若仍出现，请确认运行的是最新 Release 构建。

**托盘菜单字体发糊**  
程序启用 Per-Monitor DPI V2（运行时 + `assets/app.manifest`）。请使用最新构建。

**订阅节点数为 0**  
确认订阅已启用且更新成功；停用订阅的节点不会出现在节点页，但卡片上节点数应为解析到的真实数量。

**端口占用**  
在设置中修改代理端口 / Clash API 端口 / Web 端口。

---

## 技术栈

- **后端**：Rust, Tokio, Axum, serde, tray-icon / muda, winreg, windows-sys  
- **前端**：Vue 3, Vite, TypeScript  
- **内核**：sing-box（外置二进制，不随本仓库分发）

---

## 许可证

MIT（见 `Cargo.toml` 中 `license` 字段）。  
`sing-box` 及其规则集遵循各自上游许可证。

---

## 致谢

- [sing-box](https://github.com/SagerNet/sing-box)  
- [UIforFreedom/UIF](https://github.com/UIforFreedom/UIF)（测速思路参考）  
- Clash / mihomo 生态订阅与延迟 API 约定  

---

## 开发者自测（可选）

本项目以端到端 ad-hoc 验证为主，而非完整测试套件。后端契约变更后建议：

1. `cargo build --release` + `vite build`  
2. 隔离 data 目录拉起二进制  
3. 用 REST（及必要时 WebSocket）断言变更行为  
4. 结束后恢复用户实例（注意 Windows **命名互斥锁** 单实例，不能靠换 data 目录并行跑两个进程）

---

**YAP-XFISH** — 小而完整的 Windows Sing-box 桌面客户端。
