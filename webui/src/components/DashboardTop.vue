<script setup lang="ts">
// 仪表盘上半部分（参考 FlClash）：状态 / 网络速度 / 出站模式 / 内网 IP / 订阅 / 服务 / 内存。
// 下半部分的节点列表留在 Dashboard.vue。
import { computed, ref, watch, onMounted, onBeforeUnmount } from 'vue'
import { store, toast } from '../store'
import { api, type DashboardInfo, type ExitIp, type ServiceStatus } from '../api'

const emit = defineEmits<{
  (e: 'toggle', key: 'system_proxy' | 'enable_tun', value: boolean): void
  (e: 'navigate', tab: string): void
}>()

const modes: [string, string][] = [['rule', '规则'], ['global', '全局'], ['direct', '直连']]

/* ---------------- 格式化 ---------------- */

function fmtBytes(n: number): string {
  if (!n || n < 0) return '0 B'
  if (n >= 1024 * 1024 * 1024) return (n / 1024 / 1024 / 1024).toFixed(2) + ' GB'
  if (n >= 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + ' MB'
  if (n >= 1024) return (n / 1024).toFixed(1) + ' KB'
  return n + ' B'
}

/** 1536 → ['1.5', 'KB/s']，大数字与单位分开显示 */
function rateParts(n: number): [string, string] {
  const u = ['B/s', 'KB/s', 'MB/s', 'GB/s']
  let v = Number(n) || 0
  let i = 0
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++ }
  return [i === 0 ? String(Math.round(v)) : v.toFixed(v >= 100 ? 0 : 1), u[i]]
}

function fmtUptime(s: number): string {
  if (!s) return '—'
  const d = Math.floor(s / 86400)
  const h = Math.floor((s % 86400) / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  const hm = [h, m, sec].map(x => String(x).padStart(2, '0')).join(':')
  return d > 0 ? `${d} 天 ${hm}` : hm
}

function flagEmoji(cc: string): string {
  const code = (cc || '').toUpperCase()
  if (!/^[A-Z]{2}$/.test(code)) return ''
  return String.fromCodePoint(...[...code].map(c => 0x1f1e6 + c.charCodeAt(0) - 65))
}

const TYPE_LABEL: Record<string, string> = {
  residential: '住宅',
  mobile: '移动',
  business: '企业',
  hosting: '机房',
}
const QUALITY_LABEL: Record<string, string> = {
  good: '优',
  normal: '一般',
  risky: '风险',
}

/* ---------------- 运行信息（5 秒轮询） ---------------- */

const info = ref<DashboardInfo | null>(null)
const baseUptime = ref(0)
const baseAt = ref(Date.now())
const nowTick = ref(Date.now())

const running = computed(() => !!store.status.running)

const uptimeSecs = computed(() =>
  running.value
    ? baseUptime.value + Math.max(0, Math.floor((nowTick.value - baseAt.value) / 1000))
    : 0,
)

async function loadInfo() {
  try {
    const v: any = await api.dashboardInfo()
    // 旧版后台对未知 /api 路径会回落到 index.html，这里做个形状检查
    if (!v || typeof v !== 'object' || !('uptime_secs' in v)) return
    info.value = v
    if (running.value && v.uptime_secs != null) {
      baseUptime.value = v.uptime_secs
      baseAt.value = Date.now()
    }
  } catch {
    // 旧版后台没有这个接口：保持占位，不打扰用户
  }
}

/* ---------------- 出口 IP ---------------- */

const exit = ref<ExitIp | null>(null)
const exitState = ref<'idle' | 'loading' | 'error'>('idle')

/** 节点偶尔会慢到超过查询超时，失败后按这个间隔自动重试，全部失败才停手 */
const EXIT_RETRY_DELAYS = [8_000, 20_000]
let exitAttempts = 0
let exitTimer: number | null = null

function clearExitRetry() {
  if (exitTimer) {
    window.clearTimeout(exitTimer)
    exitTimer = null
  }
}

async function loadExit(force = false) {
  if (!running.value) {
    clearExitRetry()
    exitAttempts = 0
    exit.value = null
    exitState.value = 'idle'
    return
  }
  clearExitRetry()
  exitState.value = 'loading'
  try {
    const v: any = await api.exitIp(force)
    if (v && typeof v === 'object' && typeof v.ip === 'string') {
      exit.value = v
      exitState.value = 'idle'
      exitAttempts = 0
      return
    }
    throw new Error('empty response')
  } catch {
    exit.value = null
    exitState.value = 'error'
    scheduleExitRetry()
  }
}

function scheduleExitRetry() {
  const delay = EXIT_RETRY_DELAYS[exitAttempts]
  if (delay === undefined || !running.value) return
  exitAttempts++
  exitTimer = window.setTimeout(() => { void loadExit() }, delay)
}

const ipText = computed(() => {
  if (!running.value) return '未连接'
  if (exitState.value === 'loading') return '检测中…'
  if (exitState.value === 'error') return '获取失败'
  return exit.value?.ip || '—'
})

const ipClass = computed(() => ({
  muted: !running.value || exitState.value !== 'idle',
}))

/** 只在节点 / 模式 / 运行状态变化时重新查询；后端按（节点, 模式）缓存 5 分钟 */
watch(
  () => [store.status.running, store.profile.selected_node, store.profile.mode] as const,
  () => {
    clearExitRetry()
    exitAttempts = 0
    exitTimer = window.setTimeout(() => { void loadExit() }, 300)
  },
)

/* ---------------- 服务延迟 ---------------- */

const services = ref<ServiceStatus[]>([])
const SVC_LABEL: Record<string, string> = { google: 'Google', github: 'GitHub' }

async function loadServices() {
  if (!running.value) { services.value = []; return }
  try {
    const v: any = await api.dashboardServices()
    services.value = Array.isArray(v) ? v : []
  } catch { /* 静默 */ }
}

// 页面挂载时 store 可能还没拉到状态；启动后立即补一次
watch(running, (on) => {
  void loadInfo()
  void loadServices()
})

function latencyClass(ms: number | null): string {
  if (ms == null) return 'bad'
  if (ms < 500) return 'good'
  if (ms < 1500) return 'mid'
  return 'bad'
}

/* ---------------- 速度迷你图 + 本次会话峰值 ---------------- */

const RATE_HIST = 48
const rateUpHist = ref<number[]>(Array(RATE_HIST).fill(0))
const rateDownHist = ref<number[]>(Array(RATE_HIST).fill(0))
const peakUp = ref(0)
const peakDown = ref(0)
let rateWatchStop: (() => void) | null = null

function pushRate(up: number, down: number) {
  const u = rateUpHist.value.slice(1)
  u.push(up || 0)
  rateUpHist.value = u
  const d = rateDownHist.value.slice(1)
  d.push(down || 0)
  rateDownHist.value = d
  if (up > peakUp.value) peakUp.value = up
  if (down > peakDown.value) peakDown.value = down
}

const rateChartUp = computed(() => sparkPaths(rateUpHist.value))
const rateChartDown = computed(() => sparkPaths(rateDownHist.value))

/** 平滑曲线 + 底部填充；上传、下载各自按自己的峰值缩放 */
function sparkPaths(arr: number[]): { line: string; area: string } {
  const w = 240
  const h = 48
  const max = Math.max(1, ...arr)
  const pts = arr.map((v, i) => [
    (i / Math.max(1, arr.length - 1)) * w,
    h - (v / max) * (h - 6) - 2,
  ])
  let line = `M${pts[0][0].toFixed(1)},${pts[0][1].toFixed(1)}`
  for (let i = 1; i < pts.length; i++) {
    const [x0, y0] = pts[i - 1]
    const [x1, y1] = pts[i]
    const mx = ((x0 + x1) / 2).toFixed(1)
    line += ` C${mx},${y0.toFixed(1)} ${mx},${y1.toFixed(1)} ${x1.toFixed(1)},${y1.toFixed(1)}`
  }
  return { line, area: `${line} L${w},${h} L0,${h} Z` }
}

/** 花形按钮轮廓（播放 / 暂停键的底） */
const flowerPath = (() => {
  const cx = 50, cy = 50, r = 46, amp = 8, petals = 14
  const n = petals * 2
  const pts = Array.from({ length: n }, (_, i) => {
    const a = (i / n) * Math.PI * 2 - Math.PI / 2
    const rr = i % 2 === 0 ? r : r - amp
    return [cx + Math.cos(a) * rr, cy + Math.sin(a) * rr]
  })
  let d = `M${pts[0][0].toFixed(2)},${pts[0][1].toFixed(2)}`
  for (let i = 1; i <= n; i++) {
    const [x0, y0] = pts[(i - 1) % n]
    const [x1, y1] = pts[i % n]
    d += ` Q${((x0 + x1) / 2).toFixed(2)},${((y0 + y1) / 2).toFixed(2)} ${x1.toFixed(2)},${y1.toFixed(2)}`
  }
  return d + ' Z'
})()

/* ---------------- 当前节点 / 订阅 ---------------- */

const currentNodeLabel = computed(() => {
  const id = store.status.current_node || store.profile.selected_node
  if (!id) return '—'
  if (store.status.current_node_name) return store.status.current_node_name
  const n = store.profile.nodes.find((x: any) => x.id === id)
  return n?.name || id
})

const activeSub = computed(() => {
  const subs = (store.subscriptions || []) as any[]
  const cur = store.profile.nodes.find((n: any) => n.id === store.profile.selected_node)
  const sid = cur?.subscription_id
  if (sid != null) {
    const s = subs.find(x => x.id === sid)
    if (s) return s
  }
  return subs.find(s => s.enabled) || null
})

const subUsage = computed(() => {
  const u = activeSub.value?.userinfo
  if (!u) return null
  const used = (Number(u.upload) || 0) + (Number(u.download) || 0)
  const total = Number(u.total) || 0
  const pct = total ? Math.min(100, (used / total) * 100) : 0
  let tail = ''
  if (u.expire) {
    const days = Math.ceil((Number(u.expire) * 1000 - Date.now()) / 86400000)
    tail = days > 0 ? ` · 剩余 ${days} 天` : ' · 已到期'
  } else {
    tail = ' · 长期有效'
  }
  const text = total
    ? `已用 ${fmtBytes(used)} / ${fmtBytes(total)}`
    : `已用 ${fmtBytes(used)}`
  return { pct, text: text + tail }
})

async function refreshSub() {
  const s = activeSub.value
  if (!s) return
  try {
    await api.updateSubscription(s.id)
    toast('订阅更新中…')
  } catch (e: any) { toast(e.message) }
}

/* ---------------- 操作 ---------------- */

async function start() { try { await api.coreStart() } catch (e: any) { toast(e.message) } }
async function stop() { try { await api.coreStop() } catch (e: any) { toast(e.message) } }
async function restart() { try { await api.coreRestart() } catch (e: any) { toast(e.message) } }
function toggleRun() { running.value ? stop() : start() }

async function setMode(m: string) {
  if (store.profile.mode === m) return
  try { await api.setMode(m); toast('已切换模式') } catch (e: any) { toast(e.message) }
}

const sysproxyOn = computed(() => !!(store.config.system_proxy || store.status.system_proxy))
const tunOn = computed(() => !!(store.config.enable_tun || store.status.enable_tun))

function onToggle(key: 'system_proxy' | 'enable_tun', ev: Event) {
  const value = (ev.target as HTMLInputElement).checked
  emit('toggle', key, value)
}

function copyLan() {
  const ip = info.value?.lan_ip
  if (!ip) return
  navigator.clipboard?.writeText(ip).then(
    () => toast('已复制内网 IP'),
    () => toast('复制失败'),
  )
}

/* ---------------- 生命周期 ---------------- */

let infoTimer: number | null = null
let serviceTimer: number | null = null
let tickTimer: number | null = null

onMounted(() => {
  void loadInfo()
  void loadExit()
  void loadServices()
  infoTimer = window.setInterval(loadInfo, 5_000)
  serviceTimer = window.setInterval(() => { void loadServices() }, 60_000)
  tickTimer = window.setInterval(() => { nowTick.value = Date.now() }, 1_000)
  rateWatchStop = watch(
    () => [store.status.up_rate, store.status.down_rate] as const,
    ([u, d]) => pushRate(Number(u) || 0, Number(d) || 0),
  )
})

onBeforeUnmount(() => {
  if (infoTimer) window.clearInterval(infoTimer)
  if (serviceTimer) window.clearInterval(serviceTimer)
  if (tickTimer) window.clearInterval(tickTimer)
  clearExitRetry()
  if (rateWatchStop) { rateWatchStop(); rateWatchStop = null }
  infoTimer = serviceTimer = tickTimer = null
})
</script>

<template>
  <div class="dash-grid">
    <!-- 状态大卡 -->
    <section class="dt-card status-card">
      <div class="deco" aria-hidden="true"></div>

      <header class="st-head">
        <span class="st-badge" :class="running ? 'on' : 'off'"><i></i>{{ running ? '运行中' : '已停止' }}</span>
        <div class="st-actions">
          <button class="ghost-btn" :disabled="!running || exitState === 'loading'" title="重新检测出口 IP" @click="loadExit(true)">↻</button>
          <button class="ghost-btn" :disabled="!running" title="重启核心" @click="restart">⏻</button>
        </div>
      </header>

      <div class="st-main">
        <div class="cc-chip" :class="{ empty: !exit?.country }">
          <span class="flag">{{ exit?.country ? flagEmoji(exit.country) : '🌐' }}</span>
          <span class="cc">{{ exit?.country || '—' }}</span>
        </div>
        <div class="ip-box">
          <div class="ip" :class="ipClass" :title="exit?.ip">{{ ipText }}</div>
          <div class="tags" v-if="exit && running">
            <span v-if="exit.ip_type" class="tag type">{{ TYPE_LABEL[exit.ip_type] || exit.ip_type }}</span>
            <span v-if="exit.quality" class="tag q" :class="exit.quality">{{ QUALITY_LABEL[exit.quality] || exit.quality }}</span>
            <span v-if="exit.inferred" class="tag infer" title="查询接口没有明确给出类型，按住宅推断">推断</span>
          </div>
          <div class="org" v-if="exit?.org" :title="exit.org">{{ exit.org }}</div>
        </div>
      </div>

      <div class="st-meta">
        <div class="meta-item">
          <span class="k">运行时长</span>
          <span class="v mono">{{ fmtUptime(uptimeSecs) }}</span>
        </div>
        <div class="meta-item">
          <span class="k">当前节点</span>
          <span class="v ell" :title="currentNodeLabel">{{ currentNodeLabel }}</span>
        </div>
        <div class="meta-item">
          <span class="k">节点数</span>
          <span class="v mono">{{ store.status.node_count }}</span>
        </div>
      </div>

      <div class="power-wrap">
        <svg class="power-blob" viewBox="0 0 100 100" aria-hidden="true"><path :d="flowerPath" /></svg>
        <button class="power" :class="running ? 'on' : 'off'" :title="running ? '停止代理' : '启动代理'" @click="toggleRun">
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path v-if="running" d="M9 6.6h2.8v10.8H9zM13.2 6.6H16v10.8h-2.8z" />
            <path v-else d="M9.2 6.1v11.8c0 .5.55.8.98.53l9.1-5.9a.62.62 0 0 0 0-1.04l-9.1-5.9a.62.62 0 0 0-.98.53z" />
          </svg>
        </button>
      </div>
    </section>

    <!-- 网络速度 -->
    <section class="dt-card speed-card">
      <div class="card-title">网络速度 <span class="sub">仅统计走代理的流量</span></div>
      <div class="speed-tiles">
        <div class="speed-tile up">
          <div class="tile-label"><span class="arrow">↑</span>上传</div>
          <div class="tile-value">
            <span class="num">{{ rateParts(store.status.up_rate)[0] }}</span>
            <span class="unit">{{ rateParts(store.status.up_rate)[1] }}</span>
          </div>
          <div class="tile-meta">
            <span>累计 {{ fmtBytes(store.status.traffic_up) }}</span>
            <span>峰值 {{ rateParts(peakUp).join(' ') }}</span>
          </div>
          <svg class="tile-spark" viewBox="0 0 240 48" preserveAspectRatio="none" aria-hidden="true">
            <path :d="rateChartUp.area" class="area" />
            <path :d="rateChartUp.line" class="line" />
          </svg>
        </div>
        <div class="speed-tile down">
          <div class="tile-label"><span class="arrow">↓</span>下载</div>
          <div class="tile-value">
            <span class="num">{{ rateParts(store.status.down_rate)[0] }}</span>
            <span class="unit">{{ rateParts(store.status.down_rate)[1] }}</span>
          </div>
          <div class="tile-meta">
            <span>累计 {{ fmtBytes(store.status.traffic_down) }}</span>
            <span>峰值 {{ rateParts(peakDown).join(' ') }}</span>
          </div>
          <svg class="tile-spark" viewBox="0 0 240 48" preserveAspectRatio="none" aria-hidden="true">
            <path :d="rateChartDown.area" class="area" />
            <path :d="rateChartDown.line" class="line" />
          </svg>
        </div>
      </div>
    </section>

    <!-- 出站模式 + 系统代理 / TUN -->
    <section class="dt-card mode-card">
      <div class="card-title">出站模式</div>
      <div class="mode-tiles">
        <button v-for="[k, l] in modes" :key="k"
                class="mode-tile"
                :class="{ active: store.profile.mode === k }"
                @click="setMode(k)">
          <svg v-if="k === 'rule'" viewBox="0 0 24 24" aria-hidden="true"><rect x="4" y="4" width="16" height="16" rx="3" /><path d="M8 9h8M8 12h8M8 15h8" /></svg>
          <svg v-else-if="k === 'global'" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8" /><path d="M4 12h16M12 4c2.5 2.5 2.5 13.5 0 16M12 4c-2.5 2.5-2.5 13.5 0 16" /></svg>
          <svg v-else viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8" /><circle cx="12" cy="12" r="2.5" /></svg>
          <span>{{ l }}</span>
        </button>
      </div>
      <div class="switch-list">
        <label class="switch-row">
          <span class="sw-label">系统代理</span>
          <span class="sw-state" :class="{ on: sysproxyOn }">{{ sysproxyOn ? '开' : '关' }}</span>
          <input type="checkbox" class="tgl" :checked="sysproxyOn" @change="onToggle('system_proxy', $event)" />
        </label>
        <label class="switch-row">
          <span class="sw-label">TUN 模式</span>
          <span class="sw-state" :class="{ on: tunOn }">{{ tunOn ? '开' : '关' }}</span>
          <input type="checkbox" class="tgl" :checked="tunOn" @change="onToggle('enable_tun', $event)" />
        </label>
      </div>
    </section>

    <!-- 内网 IP -->
    <section class="dt-card mini">
      <div class="card-title">内网 IP
        <button v-if="info?.lan_ip" class="mini-action" title="复制" @click="copyLan">复制</button>
      </div>
      <div class="mini-value mono">{{ info?.lan_ip || '—' }}</div>
      <div class="mini-sub">代理端口 {{ info?.proxy_port ?? '—' }}</div>
    </section>

    <!-- 订阅 -->
    <section class="dt-card mini clickable" @click="emit('navigate', 'subscriptions')">
      <div class="card-title">订阅 <span class="chev">›</span>
        <button v-if="activeSub" class="mini-action" title="更新订阅" @click.stop="refreshSub">↻</button>
      </div>
      <div class="sub-name ell" :title="activeSub?.name">{{ activeSub?.name || '未添加订阅' }}</div>
      <template v-if="subUsage">
        <div class="usage-track"><div class="usage-bar" :style="{ width: subUsage.pct + '%' }"></div></div>
        <div class="mini-sub ell" :title="subUsage.text">{{ subUsage.text }}</div>
      </template>
      <div v-else class="mini-sub">{{ activeSub ? '暂无流量信息' : '前往订阅页添加' }}</div>
    </section>

    <!-- 服务状态 -->
    <section class="dt-card mini">
      <div class="card-title">服务状态</div>
      <div class="svc-row" v-for="s in services" :key="s.name">
        <span class="dot" :class="latencyClass(s.ms)"></span>
        <span class="svc-name">{{ SVC_LABEL[s.name] || s.name }}</span>
        <span class="svc-ms" :class="latencyClass(s.ms)">{{ s.ms != null ? s.ms + ' ms' : '超时' }}</span>
      </div>
      <div v-if="!services.length" class="mini-sub">{{ running ? '检测中…' : '代理未运行' }}</div>
    </section>

    <!-- 内存 -->
    <section class="dt-card mini">
      <div class="card-title">内存</div>
      <div class="mem-row"><span>本程序</span><b class="mono">{{ fmtBytes(info?.app_mem || 0) }}</b></div>
      <div class="mem-row"><span>sing-box</span><b class="mono">{{ running ? fmtBytes(info?.core_mem || 0) : '—' }}</b></div>
    </section>
  </div>
</template>

<style scoped>
.dash-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 12px;
  margin-bottom: 18px;
}

.dt-card {
  position: relative;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 16px;
  padding: 16px 18px;
  min-width: 0;
  overflow: hidden;
}

.card-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  font-weight: 600;
  color: #374151;
  margin-bottom: 12px;
}
.card-title .sub { margin-left: auto; color: var(--muted); font-size: 11px; font-weight: 400; }
.card-title .chev { margin-left: auto; color: var(--muted); font-size: 16px; line-height: 1; }
.mono { font-variant-numeric: tabular-nums; }
.ell { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }

/* ---------- 状态卡 ---------- */
.status-card {
  grid-column: span 2;
  grid-row: span 2;
  display: flex;
  flex-direction: column;
  min-height: 232px;
  padding-bottom: 18px;
}
.deco {
  position: absolute;
  width: 260px; height: 260px;
  right: -70px; top: -90px;
  border-radius: 50%;
  background: radial-gradient(circle, rgba(37, 99, 235, .12), rgba(37, 99, 235, 0) 68%);
  pointer-events: none;
}
.st-head { display: flex; align-items: center; justify-content: space-between; position: relative; }
.st-badge {
  display: inline-flex; align-items: center; gap: 6px;
  padding: 3px 10px; border-radius: 999px;
  font-size: 12px; font-weight: 600;
}
.st-badge i { width: 7px; height: 7px; border-radius: 50%; background: currentColor; }
.st-badge.on  { background: #dcfce7; color: #166534; }
.st-badge.off { background: #f1f5f9; color: #64748b; }
.st-actions { display: flex; gap: 6px; }
.ghost-btn {
  width: 28px; height: 28px; padding: 0;
  border: 1px solid var(--border); border-radius: 50%;
  background: #fff; color: #6b7280; font-size: 13px; line-height: 1;
  cursor: pointer;
  display: inline-flex; align-items: center; justify-content: center;
}
.ghost-btn:hover:not(:disabled) { color: var(--primary); border-color: #93c5fd; }
.ghost-btn:disabled { opacity: .45; cursor: not-allowed; }

.st-main { display: flex; align-items: center; gap: 16px; margin: 18px 0 16px; position: relative; flex: 1; }
.cc-chip {
  flex-shrink: 0;
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  width: 66px; height: 66px;
  border-radius: 18px;
  background: #eff6ff;
  border: 1px solid #dbeafe;
}
.cc-chip.empty { background: #f8fafc; border-color: var(--border); }
.cc-chip .flag { font-size: 28px; line-height: 1; }
.cc-chip .cc { margin-top: 4px; font-size: 11px; font-weight: 700; letter-spacing: .5px; color: #1d4ed8; }
.cc-chip.empty .cc { color: var(--muted); }

.ip-box { min-width: 0; flex: 1; }
.ip {
  font-size: 28px; font-weight: 600; line-height: 1.15;
  font-variant-numeric: tabular-nums;
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  color: #111827;
}
.ip.muted { color: var(--muted); font-size: 22px; font-weight: 500; }
.tags { display: flex; gap: 6px; margin-top: 8px; flex-wrap: wrap; }
.tag {
  padding: 1px 8px; border-radius: 999px;
  font-size: 11px; font-weight: 600;
  background: #f1f5f9; color: #475569;
}
.tag.q.good   { background: #dcfce7; color: #166534; }
.tag.q.normal { background: #fef3c7; color: #92400e; }
.tag.q.risky  { background: #fee2e2; color: #991b1b; }
.tag.infer    { background: #f8fafc; color: #94a3b8; font-weight: 500; }
.org {
  margin-top: 6px; font-size: 12px; color: var(--muted);
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}

.st-meta {
  display: flex; gap: 26px; flex-wrap: wrap;
  padding-right: 84px;
  border-top: 1px solid #f1f5f9;
  padding-top: 12px;
  position: relative;
}
.meta-item { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.meta-item .k { font-size: 11px; color: var(--muted); }
.meta-item .v { font-size: 13px; font-weight: 600; color: #374151; max-width: 190px; }

.power-wrap { position: absolute; right: 16px; bottom: 14px; width: 68px; height: 68px; }
.power-blob { position: absolute; inset: 0; width: 100%; height: 100%; fill: var(--primary); opacity: .16; }
.power {
  position: absolute; inset: 9px;
  border: 0; border-radius: 50%;
  background: var(--primary);
  color: #fff; cursor: pointer;
  display: flex; align-items: center; justify-content: center;
  box-shadow: 0 8px 18px rgba(37, 99, 235, .3);
  transition: background .15s, transform .1s;
}
.power:hover { background: var(--primary-d); }
.power:active { transform: scale(.96); }
.power.off { background: #2563eb; }
.power svg { width: 26px; height: 26px; fill: currentColor; }

/* ---------- 网络速度 ---------- */
.speed-card { grid-column: span 2; display: flex; flex-direction: column; }
.speed-tiles { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; flex: 1; }
.speed-tile {
  position: relative; overflow: hidden;
  background: #f3f4f6; border-radius: 14px;
  padding: 12px 14px 46px; min-width: 0;
}
.tile-label { display: flex; align-items: center; gap: 6px; color: #4b5563; font-size: 13px; }
.tile-label .arrow { font-weight: 700; }
.speed-tile.down .arrow { color: var(--primary); }
.tile-value { display: flex; align-items: baseline; gap: 6px; margin-top: 6px; }
.tile-value .num { font-size: 30px; font-weight: 600; line-height: 1; font-variant-numeric: tabular-nums; }
.tile-value .unit { font-size: 14px; color: #4b5563; }
.tile-meta {
  display: flex; gap: 10px; flex-wrap: wrap;
  margin-top: 6px; color: #6b7280; font-size: 11px;
  font-variant-numeric: tabular-nums;
}
.tile-spark { position: absolute; left: 0; right: 0; bottom: 0; width: 100%; height: 42px; display: block; }
.tile-spark .line { fill: none; stroke-width: 2; }
.speed-tile.up .line { stroke: #93c5fd; }
.speed-tile.up .area { fill: rgba(147, 197, 253, .25); }
.speed-tile.down .line { stroke: var(--primary); }
.speed-tile.down .area { fill: rgba(37, 99, 235, .15); }

/* ---------- 出站模式 ---------- */
.mode-card { grid-column: span 2; display: flex; flex-direction: column; }
.mode-tiles { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; }
.mode-tile {
  display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 5px;
  min-height: 62px; border: none; border-radius: 12px; background: #f3f4f6;
  color: #374151; font-size: 13px; cursor: pointer;
  transition: background .15s, color .15s;
}
.mode-tile svg {
  width: 20px; height: 20px; fill: none; stroke: currentColor;
  stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round;
}
.mode-tile:hover { background: #e8edf7; color: var(--primary); }
.mode-tile.active { background: var(--primary); color: #fff; }

.switch-list { margin-top: 12px; padding-top: 10px; border-top: 1px solid #f1f5f9; }
.switch-row {
  display: flex; align-items: center; gap: 8px;
  padding: 4px 0; font-size: 13px; color: #374151; cursor: pointer;
}
.sw-label { flex: 1; }
.sw-state {
  font-size: 11px; padding: 1px 8px; border-radius: 999px;
  background: #f1f5f9; color: #64748b;
}
.sw-state.on { background: #dcfce7; color: #166534; }
.tgl { width: 15px; height: 15px; accent-color: var(--primary); cursor: pointer; }

/* ---------- 底部小卡 ---------- */
.mini { min-height: 120px; display: flex; flex-direction: column; }
.mini.clickable { cursor: pointer; transition: border-color .15s; }
.mini.clickable:hover { border-color: #93c5fd; }
.mini-value { font-size: 20px; font-weight: 600; color: #111827; }
.mini-sub { margin-top: 6px; font-size: 11px; color: var(--muted); }
.mini-action {
  margin-left: auto; padding: 1px 8px;
  border: 1px solid var(--border); border-radius: 999px;
  background: #fff; color: #6b7280; font-size: 11px; line-height: 1.5;
  cursor: pointer;
}
.mini-action:hover { color: var(--primary); border-color: #93c5fd; }

.sub-name { font-size: 14px; font-weight: 600; color: #111827; }
.usage-track {
  height: 6px; margin-top: 10px; border-radius: 999px;
  background: #eef2f7; overflow: hidden;
}
.usage-bar {
  height: 100%; border-radius: 999px;
  background: linear-gradient(90deg, #60a5fa, var(--primary));
}

.svc-row { display: flex; align-items: center; gap: 8px; padding: 4px 0; font-size: 12px; }
.svc-row .dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; }
.dot.good { background: #22c55e; }
.dot.mid  { background: #f59e0b; }
.dot.bad  { background: #ef4444; }
.svc-name { flex: 1; color: #374151; }
.svc-ms { font-variant-numeric: tabular-nums; }
.svc-ms.good { color: #16a34a; }
.svc-ms.mid  { color: #d97706; }
.svc-ms.bad  { color: #dc2626; }

.mem-row {
  display: flex; align-items: baseline; justify-content: space-between;
  padding: 4px 0; font-size: 12px; color: #4b5563;
}
.mem-row b { font-size: 14px; color: #111827; }

@media (max-width: 1100px) {
  .dash-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .status-card { grid-column: span 2; grid-row: auto; }
  .speed-card, .mode-card { grid-column: span 2; }
  .st-meta { padding-right: 84px; }
}
@media (max-width: 640px) {
  .dash-grid { grid-template-columns: 1fr; }
  .status-card, .speed-card, .mode-card { grid-column: span 1; }
  .st-meta { padding-right: 0; padding-bottom: 78px; }
  .power-wrap { right: 16px; bottom: 14px; }
  .ip { font-size: 24px; }
}
</style>
