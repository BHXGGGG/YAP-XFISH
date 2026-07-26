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
const dailyMax = computed(() => Math.max(1, ...dailyDays.value.map(d => (d.up || 0) + (d.down || 0))))
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
        <div class="side-stats-bars" v-if="dailyDays.length">
          <div
            v-for="(d, i) in dailyDays"
            :key="d.date + i"
            class="bar"
            :title="`${d.date}  ↑${fmtB(d.up)}  ↓${fmtB(d.down)}`"
          >
            <div class="bar-fill" :style="{ height: ((((d.up||0)+(d.down||0)) / dailyMax) * 100) + '%' }"></div>
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

/* 侧栏流量统计 */
.side-stats {
  margin-top: auto;
  padding: 10px 10px 12px;
  border-top: 1px solid #1f2937;
  color: #cbd5e1;
  font-size: 11px;
}
.side-stats-title {
  display: flex; justify-content: space-between; align-items: center;
  color: #f1f5f9; font-weight: 700; font-size: 12px; margin-bottom: 4px;
}
.side-stats-tot { color: #94a3b8; font-weight: 500; font-size: 10px; }
.side-stats-today { color: #cbd5e1; font-size: 11px; margin-bottom: 8px; }
.side-stats-bars {
  display: flex; align-items: flex-end; gap: 2px; height: 44px;
}
.bar {
  flex: 1; background: transparent;
  border-radius: 2px 2px 0 0;
  position: relative; height: 100%;
  display: flex; align-items: flex-end;
}
.bar-fill {
  width: 100%; background: linear-gradient(180deg, #3b82f6 0%, #1e40af 100%);
  border-radius: 2px 2px 0 0; min-height: 2px;
}
.side-stats-empty { color: #64748b; font-size: 11px; }
</style>
