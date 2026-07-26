<script setup lang="ts">
import { ref, onMounted, computed, onBeforeUnmount } from 'vue'
import { store, toast } from './store'
import { api } from './api'
import { connectWs } from './ws'
import Dashboard from './views/Dashboard.vue'
import Subscriptions from './views/Subscriptions.vue'
import Rules from './views/Rules.vue'
import Settings from './views/Settings.vue'
import Logs from './views/Logs.vue'

const tabs = [
  { key: 'dashboard', label: '仪表盘', icon: '▦' },
  { key: 'subscriptions', label: '订阅', icon: '↻' },
  { key: 'rules', label: '规则', icon: '⚑' },
  { key: 'settings', label: '设置', icon: '⚙' },
  { key: 'logs', label: '日志', icon: '☰' },
]
const views: Record<string, any> = { dashboard: Dashboard, subscriptions: Subscriptions, rules: Rules, settings: Settings, logs: Logs }
const active = ref('dashboard')

// 右上角绿色指示：系统代理 / TUN 启用时显示
const indicators = computed(() => {
  const items: { key: string; label: string }[] = []
  if (store.config.system_proxy || store.status.system_proxy) {
    items.push({ key: 'sysproxy', label: '系统代理' })
  }
  if (store.config.enable_tun || store.status.enable_tun) {
    items.push({ key: 'tun', label: 'TUN' })
  }
  return items
})

async function refreshAll() {
  try {
    const [s, p, c, subs, rs] = await Promise.all([
      api.status(), api.profile(), api.config(), api.subscriptions(), api.rules(),
    ])
    Object.assign(store.status, s)
    Object.assign(store.profile, p)
    Object.assign(store.config, c)
    // 与 status 对齐，便于右上角指示灯同时读 config / status
    store.status.system_proxy = !!(s as any).system_proxy || !!(c as any).system_proxy
    store.status.enable_tun = !!(s as any).enable_tun || !!(c as any).enable_tun
    store.subscriptions = subs
    store.rules = rs
    // 数据就绪后再决定是否提示首启添加订阅（避免 Dashboard 在 refreshAll
    // 还没完成时拿到空数组误弹）。7 天内手动关闭过的不再提示。
    const empty = !subs || subs.length === 0
    if (empty && !store.promptFirstRun && !recentlyDismissedFirstRunDialog()) {
      store.promptFirstRun = true
    }
  } catch (e: any) {
    toast('加载失败: ' + e.message)
  }
}

const FIRST_RUN_LS_KEY = 'yap-xfish.add-dialog.dismissed.v1'
function recentlyDismissedFirstRunDialog(): boolean {
  try {
    const raw = localStorage.getItem(FIRST_RUN_LS_KEY)
    if (!raw) return false
    const ts = Number(raw) || 0
    const SEVEN_DAYS = 7 * 24 * 60 * 60 * 1000
    return ts > 0 && Date.now() - ts < SEVEN_DAYS
  } catch { return false }
}

onMounted(async () => {
  connectWs()
  await refreshAll()
  // 加载版本号
  try {
    const v: any = await api.version()
    store.appVersion = v?.version || ''
  } catch { /* 静默 */ }
  await loadDaily()
  // 30s 刷新一次侧栏统计（轻量；无新增后端任务）
  dailyTimer = window.setInterval(() => { void loadDaily() }, 30_000)
})

onBeforeUnmount(() => {
  if (dailyTimer) { window.clearInterval(dailyTimer); dailyTimer = null }
})

/* ---------- 侧栏流量统计（30 天每日）---------- */
interface Day { date: string; up: number; down: number }
const dailyDays = ref<Day[]>([])
const dailyToday = ref({ date: '', up: 0, down: 0 })
let dailyTimer: number | null = null
async function loadDaily() {
  try {
    const d: any = await api.trafficDaily()
    dailyDays.value = Array.isArray(d?.days) ? d.days : []
    dailyToday.value = {
      date: d?.today || '',
      up: Number(d?.today_up) || 0,
      down: Number(d?.today_down) || 0,
    }
  } catch (e) { /* 静默：侧栏小卡，错误不进 toast */ }
}
function fmtB(n: number): string {
  if (!n || n < 0) return '0'
  if (n >= 1024 * 1024 * 1024) return (n / 1024 / 1024 / 1024).toFixed(2) + ' GB'
  if (n >= 1024 * 1024) return (n / 1024 / 1024).toFixed(2) + ' MB'
  if (n >= 1024) return (n / 1024).toFixed(1) + ' KB'
  return n + ' B'
}
/** 列表展示：新在上；日期只显示 MM-DD 以省宽度 */
const dailyList = computed(() => {
  const list = [...dailyDays.value].sort((a, b) => (b.date || '').localeCompare(a.date || ''))
  return list
})
function shortDate(d: string): string {
  // 期望 YYYY-MM-DD → MM-DD；其它原样
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(d || '')
  return m ? `${m[2]}-${m[3]}` : (d || '—')
}
const daily30Total = computed(() =>
  dailyDays.value.reduce((s, d) => s + (d.up || 0) + (d.down || 0), 0),
)
</script>

<template>
  <div class="layout">
    <aside class="side">
      <div class="brand">YAP<span>-XFISH</span><span class="version" v-if="store.appVersion">v{{ store.appVersion }}</span></div>
      <nav>
        <button
          v-for="t in tabs"
          :key="t.key"
          :class="{ active: active === t.key }"
          @click="active = t.key"
        >
          <span class="ic">{{ t.icon }}</span>{{ t.label }}
        </button>
      </nav>
      <div class="conn" :class="store.connected ? 'on' : 'off'">
        {{ store.connected ? '● 已连接' : '○ 连接中…' }}
      </div>
      <div class="side-stats">
        <div class="side-stats-title">
          <span>流量统计</span>
          <span class="side-stats-tot">30天 {{ fmtB(daily30Total) }}</span>
        </div>
        <div class="side-stats-today">
          今日 ↑ {{ fmtB(dailyToday.up) }} · ↓ {{ fmtB(dailyToday.down) }}
        </div>
        <div class="side-stats-list" v-if="dailyList.length">
          <div
            v-for="(d, i) in dailyList"
            :key="d.date + i"
            class="side-stats-row"
            :title="`${d.date}  ↑${fmtB(d.up)}  ↓${fmtB(d.down)}`"
          >
            <span class="side-stats-date">{{ shortDate(d.date) }}</span>
            <span class="side-stats-up">↑{{ fmtB(d.up) }}</span>
            <span class="side-stats-down">↓{{ fmtB(d.down) }}</span>
          </div>
        </div>
        <div class="side-stats-empty" v-else>暂无数据</div>
      </div>
    </aside>
    <main class="main">
      <div class="topbar" v-if="indicators.length">
        <span
          v-for="it in indicators"
          :key="it.key"
          class="ind"
          :title="it.label + ' 已启用'"
        >
          <i class="dot"></i>{{ it.label }}
        </span>
      </div>
      <component :is="views[active]" @refresh="refreshAll" @navigate="(tab) => active = tab" />
    </main>
    <div v-if="store.toast" class="toast">{{ store.toast }}</div>
  </div>
</template>

<style scoped>
.topbar {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin: -4px 0 12px;
}
.ind {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 999px;
  background: #ecfdf5;
  color: #166534;
  border: 1px solid #86efac;
  font-size: 12px;
  font-weight: 600;
}
.ind .dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #22c55e;
  box-shadow: 0 0 0 3px rgba(34, 197, 94, 0.2);
  display: inline-block;
}

/* 侧栏流量统计：列表展示每日上下行 */
.side-stats {
  margin-top: auto;
  padding: 10px 10px 12px;
  border-top: 1px solid var(--border);
  background: transparent;
  color: var(--muted);
  font-size: 11px;
}
.side-stats-title {
  display: flex; justify-content: space-between; align-items: center;
  color: var(--text); font-weight: 700; font-size: 12px; margin-bottom: 4px;
}
.side-stats-tot { color: var(--muted); font-weight: 500; font-size: 10px; }
.side-stats-today { color: var(--muted); font-size: 11px; margin-bottom: 8px; }
.side-stats-list {
  max-height: 160px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 2px;
  background: transparent;
}
.side-stats-row {
  display: grid;
  grid-template-columns: 42px 1fr 1fr;
  gap: 4px;
  align-items: center;
  padding: 3px 4px;
  border-radius: 4px;
  line-height: 1.3;
}
.side-stats-row:hover { background: rgba(0, 0, 0, 0.04); }
.side-stats-date {
  color: var(--text);
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}
.side-stats-up { color: #16a34a; text-align: right; font-variant-numeric: tabular-nums; }
.side-stats-down { color: #2563eb; text-align: right; font-variant-numeric: tabular-nums; }
.side-stats-empty { color: var(--muted); font-size: 11px; }
</style>
