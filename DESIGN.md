---
name: YAP-XFISH
description: 原生 Rust 内核、浏览器当界面的 Windows sing-box 客户端
colors:
  primary: "#2563eb"
  primary-bright: "#3b82f6"
  primary-deep: "#1d4ed8"
  primary-tint: "#93c5fd"
  primary-wash: "#eff6ff"
  canvas: "#f5f6f8"
  surface: "#ffffff"
  surface-sunk: "#f8fafc"
  border: "#e6e8eb"
  border-strong: "#d1d5db"
  ink: "#1f2329"
  ink-strong: "#0f172a"
  ink-soft: "#374151"
  muted: "#8a9099"
  muted-soft: "#6b7280"
  muted-faint: "#9ca3af"
  success: "#16a34a"
  success-ink: "#166534"
  success-wash: "#dcfce7"
  danger: "#dc2626"
  danger-ink: "#991b1b"
  danger-wash: "#fee2e2"
  warn: "#f59e0b"
  warn-ink: "#92400e"
  warn-wash: "#fef3c7"
  log-shell: "#0f1115"
  log-ink: "#d6e0ff"
typography:
  headline:
    fontFamily: 'system-ui, "Segoe UI", "Microsoft YaHei", sans-serif'
    fontSize: "19px"
    fontWeight: 700
    lineHeight: 1.35
  title:
    fontFamily: 'system-ui, "Segoe UI", "Microsoft YaHei", sans-serif'
    fontSize: "16px"
    fontWeight: 700
    lineHeight: 1.4
  value:
    fontFamily: 'system-ui, "Segoe UI", "Microsoft YaHei", sans-serif'
    fontSize: "18px"
    fontWeight: 600
    lineHeight: 1.3
  body:
    fontFamily: 'system-ui, "Segoe UI", "Microsoft YaHei", sans-serif'
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.6
  data:
    fontFamily: 'system-ui, "Segoe UI", "Microsoft YaHei", sans-serif'
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.45
  label:
    fontFamily: 'system-ui, "Segoe UI", "Microsoft YaHei", sans-serif'
    fontSize: "12px"
    fontWeight: 600
    letterSpacing: "0.2px"
  micro:
    fontFamily: 'system-ui, "Segoe UI", "Microsoft YaHei", sans-serif'
    fontSize: "10px"
    fontWeight: 700
    letterSpacing: "0.4px"
  mono:
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.5
rounded:
  micro: "3px"
  sm: "4px"
  md: "6px"
  control: "8px"
  badge: "9px"
  card: "10px"
  dialog: "14px"
  pill: "999px"
spacing:
  hair: "4px"
  xs: "6px"
  sm: "8px"
  md: "10px"
  base: "12px"
  lg: "14px"
  xl: "16px"
  xxl: "20px"
  page-top: "28px"
  page-side: "34px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.surface}"
    rounded: "{rounded.control}"
    padding: "8px 18px"
    height: "36px"
  button-primary-hover:
    backgroundColor: "{colors.primary-deep}"
    textColor: "{colors.surface}"
    rounded: "{rounded.control}"
    padding: "8px 18px"
  button-quiet:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "8px 16px"
    height: "36px"
  button-quiet-hover:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.primary}"
    rounded: "{rounded.control}"
    padding: "8px 16px"
  nav-item:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "10px 12px"
    width: "176px"
  nav-item-active:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.surface}"
    rounded: "{rounded.control}"
    padding: "10px 12px"
    width: "176px"
  card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.card}"
    padding: "16px"
  node-card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.card}"
    padding: "10px 12px"
  node-card-selected:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.card}"
    padding: "10px 12px"
  protocol-chip:
    backgroundColor: "{colors.primary-bright}"
    textColor: "{colors.surface}"
    rounded: "{rounded.micro}"
    padding: "1px 7px"
  field:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "8px 10px"
    height: "34px"
  field-focus:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "8px 10px"
  pill-badge-on:
    backgroundColor: "{colors.success-wash}"
    textColor: "{colors.success-ink}"
    rounded: "{rounded.pill}"
    padding: "3px 12px"
  pill-badge-off:
    backgroundColor: "{colors.danger-wash}"
    textColor: "{colors.danger-ink}"
    rounded: "{rounded.pill}"
    padding: "3px 12px"
  log-console:
    backgroundColor: "{colors.log-shell}"
    textColor: "{colors.log-ink}"
    rounded: "{rounded.card}"
    padding: "14px"
  dialog-card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.dialog}"
    padding: "18px 22px"
---

# Design System: YAP-XFISH

## Overview

**Creative North Star: "安静的仪表台" (The Quiet Instrument Panel)**

这是一个你打开只为*看一眼*的界面。用户不是来欣赏页面的，是来确认「连上了吗、延迟多少、这个月跑了多少流量」——看完就走。所以整个系统的职责不是表达，是读数：数字在最前面，装饰在后面，颜色只在状态变化时才出现。界面安静，是因为一个能穿透代理的客户端不该让人紧张。

密度上这是一个偏紧的工具型面板：正文 14px 只用在侧栏和长文，数据面大量落在 12px，卡片内边距 10–16px，行高 1.3–1.45。留白不服务于「呼吸感」，服务于分组——用 4/6/8/10px 的间距把标签和数值粘成一个读块，再用 14/16px 把读块彼此分开。视觉层级靠字重（400/600/700）和灰度，不靠尺寸跳跃：整个系统最大字号只有 19px，没有 hero，没有展示型排版。

材质是纸，不是玻璃。静止的面永远是平的：白面板 + 1px 冷灰描边 + 10px 圆角。阴影是事件，不是常态——只有当某样东西浮在页面之上（对话框、toast）或者需要被按下（主行动按钮）时，它才获得阴影。选中的节点不用填充色，用一圈 1px 的内描边，因为它仍然是平的，只是被圈住了。

**Key Characteristics:**
- 读数优先：数值 18px/600，标签 12px、灰、在数值上方。
- 平到底：默认零阴影，描边代替投影。
- 单一蓝：主色只出现在「这一屏唯一的主动作」上；其余颜色全是状态语义（绿=通、红=断、琥珀=慢）。
- 中文原生：字号、行高、字重按中文渲染调整，不做西文优先的紧凑排版。
- 圆角只有一套语汇：3px 芯片 / 8px 控件 / 10px 卡片 / 14px 对话框 / 999px 药丸。

## Colors

冷白底 + 单一信号蓝 + 三色状态语义。整个调色板是无彩度的纸面，颜色只用来标记「发生了什么」。

> **关于两套蓝的史实**：颜色来自两个时代。壳层（`webui/src/style.css`）是一套 Ant Design 风格的 CSS 变量（原 `--primary: #4096ff`）；视图层（各 `.vue` 的 scoped 样式）是硬编码的 Tailwind 蓝（`#3b82f6` / `#2563eb` / `#1d4ed8`）。**本文件以 Tailwind 蓝为规范**，`#4096ff` 已退役，不再新增。灰阶同理：壳层的 `#8a9099` 与视图层的 `#6b7280` / `#9ca3af` 都在用，各有职责，见下方 Named Rules。

### Primary
- **信号蓝 Signal Blue** (#2563eb)：主色。链接、选中文字、主数值、折线图下行走线、主按钮渐变下半段。它是「可操作」和「当前」的信号。
- **亮蓝 Bright Blue** (#3b82f6)：主按钮渐变上半段、WG 协议芯片底色、输入框聚焦描边。只在需要比 #2563eb 更亮一档时出现。
- **深蓝 Deep Blue** (#1d4ed8)：主按钮 1px 描边、链接 hover 文字。唯一职责是给渐变按钮一个「硬边」，否则按钮会发糊。
- **浅蓝 Tint Blue** (#93c5fd)：输入框 hover 描边、节点卡 hover 描边、「正在测速」卡片描边。最轻的一层蓝，只做反馈。
- **蓝洗 Wash Blue** (#eff6ff)：标签底色、测速中卡片底色、侧栏 hover 底色。用于「这块和蓝有关，但不重要」。

### Neutral
- **画布 Canvas** (#f5f6f8)：应用背景。比面板暗一档，让白面板浮出来——这是本系统唯一的「深度手段」。
- **面板 Paper** (#ffffff)：卡片、侧栏、对话框、输入框的底。
- **凹面 Sunk** (#f8fafc)：空状态、对话框头部渐变起点。
- **描边 Hairline** (#e6e8eb)：默认 1px 边框。系统里几乎每个分隔都是它。
- **重描边 Hairline-Strong** (#d1d5db)：表单输入框描边。比默认深一档，把可输入的框从卡片底上分出来。
- **正文 Ink** (#1f2329)：`body` 默认文字色。
- **标题 Ink-Strong** (#0f172a)：区块标题、对话框标题、节点名。靠这一点差建立层级。
- **次级 Ink-Soft** (#374151)：卡片内说明段落、日志徽标文字。
- **辅助 Muted** (#8a9099)：壳层 `--muted`，侧栏版本号、挂件文字。
- **辅助替代 Muted-Soft** (#6b7280)：视图层说明文字、字段标签。**新代码用这个**；`#8a9099` 只留在壳层既有位置。
- **占位 Faint** (#9ca3af)：`::placeholder`、空列表文案。不要再用于正文，白底上低于 4.5:1。

### Success / Danger / Warning
- **通 Success** (#16a34a) / **通-深** (#166534) / **通-洗** (#dcfce7)：连接指示灯的点、上行流量数字、`.badge.on` 药丸、顶部「系统代理已开」挂件。
- **断 Danger** (#dc2626) / **断-深** (#991b1b) / **断-洗** (#fee2e2)：`.badge.off`、错误日志徽标、失败计数。
- **慢 Warning** (#f59e0b) / **慢-深** (#92400e) / **慢-洗** (#fef3c7)：警告日志徽标、延迟劣化提示。

### 壳层变量的映射（`webui/src/style.css`）

壳层的 6 个语义变量现在就是本文件 token 的别名，改一处即可：

| CSS 变量 | 值 | 等于 |
| --- | --- | --- |
| `--bg` | #f5f6f8 | canvas |
| `--panel` | #ffffff | surface |
| `--border` | #e6e8eb | border |
| `--text` | #1f2329 | ink |
| `--muted` | #8a9099 | muted |
| `--primary` | #2563eb | primary |
| `--primary-d` | #1d4ed8 | primary-deep |
| `--green` | #166534 | success-ink（深档，保证药丸上的可读性） |
| `--green-bg` | #dcfce7 | success-wash |
| `--red` | #991b1b | danger-ink |
| `--red-bg` | #fee2e2 | danger-wash |
| `--radius` | 10px | rounded.card |

注意 `--green` / `--red` 刻意指向「深」档而不是 `success` / `danger` 本身：它们只用于浅底药丸上的文字，浅档会在 `#dcfce7` 上掉到 3:1 以下。

### Terminal
- **终端底 Log Shell** (#0f1115)：日志控制台背景。全系统唯一的大块深色，它标记「这是原始终端输出，不是我们的排版」。
- **终端字 Log Ink** (#d6e0ff)：日志正文。淡蓝白而非纯白，降低长段等宽文本的刺眼度。

### Named Rules
**The One Blue Rule.** 信号蓝只允许出现在每一屏的*一个*主行动上（连接、保存、测速），加少量文本链接。一个屏幕上出现两处渐变按钮就是错的——第二处应降级成静音按钮（白底 + 灰边）。

**The Retirement Rule.** `#4096ff` / `#2f80ed` 已退役。任何新样式都不许再引用，也不许引入 Tailwind 之外的第三套蓝。

**The No-New-Gray Rule.** 灰阶已有 6 档（`#1f2329` / `#0f172a` / `#374151` / `#8a9099` / `#6b7280` / `#9ca3af`）。新增第 7 种灰之前，先从这 6 档里挑。

## Typography

**Display Font:** 无（本系统不设展示字号）
**Body Font:** `system-ui, "Segoe UI", "Microsoft YaHei", sans-serif`
**Label/Mono Font:** `ui-monospace, SFMono-Regular, Menlo, monospace`

**Character:** 系统字体，零下载、零 FOIT，符合「便携、不背资源」的产品约束。中文靠 Microsoft YaHei，西文靠 Segoe UI，两者 x-height 差异由 1.3–1.6 的行高和偏小的字号补偿。界面里没有字距压缩、没有全大写西文标题——中文不做 letterspacing 游戏，只在 10px 协议芯片和 12px 字段标签上加 0.2–0.4px 的微量字距。

### Hierarchy
- **Headline**（700，19px，行高 1.35）：每页一个 `section h2`。系统里最大的字就是它。
- **Title**（700，16px）：对话框标题；`section h3` 用 15px + 辅助灰的变体。
- **Value**（600，18px）：卡片主数值（延迟、流量、连接数）。数值永远配一个 12px 的灰标签在它上面。
- **Body**（400，14px，行高 1.6）：壳层正文、侧栏导航、表单默认。长文本控制在 65–75ch。
- **Data**（400，12px，行高 1.45）：视图层默认字号——所有表格、列表元信息、说明文字、日志条目。
- **Label**（600，12px，字距 0.2px）：字段名、卡片标签（`.card .k`）。永远在它描述的数值之上。
- **Micro**（700，10px，字距 0.4px）：协议芯片（SS / VMEM / VLESS / TROJAN / HY2 / TUIC / WG）、侧栏统计次要数字。
- **Mono**（400，12px）：日志正文。任何按字符对齐的内容必须等宽 + tabular-nums。

### Named Rules
**The Twelve-Pixel Rule.** 数据面说 12px，壳层说 14px。往卡片、列表、表格里加文字，默认 12px；只有侧栏导航和长段落用 14px。这两个值之外的字号必须有理由（19px 标题、18px 数值、10px 芯片）。

**The Label-Above Rule.** 标签在数值上方，字号小两档，颜色辅助灰。反过来（黑体大字标签压着数值）会毁掉读数的第一眼。

**The Tabular Rule.** 任何会被上下对齐比对的数字（延迟、流量、时间、日志计数）必须 `font-variant-numeric: tabular-nums`，否则列会跳。

## Layout

固定左侧栏 + 单一滚动主区。没有响应式抽屉，没有面包屑，没有页脚。

- **壳层**：`.layout` 为 `display: flex; height: 100%`。侧栏 `.side` 固定 **200px**（内容宽 176px = 200 − 两侧 12px padding），白底，`border-right: 1px solid #e6e8eb`，纵向 `padding: 16px 12px`。主区 `.main` 为 `flex: 1; overflow: auto; padding: 28px 34px`。没有任何 `max-width` 容器——窗口多宽，内容就多宽。
- **侧栏三段**（靠 `margin-top: auto` 顶开）：品牌字标 YAP + -XFISH + 版本号（11px）→ 6 个导航项（`gap: 4px`）→ 底部流量统计块（`border-top` 分隔，11px，日列表 `max-height: 160px` 内部滚动）。
- **卡片网格**：基准 `.cards` 是 `repeat(auto-fill, minmax(160px, 1fr))`，`gap: 14px`。仪表盘覆写成 6 等分 + 12px 间距，并让状态卡与模式卡各占 2 格。节点网格 `.nodes-grid` 是 `repeat(4, minmax(0, 1fr))`，`gap: 10px`。
- **断点**（只有三档，全部只改网格列数，不改布局结构）：
  - `1280px` → 节点 3 列
  - `900px` → 节点 2 列、速率卡降为 1 格
  - `560px` → 节点 1 列
  - 另有 `1100px` / `640px` / `480px` 的一处表单降级，是局部补丁，不算系统断点。
- **表单网格**：设置页 `.settings-fields` 两列 `gap: 14px`；TUN 面板覆写成 `minmax(0,2fr) minmax(0,1fr)`。表单最大宽度 460px（`.form2`），对话框内表单 `max-width: calc(100vw - 32px)`。
- **间距节奏**：4 / 6 / 8 / 10 / 12 / 14 / 16 / 20。页级 28×34。**没有 24、32 这种大数**——这是工具面板，不是营销页。

## Elevation & Depth

**平到底。** 静止的表面永远平的：`background: #fff` + `1px solid #e6e8eb` + `border-radius: 10px`，阴影为零。深度只由三件事表达：(1) 画布 `#f5f6f8` 比面板暗一档；(2) 1px 描边的深浅（`#e6e8eb` → `#d1d5db`）；(3) 选中态的内描边 `box-shadow: 0 0 0 1px var(--primary) inset`——注意这是环，不是投影，物体仍在纸面内。

阴影只在两类情况下出现：**浮层**（对话框、toast——它们真的在页面之上）和**主行动按钮**（需要看起来可被按下）。卡片、列表、节点、设置区块，一律无阴影。

### Shadow Vocabulary
- **行动浮起**（`box-shadow: 0 4px 12px rgba(37, 99, 235, 0.25)`）：主行动按钮专用，蓝调投影，承担「这个按钮是立起来的」。
- **行动浮起-强**（`box-shadow: 0 6px 16px rgba(37, 99, 235, 0.32)`）：同上，hover/pressed 加强版。
- **选中环**（`box-shadow: 0 0 0 1px var(--primary) inset`）：节点卡片、列表项被选中。替代填充色，保持平面。
- **聚焦环**（`box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.18)`）：输入框 `:focus`。3px 软蓝雾，无位移。
- **活点光晕**（`box-shadow: 0 0 0 3px rgba(34, 197, 94, 0.2)`）：顶部「系统代理已开」挂件的绿点。状态灯的唯一一种发光。
- **对话框**（`box-shadow: 0 1px 0 rgba(255,255,255,0.6) inset, 0 24px 48px -12px rgba(15,23,42,0.35), 0 8px 16px rgba(15,23,42,0.18)`）：三层——顶部 1px 内高光当「纸的厚度」，两段中性投影当「离地高度」。
- **吐司**（`box-shadow: 0 4px 16px rgba(0,0,0,0.2)`）：toast 药丸的轻投影。

### Named Rules
**The Flat-By-Default Rule.** 静止的面零阴影。加阴影前先问：这东西浮在页面之上了吗？没有就用描边，不要用投影。

**The Ring-Not-Fill Rule.** 选中态用 1px 内描边环，不用填充背景、不用左侧色条。物体仍在纸面内，只是被圈住。

**The No-Glow-On-Paper Rule.** 彩色投影只允许出现在主行动按钮和状态灯上。把 `rgba(59,130,246,…)` 的发光加在静止卡片上是禁止的——这是最典型的 AI 界面味。

## Shapes

圆角是一套 5 档核心语汇（外加 4px / 9px 两个既有微调档），按「物体的物理尺寸」选，不按审美选：

- **3px** — 协议芯片（`.proto-tag`）。最小的可读块，接近方角，读起来像标签纸。
- **4px** — 微交互层：当日流量行 hover 底色。
- **8px** — **控件标准**：按钮、输入框、下拉、侧栏导航项、主按钮。
- **9px** — 日志计数徽标（`.badge-mini`），18px 高的小药丸，8px 会显得太方。
- **10px** — **卡片标准**（`--radius`）：`.card`、`.node-card`、`.settings-card`、日志控制台、空状态。
- **14px** — 对话框卡片。唯一的大圆角，标记「这是一层新的面」。
- **999px** — 药丸：状态徽标、连接状态、顶部挂件、toast、`.tag`。

描边一律 1px 实线（唯一例外是空状态的 `1px dashed`）。无 `border-left` 色条、无切角、无 `clip-path`、无渐变描边。图形都是字符或内联 SVG（协议芯片、趋势折线、状态点），不引入图标字体。

### Named Rules
**The Five-Radius Rule.** 核心只有 3 / 8 / 10 / 14 / 999 五档；4px 与 9px 是既有微调档，沿用但不扩展。看到 6px、12px、16px、18px，是有人没查这套语汇。

**The Square-Chip Rule.** 芯片要方、药丸要圆。协议芯片（信息，可密集并排）用 3px；状态徽标（状态，单独出现）用 999px。两者不可互换，那是信息与状态的分界。

## Components

### Buttons
- **Shape:** 8px 圆角，横向内边距 16–18px，纵向 8px，高 36px，字重 600（主）/ 400（静音）。
- **Primary:** 渐变 `linear-gradient(180deg, #3b82f6 0%, #2563eb 100%)`，1px 描边 `#1d4ed8`，白字，配 `0 4px 12px rgba(37,99,235,.25)`。这是「这一屏唯一的动作」。
- **Hover / Focus:** 主按钮 hover 加深渐变并升到 `0 6px 16px rgba(37,99,235,.32)`；静音按钮 hover 时描边转 `--primary`、文字转 `--primary`，`transition: .15s`。
- **Quiet（静音）:** 白底 + `1px solid #e6e8eb` + 墨色字，`padding: 8px 16px`。次级动作、取消、刷新这类反复可点的按钮全用它。
- **Disabled:** `opacity: .5` + `cursor: not-allowed`，不改颜色。
- **注意：** frontmatter 里的 `components.button-primary` 只能记单色（Stitch 的 8 属性集不支持渐变），渐变以本段为准。
- **反模式（现存技术债）:** 渐变样式被复制了 4 份（`webui/src/views/Dashboard.vue:764`、`webui/src/views/Logs.vue:287`、`webui/src/views/Settings.vue:329`、`webui/src/components/AddSubscriptionForm.vue:196`），而 `webui/src/style.css:60` 的 `button.primary` 仍是已退役的 `#4096ff` 平涂。统一时应把渐变收成一个 token 或一个 `.btn-primary` 类。

### Cards / Containers
- **Corner Style:** 10px。
- **Background:** 面板白 `#ffffff`；内部再分层时用 `#f8fafc` 或 `#eff6ff`。
- **Shadow Strategy:** 无。见 Elevation & Depth。
- **Border:** `1px solid #e6e8eb`。
- **Internal Padding:** 卡片 16px、节点卡 10px 12px（更密，因为它要 4 列并排）、设置区块 20px。
- **结构:** 标签（`.k`，12px 灰）在数值（`.v`，18px/600）之上，`margin-bottom: 8px`。

### Node Card（签名组件）
代理节点的标准砖块，仪表盘与（未挂载的）Profiles 页共用。
- 白底、10px 圆角、`1px solid #e6e8eb`、`padding: 10px 12px`、`gap: 4px` 竖排。
- **States:** 默认 → hover 描边转 `#93c5fd`；`.sel` 加 `0 0 0 1px #4096ff inset` 内环（应迁到新主色）；`.testing` 底色转 `#eff6ff` 且描边转 `#93c5fd`。全部 `transition: border-color .15s, box-shadow .15s, background .15s`。
- 第一行是名字（13px/600，单行省略）+ 协议芯片；第二行是延迟数字。

### Chips
- **Protocol chip（`.proto-tag`）:** 3px 圆角，`padding: 1px 7px`，10px/700，字距 0.4px，白字，底色按协议取色（WG = `#3b82f6`，其余协议各另有固定色）。
- **Tag（`.tag`）:** 999px 药丸，11px/500，`#2563eb` on `#eff6ff`。用于「默认」这类弱标记。
- **Status badge（`.badge`）:** 999px，`padding: 3px 12px`，13px/600。`.on` = `#166534` on `#dcfce7`；`.off` = `#991b1b` on `#fee2e2`。
- **Count badge（`.badge-mini`）:** 18px 高，9px 圆角，11px，`#374151` on `#e5e7eb`；`.warn` 琥珀、`.error` 红。

### Inputs / Fields
- **Style:** 白底、`1px solid #d1d5db`、8px 圆角、`padding: 8px 10px`、13px 字。下拉框去掉原生箭头，换成右 8px 处的内联 SVG 三角。
- **Hover:** 描边转 `#93c5fd`。
- **Focus:** 描边转 `#3b82f6`，加 `0 0 0 3px rgba(59,130,246,.18)` 聚焦环，`outline: none`。无位移、无放大。
- **Placeholder:** `#9ca3af`。
- **Checkbox:** 16×16，`accent-color: #3b82f6`，原生控件不做重绘。

### Navigation
- **侧栏项:** 8px 圆角，`padding: 10px 12px`，`gap: 10px`，18px 宽的字符图标槽（▦ ↻ ⚑ ⇄ ⚙ ☰），14px 字。
- **默认** 透明底 + 墨色字；**hover** 底色 `#f0f4ff`；**active** 实心 `--primary` 底 + 白字（注意：平涂，不是渐变——导航不是动作）。
- **无移动端形态:** 桌面客户端，窗口变窄时侧栏不折叠。

### Log Console（签名组件）
- `background: #0f1115`，字色 `#d6e0ff`，`font-family: ui-monospace, monospace`，12px，`padding: 14px`，10px 圆角，`max-height: 320px` 内部滚动，`white-space: pre-wrap`。
- 全系统唯一的大块深色面。它的作用是声明「下面是原始终端输出」，因此不套用任何排版语汇。

### Dialog（Modal）
- **遮罩:** `radial-gradient(ellipse at top, rgba(30,64,175,.18), rgba(15,23,42,.55))`，`padding-top: 10vh`，`z-index: 9999`，**不加 backdrop blur**。入场 `modal-mask-in .18s ease-out`。
- **卡片:** 白→`#fbfcff` 的竖向微渐变、14px 圆角、三层阴影（见 Elevation）、`1px solid rgba(15,23,42,.08)`、`max-width: calc(100vw - 32px)`。入场 `modal-card-in .22s cubic-bezier(.2,.8,.2,1)`。
- **头部:** `padding: 18px 22px 14px`，下描边，背景 `#f8fafc` 渐隐到透明。左侧 36×36 的 10px 圆角图标块，靛蓝→蓝渐变 `#6366f1 → #3b82f6`（**全系统唯一的靛蓝，漂移点，应收敛到蓝**），白字 18px/700，配 `0 4px 12px rgba(59,130,246,.35)`。
- **标题** 16px/700 `#0f172a` + 字距 0.2px；**副标题** 12px `#6b7280`。

### Toast
- 固定底部居中（`bottom: 24px`、`left: 50%`、`translateX(-50%)`），`#1f2329` 底 + 白字 + 999px 药丸 + `padding: 10px 20px` + `0 4px 16px rgba(0,0,0,.2)`。13px。

## Do's and Don'ts

### Do:
- **Do** 用 12px 做数据面默认字号，14px 只留给壳层与长段落（The Twelve-Pixel Rule）。
- **Do** 让标签在数值上方、小两档、辅助灰；数值 18px/600。
- **Do** 用 `border: 1px solid #e6e8eb` + `border-radius: 10px` 表达卡片，静止态零阴影。
- **Do** 用 1px 内描边环（`0 0 0 1px #2563eb inset`）表达选中，不用填充色。
- **Do** 每屏只给一个渐变主按钮；其余动作降级成白底静音按钮（The One Blue Rule）。
- **Do** 给数字加 `font-variant-numeric: tabular-nums`；日志和任何字符对齐内容用 `ui-monospace`。
- **Do** 只在 3 / 8 / 10 / 14 / 999 里选圆角。
- **Do** 用 150ms 的 `border-color`/`background` 过渡做状态反馈——够快，不会被察觉成动画。

### Don't:
- **Don't** 再引用 `#4096ff` / `#2f80ed`——已退役；新样式一律 `#2563eb` 家族（The Retirement Rule）。
- **Don't** 引入第 7 种灰；6 档之外先复用（The No-New-Gray Rule）。
- **Don't** 给静止卡片加彩色投影或发光（`rgba(59,130,246,…)` 的 glow 属于按钮和状态灯，不属于纸面）。
- **Don't** 把靛蓝 `#6366f1` 扩散出去——它目前只存在于对话框图标块，是待收敛的漂移，不是新副色。
- **Don't** 用超过 19px 的字号；系统没有 hero，也没有展示型排版。
- **Don't** 加 `backdrop-filter: blur()`——遮罩是渐变、不加模糊，这是低配机器上不掉帧的代价。
- **Don't** 复制按钮样式到各处；渐变已经复制了 4 份，第 5 份请先抽成 token。
- **Don't** 在侧栏导航 active 态用渐变——导航不是动作，用平涂 `#2563eb`。
