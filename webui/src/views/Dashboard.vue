<script setup lang="ts">
import { computed, ref } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

const modes: [string, string][] = [['global', '全局'], ['rule', '规则'], ['direct', '直连']]

const PROTOCOL_META: Record<string, { label: string; color: string }> = {
  shadowsocks: { label: 'SS',     color: '#0ea5e9' },
  ss:          { label: 'SS',     color: '#0ea5e9' },
  vmess:       { label: 'VMess',  color: '#a855f7' },
  vless:       { label: 'VLESS',  color: '#ec4899' },
  trojan:      { label: 'Trojan', color: '#f97316' },
  hysteria2:   { label: 'Hy2',    color: '#22c55e' },
  hysteria:    { label: 'Hy2',    color: '#22c55e' },
  tuic:        { label: 'TUIC',   color: '#14b8a6' },
  socks:       { label: 'SOCKS',  color: '#64748b' },
  socks5:      { label: 'SOCKS',  color: '#64748b' },
  http:        { label: 'HTTP',   color: '#64748b' },
  https:       { label: 'HTTP',   color: '#64748b' },
  wireguard:   { label: 'WG',     color: '#3b82f6' },
  ssh:         { label: 'SSH',    color: '#737373' },
}
function protocolMeta(kind: string) {
  return PROTOCOL_META[kind] || { label: kind || '?', color: '#64748b' }
}

// 仅显示「启用订阅 + 手动节点」
const visibleNodes = computed(() => {
  const enabled = new Set(
    store.subscriptions.filter((s: any) => s.enabled).map((s: any) => s.id)
  )
  return store.profile.nodes.filter(
    (n: any) => n.subscription_id == null || enabled.has(n.subscription_id)
  )
})

const sortByLatency = ref(false)
const displayedNodes = computed(() => {
  if (!sortByLatency.value) return visibleNodes.value
  return [...visibleNodes.value].sort((a: any, b: any) => {
    const rank = (n: any) =>
      n.latency_status === 'ok' ? 0 : n.latency_status === 'timeout' ? 1 : 2
    const ra = rank(a), rb = rank(b)
    if (ra !== rb) return ra - rb
    const la = a.latency == null ? Infinity : a.latency
    const lb = b.latency == null ? Infinity : b.latency
    return la - lb
  })
})

async function sel(id: string) {
  if (id === store.profile.selected_node) return
  try { await api.selectNode(id); toast('已选择节点') } catch (e: any) { toast(e.message) }
}
async function setMode(m: string) {
  try { await api.setMode(m); toast('已切换模式') } catch (e: any) { toast(e.message) }
}
async function testAll() {
  visibleNodes.value.forEach((n: any) => {
    n.testing = true
    if (n._testingTimer) clearTimeout(n._testingTimer)
    n._testingTimer = setTimeout(() => { n.testing = false }, 30_000)
  })
  try { await api.testAllLatency(); toast('已开始测速（结果实时刷新）') } catch (e: any) { toast(e.message) }
}
async function testOne(id: string, ev?: Event) {
  if (ev) ev.stopPropagation()
  const n = visibleNodes.value.find((x: any) => x.id === id)
  if (n) {
    n.testing = true
    if (n._testingTimer) clearTimeout(n._testingTimer)
    n._testingTimer = setTimeout(() => { n.testing = false }, 30_000)
  }
  try { await api.testNodeLatency(id) } catch (e: any) { toast(e.message) }
}

const emit = defineEmits<{ (e: 'navigate', tab: string): void }>()

// 日志
const SOURCE_COLOR: Record<string, string> = {
  core: '#3b82f6', sub: '#22c55e', http: '#6b7280', config: '#a855f7',
  latency: '#f59e0b', net: '#06b6d4', app: '#9ca3af',
}
const filteredLogs = computed(() =>
  store.logFilter === 'all' ? store.logs : store.logs.filter((l: any) => l.level === store.logFilter)
)
function fmtTime(ts: number) {
  const d = new Date(ts)
  return `${String(d.getHours()).padStart(2,'0')}:${String(d.getMinutes()).padStart(2,'0')}:${String(d.getSeconds()).padStart(2,'0')}`
}
async function start() { try { await api.coreStart() } catch (e: any) { toast(e.message) } }
async function stop()  { try { await api.coreStop()  } catch (e: any) { toast(e.message) } }
async function restart(){ try { await api.coreRestart() } catch (e: any) { toast(e.message) } }
function clearLogs() { store.logs.splice(0, store.logs.length) }
function setFilter(f: 'all' | 'info' | 'warn' | 'error') { store.logFilter = f }
const counts = computed(() => {
  const c = { info: 0, warn: 0, error: 0 }
  for (const l of store.logs) {
    if (l.level === 'info' || l.level === 'warn' || l.level === 'error') {
      c[l.level as 'info' | 'warn' | 'error']++
    }
  }
  return c
})
const currentNodeLabel = computed(() => {
  const id = store.status.current_node || store.profile.selected_node
  if (!id) return '—'
  if (store.status.current_node_name) return store.status.current_node_name
  const n = store.profile.nodes.find((x: any) => x.id === id)
  return n?.name || id
})
</script>

<template>
  <section>
    <h2>仪表盘</h2>

    <div class="cards">
      <div class="card">
        <div class="k">代理状态</div>
        <div class="v">
          <span class="badge" :class="store.status.running ? 'on' : 'off'">
            {{ store.status.running ? '运行中' : '已停止' }}
          </span>
        </div>
      </div>
      <div class="card"><div class="k">模式</div><div class="v">{{ store.status.mode }}</div></div>
      <div class="card"><div class="k">当前节点</div><div class="v node-name">{{ currentNodeLabel }}</div></div>
      <div class="card"><div class="k">节点数</div><div class="v">{{ store.status.node_count }}</div></div>
      <div class="card"><div class="k">上行</div><div class="v">{{ (store.status.traffic_up / 1024 / 1024).toFixed(2) }} MB</div></div>
      <div class="card"><div class="k">下行</div><div class="v">{{ (store.status.traffic_down / 1024 / 1024).toFixed(2) }} MB</div></div>
    </div>

    <div class="actions">
      <button class="primary" :disabled="store.status.running" @click="start">启动代理</button>
      <button :disabled="!store.status.running" @click="stop">停止</button>
      <button @click="restart">重启</button>
    </div>

    <!-- 节点区：把原「节点页」直接挪到首页 -->
    <div class="section-head">
      <h3 style="margin:0">节点</h3>
      <div class="row-inline">
        <span class="muted">模式：</span>
        <button v-for="[k, l] in modes" :key="k"
                :class="{ active: store.profile.mode === k }"
                @click="setMode(k)">{{ l }}</button>
        <button :class="{ active: sortByLatency }"
                @click="sortByLatency = !sortByLatency">按延迟排序</button>
        <button class="primary" @click="testAll">测试全部延迟</button>
      </div>
    </div>

    <div v-if="!visibleNodes.length" class="empty">
      暂无节点。请在「订阅」中添加订阅，或后续手动导入。
    </div>

    <div class="nodes-grid">
      <div
        v-for="n in displayedNodes"
        :key="n.id"
        class="node-card"
        :class="{ sel: n.id === store.profile.selected_node, testing: n.testing }"
        role="button" tabindex="0"
        :title="n.id === store.profile.selected_node ? '当前节点' : '点击选择此节点'"
        @click="sel(n.id)"
        @keydown.enter.prevent="sel(n.id)"
        @keydown.space.prevent="sel(n.id)"
      >
        <div class="card-head">
          <span class="proto-tag" :style="{ background: protocolMeta(n.type).color }">
            {{ protocolMeta(n.type).label }}
          </span>
          <span class="node-name" :title="n.name">{{ n.name }}</span>
          <button class="icon-btn"
                  :class="{ spinning: n.testing }"
                  :disabled="n.testing"
                  title="测速" aria-label="测速"
                  @click="testOne(n.id, $event)">⚡</button>
        </div>
        <div class="card-server" :title="n.server + ':' + n.port">
          {{ n.server }}:{{ n.port }}
        </div>
        <div class="card-latency">
          <template v-if="n.testing"><span class="meta-testing">● 测速中…</span></template>
          <template v-else-if="n.latency_status === 'ok' && n.latency != null">
            <b :class="{ good: n.latency < 200, mid: n.latency >= 200 && n.latency < 500, bad: n.latency >= 500 }">
              {{ n.latency }} ms
            </b>
          </template>
          <span v-else-if="n.latency_status === 'timeout'" class="meta-timeout">不可达</span>
          <span v-else class="meta-untested">未测速</span>
        </div>
      </div>
    </div>

    <!-- 日志 -->
    <div class="log-head">
      <h3 style="margin:0">实时日志</h3>
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
        <button class="clear" @click="clearLogs" title="清空当前日志（不影响后端）">清空</button>
      </div>
    </div>

    <div v-if="!filteredLogs.length" class="log-empty">暂无日志</div>
    <div v-else class="log">
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
  </section>
</template>

<style scoped>
.cards { display: grid; grid-template-columns: repeat(6, 1fr); gap: 12px; margin-bottom: 16px; }
.card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 12px 14px; }
.k { font-size: 12px; color: #6b7280; }
.v { font-size: 18px; font-weight: 600; margin-top: 4px; }
.v.node-name {
  font-size: 14px; line-height: 1.35;
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
.badge { display: inline-block; padding: 2px 10px; border-radius: 4px; font-size: 12px; font-weight: 600; }
.badge.on  { background: #dcfce7; color: #166534; }
.badge.off { background: #fee2e2; color: #991b1b; }

.actions { display: flex; gap: 8px; margin: 12px 0 18px; }

/* 节点区 */
.section-head {
  display: flex; align-items: center; justify-content: space-between;
  margin: 8px 0 8px; gap: 8px; flex-wrap: wrap;
}
.row-inline { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.muted { color: #6b7280; font-size: 13px; }
.row-inline button { padding: 4px 10px; font-size: 12px; }
.row-inline button.active { background: var(--primary); color: #fff; }
.empty {
  padding: 16px; color: #9ca3af; text-align: center;
  background: var(--panel); border-radius: var(--radius); border: 1px dashed var(--border);
}

.nodes-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; }
@media (max-width: 1280px) { .nodes-grid { grid-template-columns: repeat(3, minmax(0, 1fr)); } }
@media (max-width: 900px)  { .nodes-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
@media (max-width: 560px)  { .nodes-grid { grid-template-columns: 1fr; } }

.node-card {
  background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius);
  padding: 10px 12px; display: flex; flex-direction: column; gap: 4px; min-width: 0;
  transition: border-color .15s, box-shadow .15s, background .15s; cursor: pointer; user-select: none;
}
.node-card:hover  { border-color: #93c5fd; }
.node-card.sel    { border-color: var(--primary); box-shadow: 0 0 0 1px var(--primary) inset; }
.node-card.testing{ border-color: #93c5fd; background: #eff6ff; }

.card-head { display: flex; align-items: center; gap: 6px; min-width: 0; }
.node-name {
  font-size: 13px; font-weight: 600; white-space: nowrap;
  overflow: hidden; text-overflow: ellipsis; min-width: 0; flex: 1;
}
.card-server {
  font-size: 11px; color: #6b7280; white-space: nowrap;
  overflow: hidden; text-overflow: ellipsis;
}
.card-latency { font-size: 12px; min-height: 18px; }

.icon-btn {
  flex-shrink: 0; width: 28px; height: 28px; padding: 0;
  border: 1px solid var(--border); border-radius: 6px; background: #fff;
  font-size: 14px; line-height: 1; cursor: pointer;
  display: inline-flex; align-items: center; justify-content: center; color: #2563eb;
}
.icon-btn:hover:not(:disabled) { background: #eff6ff; border-color: #93c5fd; }
.icon-btn:disabled { opacity: .55; cursor: not-allowed; }
.icon-btn.spinning { animation: pulse 1s ease-in-out infinite; }
@keyframes pulse {
  0%, 100% { opacity: 1; transform: scale(1); }
  50%      { opacity: .55; transform: scale(.92); }
}

.proto-tag {
  display: inline-block; padding: 1px 7px; border-radius: 3px;
  font-size: 10px; font-weight: 700; color: #fff; letter-spacing: .4px; flex-shrink: 0;
}

.good { color: #16a34a; font-weight: 700; }
.mid  { color: #f59e0b; font-weight: 700; }
.bad  { color: #dc2626; font-weight: 700; }
.meta-timeout  { color: #dc2626; }
.meta-untested { color: #9ca3af; }
.meta-testing  { color: #2563eb; }

/* 日志 */
.log-head { display: flex; align-items: center; justify-content: space-between; margin: 18px 0 6px; }
.log-filters { display: flex; gap: 6px; align-items: center; }
.log-filters button { padding: 3px 10px; font-size: 12px; }
.log-filters button.active { background: var(--primary); color: #fff; }
.log-filters button.clear { color: #b91c1c; }
.badge-mini {
  display: inline-block; min-width: 18px; padding: 0 5px; margin-left: 4px;
  background: #e5e7eb; color: #374151; border-radius: 9px;
  font-size: 11px; text-align: center; vertical-align: middle;
}
.badge-mini.warn  { background: #fef3c7; color: #92400e; }
.badge-mini.error { background: #fee2e2; color: #991b1b; }
.log-empty { padding: 16px; color: #9ca3af; text-align: center; background: var(--panel); border-radius: var(--radius); }
.log {
  max-height: 540px; overflow-y: auto;
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
</style>