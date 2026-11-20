<script setup lang="ts">
// 仪表盘上半部分：网络信息 / 流量统计 / 订阅 / 服务。
// 下半部分的节点列表留在 Dashboard.vue。
import { computed, ref, watch, onMounted, onBeforeUnmount } from 'vue'
import { store, toast } from '../store'
import { api, type DashboardInfo, type ExitIp, type ServiceStatus } from '../api'
import { appendTrafficSample, trafficChart, TRAFFIC_WINDOW_MS, type TrafficSample } from '../trafficChart'

const emit = defineEmits<{
  (e: 'toggle', key: 'system_proxy' | 'enable_tun', value: boolean): void
  (e: 'navigate', tab: string): void
}>()

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

/* ---------------- 最近十分钟流量 ---------------- */

const rateHistory = ref<TrafficSample[]>([])
const rateChart = computed(() => trafficChart(rateHistory.value, nowTick.value))
const chartTimes = computed(() => [0, 1, 2, 3, 4].map(i =>
  new Date(nowTick.value - TRAFFIC_WINDOW_MS + i * TRAFFIC_WINDOW_MS / 4)
    .toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false }),
))

function sampleTraffic() {
  nowTick.value = Date.now()
  if (!store.connected) return
  rateHistory.value = appendTrafficSample(rateHistory.value, {
    at: nowTick.value,
    up: running.value ? store.status.up_rate : 0,
    down: running.value ? store.status.down_rate : 0,
  })
}

const trafficStats = computed(() => [
  { label: '上传速度', tone: 'up', icon: 'M12 19V5m-6 6 6-6 6 6', value: rateParts(running.value ? store.status.up_rate : 0).join(' ') },
  { label: '下载速度', tone: 'down', icon: 'M12 5v14m-6-6 6 6 6-6', value: rateParts(running.value ? store.status.down_rate : 0).join(' ') },
  { label: '活跃连接', tone: 'connections', icon: 'M10 7H8a5 5 0 0 0 0 10h2m4-10h2a5 5 0 0 1 0 10h-2M8 12h8', value: String(running.value ? store.status.active_connections : 0) },
  { label: '上传量', tone: 'up', icon: 'M5 16v4h14v-4M12 15V4m-4 4 4-4 4 4', value: fmtBytes(store.status.traffic_up) },
  { label: '下载量', tone: 'down', icon: 'M5 16v4h14v-4M12 4v11m-4-4 4 4 4-4', value: fmtBytes(store.status.traffic_down) },
  { label: '内核占用', tone: 'memory', icon: 'M7 7h10v10H7zM10 10h4v4h-4zM9 3v4m6-4v4M9 17v4m6-4v4M3 9h4m-4 6h4m10-6h4m-4 6h4', value: running.value ? fmtBytes(info.value?.core_mem || 0) : '—' },
])

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
  sampleTraffic()
  tickTimer = window.setInterval(sampleTraffic, 1_000)
})

onBeforeUnmount(() => {
  if (infoTimer) window.clearInterval(infoTimer)
  if (serviceTimer) window.clearInterval(serviceTimer)
  if (tickTimer) window.clearInterval(tickTimer)
  clearExitRetry()
  infoTimer = serviceTimer = tickTimer = null
})
</script>

<template>
  <div class="dash-grid">
    <!-- 外网 / 内网 IP 与运行状态 -->
    <section class="dt-card status-card">
      <div class="deco" aria-hidden="true"></div>

      <header class="st-head">
        <div class="st-heading">
          <span class="card-title">网络信息</span>
          <span class="st-badge" :class="running ? 'on' : 'off'"><i></i>{{ running ? '运行中' : '已停止' }}</span>
        </div>
        <div class="st-actions">
          <button class="ghost-btn" :disabled="!running || exitState === 'loading'" title="重新检测出口 IP" @click="loadExit(true)">↻</button>
          <button class="ghost-btn" :disabled="!running" title="重启核心" @click="restart">⏻</button>
          <button class="power" :title="running ? '停止代理' : '启动代理'" :aria-label="running ? '停止代理' : '启动代理'" @click="toggleRun">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path v-if="running" d="M9 6.6h2.8v10.8H9zM13.2 6.6H16v10.8h-2.8z" />
              <path v-else d="M9.2 6.1v11.8c0 .5.55.8.98.53l9.1-5.9a.62.62 0 0 0 0-1.04l-9.1-5.9a.62.62 0 0 0-.98.53z" />
            </svg>
          </button>
        </div>
      </header>

      <div class="st-main">
        <div class="cc-chip" :class="{ empty: !exit?.country }">
          <span class="flag">{{ exit?.country ? flagEmoji(exit.country) : '🌐' }}</span>
          <span class="cc">{{ exit?.country || '—' }}</span>
        </div>
        <div class="ip-box">
          <div class="ip-label">外网 IP</div>
          <div class="ip" :class="ipClass" :title="exit?.ip">{{ ipText }}</div>
          <div class="tags" v-if="exit && running">
            <span v-if="exit.ip_type" class="tag type">{{ TYPE_LABEL[exit.ip_type] || exit.ip_type }}</span>
            <span v-if="exit.quality" class="tag q" :class="exit.quality">{{ QUALITY_LABEL[exit.quality] || exit.quality }}</span>
            <span v-if="exit.inferred" class="tag infer" title="查询接口没有明确给出类型，按住宅推断">推断</span>
          </div>
          <div class="org" v-if="exit?.org" :title="exit.org">{{ exit.org }}</div>
        </div>
      </div>

      <div class="lan-row">
        <span class="lan-label">内网 IP</span>
        <span class="lan-ip mono">{{ info?.lan_ip || '—' }}</span>
        <button v-if="info?.lan_ip" class="mini-action" title="复制内网 IP" @click="copyLan">复制</button>
        <span class="lan-port">代理端口 <b class="mono">{{ info?.proxy_port ?? '—' }}</b></span>
      </div>

      <div class="switch-list">
        <label class="switch-row">
          <span class="sw-label">系统代理</span>
          <input type="checkbox" role="switch" class="tgl" :checked="sysproxyOn" @change="onToggle('system_proxy', $event)" />
        </label>
        <label class="switch-row">
          <span class="sw-label">TUN 模式</span>
          <input type="checkbox" role="switch" class="tgl" :checked="tunOn" @change="onToggle('enable_tun', $event)" />
        </label>
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
    </section>

    <!-- 流量统计 -->
    <section class="dt-card speed-card">
      <div class="card-title">流量统计 <span class="sub">速率与流量仅统计代理连接</span></div>
      <div class="traffic-chart">
        <div class="chart-head">
          <span class="chart-range">最近 10 分钟</span>
          <span class="chart-legend up">上传</span>
          <span class="chart-legend down">下载</span>
        </div>
        <div class="chart-body">
          <div class="chart-scale mono">
            <span>{{ rateParts(rateChart.max).join(' ') }}</span>
            <span>{{ rateParts(rateChart.max / 2).join(' ') }}</span>
            <span>0</span>
          </div>
          <svg class="traffic-plot" viewBox="0 0 600 100" preserveAspectRatio="none" role="img" aria-label="最近十分钟上传和下载速率曲线">
            <path class="chart-grid" d="M0 0H600M0 25H600M0 50H600M0 75H600M0 100H600M0 0V100M150 0V100M300 0V100M450 0V100M600 0V100" />
            <path :d="rateChart.down" class="chart-line down" />
            <path :d="rateChart.up" class="chart-line up" />
          </svg>
        </div>
        <div class="chart-times mono"><span v-for="(time, i) in chartTimes" :key="i">{{ time }}</span></div>
      </div>
      <div class="traffic-stats">
        <div v-for="stat in trafficStats" :key="stat.label" class="traffic-stat" :class="stat.tone">
          <span class="stat-icon"><svg viewBox="0 0 24 24" aria-hidden="true"><path :d="stat.icon" /></svg></span>
          <div class="stat-text">
            <div class="stat-label">{{ stat.label }}</div>
            <div class="stat-value mono">{{ stat.value.split(' ')[0] }} <small>{{ stat.value.split(' ')[1] }}</small></div>
          </div>
        </div>
      </div>
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

  </div>
</template>

<style scoped>
.dash-grid {
  display: grid;
  grid-template-columns: repeat(12, minmax(0, 1fr));
  gap: 10px;
  margin-bottom: 14px;
}

.dt-card {
  position: relative;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 14px;
  padding: 14px 16px;
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
  margin-bottom: 10px;
}
.card-title .sub { margin-left: auto; color: var(--muted); font-size: 11px; font-weight: 400; }
.card-title .chev { margin-left: auto; color: var(--muted); font-size: 16px; line-height: 1; }
.mono { font-variant-numeric: tabular-nums; }
.ell { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }

/* ---------- 状态卡 ---------- */
.status-card {
  grid-column: span 5;
  display: flex;
  flex-direction: column;
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
.st-heading { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.st-heading .card-title { margin: 0; }
.st-badge {
  display: inline-flex; align-items: center; gap: 6px;
  padding: 3px 10px; border-radius: 999px;
  font-size: 12px; font-weight: 600;
}
.st-badge i { width: 7px; height: 7px; border-radius: 50%; background: currentColor; }
.st-badge.on  { background: #dcfce7; color: #166534; }
.st-badge.off { background: #f1f5f9; color: #64748b; }
.st-actions { display: flex; gap: 6px; flex-shrink: 0; }
.ghost-btn {
  width: 30px; height: 30px; padding: 0;
  border: 1px solid var(--border); border-radius: 50%;
  background: #fff; color: #6b7280; font-size: 13px; line-height: 1;
  cursor: pointer;
  display: inline-flex; align-items: center; justify-content: center;
}
.ghost-btn:hover:not(:disabled) { color: var(--primary); border-color: #93c5fd; }
.ghost-btn:disabled { opacity: .45; cursor: not-allowed; }

.st-main { display: flex; align-items: center; gap: 12px; margin: 12px 0; position: relative; }
.cc-chip {
  flex-shrink: 0;
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  width: 52px; height: 58px;
  border-radius: 14px;
  background: #eff6ff;
  border: 1px solid #dbeafe;
}
.cc-chip.empty { background: #f8fafc; border-color: var(--border); }
.cc-chip .flag { font-size: 23px; line-height: 1; }
.cc-chip .cc { margin-top: 4px; font-size: 11px; font-weight: 700; letter-spacing: .5px; color: #1d4ed8; }
.cc-chip.empty .cc { color: var(--muted); }

.ip-box { min-width: 0; flex: 1; }
.ip-label { font-size: 11px; color: var(--muted); margin-bottom: 3px; }
.ip {
  font-size: 26px; font-weight: 600; line-height: 1.15;
  font-variant-numeric: tabular-nums;
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  color: #111827;
}
.ip.muted { color: var(--muted); font-size: 22px; font-weight: 500; }
.tags { display: flex; gap: 6px; margin-top: 5px; flex-wrap: wrap; }
.tag {
  padding: 1px 8px; border-radius: 999px; margin-left: 0;
  font-size: 11px; font-weight: 600;
  background: #f1f5f9; color: #475569;
}
.tag.q.good   { background: #dcfce7; color: #166534; }
.tag.q.normal { background: #fef3c7; color: #92400e; }
.tag.q.risky  { background: #fee2e2; color: #991b1b; }
.tag.infer    { background: #f8fafc; color: #94a3b8; font-weight: 500; }
.org {
  margin-top: 4px; font-size: 12px; color: var(--muted);
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}

.lan-row {
  display: flex; align-items: center; flex-wrap: wrap; gap: 6px 8px;
  padding: 8px 10px; margin-bottom: 10px;
  border-radius: 8px; background: #f8fafc; font-size: 12px;
}
.lan-label, .lan-port { color: var(--muted); font-size: 11px; }
.lan-ip { color: #374151; font-weight: 600; }
.lan-row .mini-action { margin-left: 0; }
.lan-port { margin-left: auto; white-space: nowrap; }
.lan-port b { color: #4b5563; font-weight: 500; }

.st-meta {
  display: grid; grid-template-columns: auto minmax(0, 1fr) auto; gap: 16px;
  border-top: 1px solid #f1f5f9;
  padding-top: 10px; margin-top: auto;
  position: relative;
}
.meta-item { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.meta-item .k { font-size: 11px; color: var(--muted); }
.meta-item .v { font-size: 12px; font-weight: 600; color: #374151; }

.power {
  width: 30px; height: 30px; padding: 0;
  border: 0; border-radius: 50%;
  background: var(--primary);
  color: #fff; cursor: pointer;
  display: flex; align-items: center; justify-content: center;
  transition: background .15s, transform .1s;
}
.power:hover { background: var(--primary-d); }
.power:active { transform: scale(.96); }
.power svg { width: 20px; height: 20px; fill: currentColor; }

/* ---------- 流量统计 ---------- */
.speed-card { grid-column: span 7; }
.traffic-chart { padding: 8px 10px; border: 1px solid var(--border); border-radius: 10px; background: #f8fafc; }
.chart-head { display: flex; align-items: center; gap: 12px; margin-bottom: 8px; font-size: 11px; }
.chart-range { margin-right: auto; padding: 2px 7px; border-radius: 4px; background: #eef1f5; color: #64748b; }
.up { --stat-color: #e78961; --stat-bg: #fff8f3; --icon-bg: #ffede3; }
.down { --stat-color: #2583eb; --stat-bg: #f2f8ff; --icon-bg: #e0eeff; }
.connections { --stat-color: #159951; --stat-bg: #f2faf5; --icon-bg: #ddf3e6; }
.memory { --stat-color: #e0525c; --stat-bg: #fff5f5; --icon-bg: #ffe5e7; }
.chart-legend { display: inline-flex; align-items: center; gap: 4px; color: var(--stat-color); }
.chart-legend::before { content: ''; width: 12px; height: 2px; background: currentColor; }
.chart-body { display: flex; gap: 8px; height: 88px; }
.chart-scale { display: flex; flex-direction: column; justify-content: space-between; flex-shrink: 0; width: 58px; text-align: right; font-size: 9px; color: var(--muted); }
.traffic-plot { display: block; width: 100%; min-width: 0; height: 100%; overflow: visible; }
.chart-grid { fill: none; stroke: var(--border); stroke-width: 1; vector-effect: non-scaling-stroke; }
.chart-line { fill: none; stroke: var(--stat-color); stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; vector-effect: non-scaling-stroke; }
.chart-times { display: flex; justify-content: space-between; margin: 5px 0 0 66px; color: var(--muted); font-size: 9px; }
.traffic-stats { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; margin-top: 8px; }
.traffic-stat { display: flex; align-items: center; gap: 8px; min-width: 0; padding: 9px 10px; border: 1px solid var(--border); border-radius: 9px; background: var(--stat-bg); }
.stat-icon { display: flex; align-items: center; justify-content: center; flex-shrink: 0; width: 30px; height: 30px; border-radius: 50%; color: var(--stat-color); background: var(--icon-bg); }
.stat-icon svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
.stat-text { min-width: 0; }
.stat-label { color: #4b5563; font-size: 11px; }
.stat-value { margin-top: 2px; font-size: 16px; font-weight: 600; white-space: nowrap; }
.stat-value small { font-size: 11px; font-weight: 400; }

.switch-list { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; margin: 0 0 12px; }
.switch-row {
  display: flex; align-items: center; gap: 8px;
  padding: 9px 12px; border-radius: 999px; border: 1px solid var(--border);
  background: #fafafa; font-size: 12px; font-weight: 600; color: #374151; cursor: pointer;
}
.sw-label { flex: 1; }
.tgl { appearance: none; position: relative; flex-shrink: 0; width: 38px; height: 22px; margin: 0; border: 0; border-radius: 999px; background: #e5e7eb; cursor: pointer; transition: background .15s; }
.tgl::before { content: ''; position: absolute; width: 16px; height: 16px; top: 3px; left: 3px; border-radius: 50%; background: #7b818c; transition: transform .15s, background .15s; }
.tgl:checked { background: #0783fa; }
.tgl:checked::before { background: #fff; transform: translateX(16px); }
.tgl:focus-visible { outline: 2px solid var(--primary); outline-offset: 3px; }

/* ---------- 底部小卡 ---------- */
.mini { grid-column: span 6; display: flex; flex-direction: column; }
.mini.clickable { cursor: pointer; transition: border-color .15s; }
.mini.clickable:hover { border-color: #93c5fd; }
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

@media (max-width: 1100px) {
  .dash-grid { grid-template-columns: repeat(6, minmax(0, 1fr)); }
  .status-card, .speed-card { grid-column: span 6; }
  .mini { grid-column: span 3; }
}
@media (max-width: 760px) {
  .dash-grid { grid-template-columns: 1fr; }
  .status-card, .speed-card, .mini { grid-column: span 1; }
  .dt-card { padding: 12px; }
  .st-meta { gap: 10px; }
  .ip { font-size: 22px; }
  .traffic-stat { padding: 8px; gap: 6px; }
  .stat-icon { width: 24px; height: 24px; }
  .stat-value { font-size: 14px; }
}
@media (max-width: 680px) {
  .traffic-stats { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
</style>
