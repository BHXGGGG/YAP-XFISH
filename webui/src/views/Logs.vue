<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'
import Modal from '../components/Modal.vue'

const SOURCE_COLOR: Record<string, string> = {
  core:    '#3b82f6',
  sub:     '#22c55e',
  http:    '#6b7280',
  config:  '#a855f7',
  latency: '#f59e0b',
  net:     '#06b6d4',
  app:     '#9ca3af',
}

const filteredLogs = computed(() =>
  store.logFilter === 'all'
    ? store.logs
    : store.logs.filter((l: any) => l.level === store.logFilter)
)

function fmtTime(ts: number) {
  const d = new Date(ts)
  return `${String(d.getHours()).padStart(2,'0')}:${String(d.getMinutes()).padStart(2,'0')}:${String(d.getSeconds()).padStart(2,'0')}`
}

function setFilter(f: 'all' | 'info' | 'warn' | 'error') { store.logFilter = f }
function clearLogs() { store.logs.splice(0, store.logs.length) }

const counts = computed(() => {
  const c = { info: 0, warn: 0, error: 0 }
  for (const l of store.logs) {
    if (l.level === 'info' || l.level === 'warn' || l.level === 'error') {
      c[l.level as 'info' | 'warn' | 'error']++
    }
  }
  return c
})

const autoscroll = ref(true)
const logEl = ref<HTMLElement | null>(null)
watch(filteredLogs, () => {
  if (!autoscroll.value) return
  queueMicrotask(() => {
    if (logEl.value) logEl.value.scrollTop = logEl.value.scrollHeight
  })
})

/* ---------- 代理与系统代理 / TUN 快捷开关 ---------- */
const running = computed(() => !!store.status.running)
const sysproxyOn = computed({
  get: () => !!(store.config.system_proxy || store.status.system_proxy),
  set: (v: boolean) => { void toggleConfigFlag('system_proxy', v) },
})
const tunOn = computed({
  get: () => !!(store.config.enable_tun || store.status.enable_tun),
  set: (v: boolean) => { void toggleConfigFlag('enable_tun', v) },
})

async function callAndRefresh(label: string, fn: () => Promise<unknown>) {
  try {
    await fn()
    const [s, c] = await Promise.all([api.status(), api.config()])
    Object.assign(store.status, s)
    Object.assign(store.config, c)
    store.status.system_proxy = !!(s as any).system_proxy || !!(c as any).system_proxy
    store.status.enable_tun = !!(s as any).enable_tun || !!(c as any).enable_tun
    toast(label + ' 已完成')
  } catch (e: any) { toast(`${label} 失败: ${e.message}`) }
}

async function startCore() { await callAndRefresh('启动代理', () => api.coreStart()) }
async function stopCore()  { await callAndRefresh('停止代理', () => api.coreStop()) }
async function restartCore() { await callAndRefresh('重启代理', () => api.coreRestart()) }
async function updateAllSubs() { await callAndRefresh('更新全部订阅', () => api.updateAllSubscriptions()) }

async function toggleConfigFlag(key: 'system_proxy' | 'enable_tun', value: boolean) {
  // TUN 开启且未提权 → 弹提权引导弹窗
  if (key === 'enable_tun' && value === true && !store.status.elevated) {
    // 提权引导：先把 enable_tun=true 持久化（写盘不需要管理员权限）
    // 这样提权后的新实例启动时会读到 enable_tun=true 并自动启用 TUN。
    try {
      const next = { ...store.config, [key]: value }
      const updated = await api.updateConfig(next as any)
      Object.assign(store.config, updated)
    } catch (e: any) {
      toast(e.message || '保存设置失败，请重试')
      return
    }
    showElevateDialog.value = true
    return
  }
  try {
    const next = { ...store.config, [key]: value }
    const updated = await api.updateConfig(next as any)
    Object.assign(store.config, updated)
    // status 同步：系统代理 / TUN 启用状态会被后端广播
    if (key === 'system_proxy') {
      store.status.system_proxy = value
      toast(value ? '已开启系统代理' : '已关闭系统代理')
    } else {
      store.status.enable_tun = value
      toast(value ? '已开启 TUN' : '已关闭 TUN')
    }
  } catch (e: any) { toast(e.message) }
}

/* ---------- TUN 提权引导 ---------- */
const showElevateDialog = ref(false)
function onElevateConfirm() {
  showElevateDialog.value = false
  api.adminElevate().then(() => {
    toast('正在以管理员身份重新启动…')
  }).catch((e: any) => {
    toast('提权失败: ' + e.message)
  })
}
function onElevateCancel() {
  showElevateDialog.value = false
}
</script>

<template>
  <section>
    <h2>实时日志</h2>

    <div class="ctrl-card">
      <div class="ctrl-row">
        <span class="ctrl-label">代理状态</span>
        <span class="badge" :class="running ? 'on' : 'off'">{{ running ? '运行中' : '已停止' }}</span>
        <button class="primary" :disabled="running" @click="startCore">启动代理</button>
        <button :disabled="!running" @click="stopCore">停止代理</button>
        <button @click="restartCore">重启代理</button>
        <button @click="updateAllSubs">更新全部订阅</button>
      </div>
      <div class="ctrl-row">
        <label class="ctrl-switch">
          <input type="checkbox" v-model="sysproxyOn" />
          <span>系统代理</span>
          <em :class="sysproxyOn ? 'on' : 'off'">{{ sysproxyOn ? '开' : '关' }}</em>
        </label>
        <label class="ctrl-switch">
          <input type="checkbox" v-model="tunOn" />
          <span>TUN 模式</span>
          <em :class="tunOn ? 'on' : 'off'">{{ tunOn ? '开' : '关' }}</em>
        </label>
        <span class="ctrl-hint">操作会触发对应后端事件，日志区会实时收到反馈。</span>
      </div>
    </div>

    <div class="log-filters">
      <button :class="{ active: store.logFilter === 'all' }" @click="setFilter('all')">
        全部 <span class="badge-mini">{{ store.logs.length }}</span>
      </button>
      <button :class="{ active: store.logFilter === 'info' }" @click="setFilter('info')">
        info <span class="badge-mini">{{ counts.info }}</span>
      </button>
      <button :class="{ active: store.logFilter === 'warn' }" @click="setFilter('warn')">
        warn <span class="badge-mini warn">{{ counts.warn }}</span>
      </button>
      <button :class="{ active: store.logFilter === 'error' }" @click="setFilter('error')">
        error <span class="badge-mini error">{{ counts.error }}</span>
      </button>
      <button :class="{ active: autoscroll }" @click="autoscroll = !autoscroll" title="自动滚到最新一行">
        自动滚动
      </button>
      <button class="clear" @click="clearLogs" title="清空当前日志（不影响后端）">清空</button>
    </div>

    <div v-if="!filteredLogs.length" class="log-empty">暂无日志</div>
    <div v-else class="log" ref="logEl">
      <div v-for="(l, i) in filteredLogs" :key="i"
           class="log-row" :class="['lv-' + (l.level || 'info'), 'src-' + (l.source || 'app')]">
        <span class="ts">{{ fmtTime(l.ts) }}</span>
        <span class="src" :style="{ background: SOURCE_COLOR[l.source] || SOURCE_COLOR.app }">
          {{ l.source || 'app' }}
        </span>
        <span class="lvl">{{ l.level || 'info' }}</span>
        <span class="msg">{{ l.message }}</span>
      </div>
    </div>

    <Modal :open="showElevateDialog" title="需要管理员权限" subtitle="TUN 模式需要管理员权限才能创建虚拟网卡" icon="⚡" :width="440" @close="onElevateCancel">
      <div class="elevate-body">
        <p>当前未以管理员身份运行，启用 TUN 后 sing-box 无法创建虚拟网卡。</p>
        <p>是否立即<strong>以管理员身份重新启动</strong> YAP-XFISH？重启后自动启用 TUN。</p>
        <div class="elevate-actions">
          <button class="primary elevate-btn" @click="onElevateConfirm">以管理员身份运行</button>
          <button @click="onElevateCancel">取消</button>
        </div>
      </div>
    </Modal>
  </section>
</template>

<style scoped>
.ctrl-card {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px 14px;
  margin-bottom: 14px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--panel);
}
.ctrl-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
}
.ctrl-label {
  font-weight: 600;
  color: #374151;
}
.ctrl-switch {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  user-select: none;
}
.ctrl-switch input { width: 16px; height: 16px; }
.ctrl-switch em {
  font-style: normal;
  font-size: 12px;
  padding: 1px 8px;
  border-radius: 999px;
  background: #f3f4f6;
  color: #6b7280;
}
.ctrl-switch em.on  { background: #dcfce7; color: #166534; }
.ctrl-switch em.off { background: #fee2e2; color: #991b1b; }
.ctrl-hint { color: #9ca3af; font-size: 12px; }
.badge {
  display: inline-block;
  padding: 2px 10px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
}
.badge.on  { background: #dcfce7; color: #166534; }
.badge.off { background: #fee2e2; color: #991b1b; }

.log-filters { display: flex; gap: 6px; align-items: center; margin-bottom: 6px; flex-wrap: wrap; }
.log-filters button { padding: 3px 10px; font-size: 12px; }
.log-filters button.active { background: var(--primary); color: #fff; }
.log-filters button.clear  { color: #b91c1c; }
.badge-mini {
  display: inline-block; min-width: 18px; padding: 0 5px; margin-left: 4px;
  background: #e5e7eb; color: #374151; border-radius: 9px;
  font-size: 11px; text-align: center; vertical-align: middle;
}
.badge-mini.warn  { background: #fef3c7; color: #92400e; }
.badge-mini.error { background: #fee2e2; color: #991b1b; }
.log-empty {
  padding: 16px; color: #9ca3af; text-align: center;
  background: var(--panel); border-radius: var(--radius);
}
.log {
  max-height: 480px; overflow-y: auto;
  background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
}
.log-row {
  display: grid; grid-template-columns: 76px 56px 50px 1fr; gap: 8px;
  padding: 3px 10px; border-bottom: 1px solid #f3f4f6; align-items: baseline;
}
.log-row:hover { background: #f9fafb; }
.ts { color: #9ca3af; }
.src { color: #fff; text-align: center; border-radius: 3px; font-size: 10px; font-weight: 700; letter-spacing: .3px; padding: 1px 0; }
.lvl { font-weight: 700; }
.msg { white-space: pre-wrap; word-break: break-all; }
.lv-info  .lvl { color: #2563eb; }
.lv-warn  { background: #fffbeb; }
.lv-warn  .lvl { color: #d97706; }
.lv-error { background: #fef2f2; }
.lv-error .lvl { color: #dc2626; }

.elevate-body { padding: 4px 0; }
.elevate-body p { margin: 8px 0; line-height: 1.6; color: #374151; font-size: 14px; }
.elevate-actions { display: flex; gap: 10px; margin-top: 18px; justify-content: flex-end; }
.elevate-btn {
  background: linear-gradient(180deg, #3b82f6 0%, #2563eb 100%);
  border: 1px solid #1d4ed8; color: #fff; padding: 8px 18px;
  border-radius: 8px; font-weight: 600;
  box-shadow: 0 4px 12px rgba(37, 99, 235, 0.25);
}
</style>
