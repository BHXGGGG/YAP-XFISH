<script setup lang="ts">
import { computed } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

const filteredLogs = computed(() => {
  if (store.logFilter === 'all') return store.logs
  return store.logs.filter((l: any) => l.level === store.logFilter)
})

// 来源 chip 的背景色：core 蓝、sub 绿、http 灰、config 紫、latency 橙、net/app 默认
const SOURCE_COLOR: Record<string, string> = {
  core:    '#3b82f6',
  sub:     '#22c55e',
  http:    '#6b7280',
  config:  '#a855f7',
  latency: '#f59e0b',
  net:     '#06b6d4',
  app:     '#9ca3af',
}

function fmtTime(ts: number) {
  const d = new Date(ts)
  const hh = String(d.getHours()).padStart(2, '0')
  const mm = String(d.getMinutes()).padStart(2, '0')
  const ss = String(d.getSeconds()).padStart(2, '0')
  return `${hh}:${mm}:${ss}`
}

async function start() {
  try { await api.coreStart() } catch (e: any) { toast(e.message) }
}
async function stop() {
  try { await api.coreStop() } catch (e: any) { toast(e.message) }
}
async function restart() {
  try { await api.coreRestart() } catch (e: any) { toast(e.message) }
}
function clearLogs() {
  store.logs.splice(0, store.logs.length)
}
function setFilter(f: 'all' | 'info' | 'warn' | 'error') {
  store.logFilter = f
}

// 错误/警告数（用于过滤按钮 badge）
const counts = computed(() => {
  const c = { info: 0, warn: 0, error: 0 }
  for (const l of store.logs) {
    if (l.level === 'info' || l.level === 'warn' || l.level === 'error') {
      c[l.level as 'info' | 'warn' | 'error']++
    }
  }
  return c
})
</script>

<template>
  <section>
    <h2>仪表盘</h2>
    <div class="cards">
      <div class="card">
        <div class="k">代理状态</div>
        <div class="v">
          <span class="badge" :class="store.status.running ? 'on' : 'off'">{{ store.status.running ? '运行中' : '已停止' }}</span>
        </div>
      </div>
      <div class="card"><div class="k">模式</div><div class="v">{{ store.status.mode }}</div></div>
      <div class="card"><div class="k">当前节点</div><div class="v">{{ store.status.current_node || '—' }}</div></div>
      <div class="card"><div class="k">节点数</div><div class="v">{{ store.status.node_count }}</div></div>
      <div class="card"><div class="k">上行</div><div class="v">{{ (store.status.traffic_up / 1024 / 1024).toFixed(2) }} MB</div></div>
      <div class="card"><div class="k">下行</div><div class="v">{{ (store.status.traffic_down / 1024 / 1024).toFixed(2) }} MB</div></div>
    </div>
    <div class="actions">
      <button class="primary" :disabled="store.status.running" @click="start">启动代理</button>
      <button :disabled="!store.status.running" @click="stop">停止</button>
      <button @click="restart">重启</button>
    </div>

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
      <div
        v-for="(l, i) in filteredLogs"
        :key="i"
        class="log-row"
        :class="['lv-' + (l.level || 'info'), 'src-' + (l.source || 'app')]"
      >
        <span class="ts">{{ fmtTime(l.ts) }}</span>
        <span
          class="src"
          :style="{ background: SOURCE_COLOR[l.source] || SOURCE_COLOR.app }"
        >{{ l.source || 'app' }}</span>
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
.badge { display: inline-block; padding: 2px 10px; border-radius: 4px; font-size: 12px; font-weight: 600; }
.badge.on { background: #dcfce7; color: #166534; }
.badge.off { background: #fee2e2; color: #991b1b; }

.actions { display: flex; gap: 8px; margin: 12px 0 18px; }

.log-head { display: flex; align-items: center; justify-content: space-between; margin: 6px 0 6px; }
.log-filters { display: flex; gap: 6px; align-items: center; }
.log-filters button { padding: 3px 10px; font-size: 12px; }
.log-filters button.active { background: var(--primary); color: #fff; }
.log-filters button.clear { color: #b91c1c; }
.badge-mini {
  display: inline-block; min-width: 18px; padding: 0 5px; margin-left: 4px;
  background: #e5e7eb; color: #374151; border-radius: 9px; font-size: 11px;
  text-align: center; vertical-align: middle;
}
.badge-mini.warn { background: #fef3c7; color: #92400e; }
.badge-mini.error { background: #fee2e2; color: #991b1b; }

.log-empty { padding: 16px; color: #9ca3af; text-align: center; background: var(--panel); border-radius: var(--radius); }
.log {
  max-height: 540px; overflow-y: auto;
  background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
}
.log-row {
  display: grid;
  grid-template-columns: 76px 56px 50px 1fr;
  gap: 8px;
  padding: 3px 10px;
  border-bottom: 1px solid #f3f4f6;
  align-items: baseline;
}
.log-row:hover { background: #f9fafb; }
.ts { color: #9ca3af; }
.src { color: #fff; text-align: center; border-radius: 3px; font-size: 10px; font-weight: 700; letter-spacing: 0.3px; padding: 1px 0; }
.lvl { font-weight: 700; }
.msg { white-space: pre-wrap; word-break: break-all; }

/* 级别染色 */
.lv-info  .lvl { color: #2563eb; }
.lv-warn  { background: #fffbeb; }
.lv-warn  .lvl { color: #d97706; }
.lv-error { background: #fef2f2; }
.lv-error .lvl { color: #dc2626; }
</style>
