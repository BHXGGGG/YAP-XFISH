<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

// 只在本页打开时轮询（切走组件卸载即停止），后台不常驻
const POLL_MS = 1000

interface Row {
  id: string
  process: string
  host: string
  port: string
  rule: string
  outbound: string
  upRate: number
  downRate: number
  upload: number
  download: number
  start: string
}

const rows = ref<Row[]>([])
const totals = ref({ up: 0, down: 0 })
const query = ref('')
const showSearch = ref(false)
const loaded = ref(false)
let timer: number | null = null
// 上一轮每条连接的累计字节，用来算实时速度
let prev = new Map<string, { up: number; down: number; t: number }>()

// sing-box 出站 tag 就是节点 id，显示成节点名
const nodeNameById = computed(() => {
  const m = new Map<string, string>()
  for (const n of store.profile.nodes || []) m.set(n.id, n.name)
  return m
})
const tagName = (t: string) => nodeNameById.value.get(t) || t

function baseName(p: string) {
  return (p || '').split(/[\\/]/).pop() || ''
}

async function poll() {
  try {
    const data: any = await api.connections()
    const now = Date.now()
    const next = new Map<string, { up: number; down: number; t: number }>()
    rows.value = (Array.isArray(data?.connections) ? data.connections : []).map((c: any) => {
      const m = c.metadata || {}
      const up = Number(c.upload) || 0
      const down = Number(c.download) || 0
      const p = prev.get(c.id)
      const dt = p ? (now - p.t) / 1000 : 0
      next.set(c.id, { up, down, t: now })
      const chains: string[] = Array.isArray(c.chains) ? c.chains : []
      return {
        id: c.id,
        process: baseName(m.processPath || m.process || ''),
        host: m.host || m.destinationIP || '—',
        port: m.destinationPort ? String(m.destinationPort) : '',
        // 规则列只留匹配条件，出口单独一列
        rule: String(c.rule || '').split(' => ')[0] || '—',
        outbound: tagName(chains[0] || c.outbound || '—'),
        upRate: dt > 0 ? Math.max(0, (up - p!.up) / dt) : 0,
        downRate: dt > 0 ? Math.max(0, (down - p!.down) / dt) : 0,
        upload: up,
        download: down,
        start: c.start || '',
      }
    })
    rows.value.sort((a, b) => (b.start || '').localeCompare(a.start || ''))
    prev = next
    totals.value = { up: Number(data?.uploadTotal) || 0, down: Number(data?.downloadTotal) || 0 }
  } catch { /* 核心未运行等：保持上次结果 */ }
  loaded.value = true
}
// 直连连接默认隐藏（只看走代理的），勾选记在本地
const DIRECT_LS_KEY = 'yap-xfish.connections.show-direct'
const showDirect = ref(localStorage.getItem(DIRECT_LS_KEY) === '1')
watch(showDirect, (v) => {
  try { localStorage.setItem(DIRECT_LS_KEY, v ? '1' : '0') } catch {}
})
const visible = computed(() =>
  showDirect.value ? rows.value : rows.value.filter((r) => r.outbound !== 'direct'),
)
const hiddenDirect = computed(() => rows.value.length - visible.value.length)

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase()
  if (!q) return visible.value
  return visible.value.filter((r) =>
    [r.host, r.rule, r.outbound, r.process].some((s) => s.toLowerCase().includes(q)),
  )
})

function fmtB(n: number) {
  if (!n || n < 0) return '0B'
  const u = ['B', 'KB', 'MB', 'GB']
  let i = 0
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++ }
  return (i ? n.toFixed(1) : Math.round(n)) + u[i]
}

async function closeOne(r: Row) {
  try {
    await api.closeConnection(r.id)
    rows.value = rows.value.filter((x) => x.id !== r.id)
  } catch (e: any) { toast(e.message) }
}

async function closeAll() {
  if (!rows.value.length) return
  if (!confirm(`断开全部 ${rows.value.length} 个连接？`)) return
  try {
    await api.closeAllConnections()
    rows.value = []
    toast('已断开全部连接')
  } catch (e: any) { toast(e.message) }
}

onMounted(() => {
  void poll()
  timer = window.setInterval(() => { void poll() }, POLL_MS)
})
onBeforeUnmount(() => {
  if (timer) window.clearInterval(timer)
  timer = null
})
</script>

<template>
  <section class="conn-page">
    <header class="head">
      <div>
        <h2>实时连接</h2>
        <p class="sub">
          {{ filtered.length }} 个连接<template v-if="hiddenDirect"> · 已隐藏 {{ hiddenDirect }} 个直连</template>
          · 总计 ↑ {{ fmtB(totals.up) }} · ↓ {{ fmtB(totals.down) }}
        </p>
      </div>
      <div class="tools">
        <label class="direct-toggle" title="直连（direct）连接默认不显示">
          <input v-model="showDirect" type="checkbox" />显示直连
        </label>
        <input
          v-if="showSearch"
          v-model="query"
          class="search"
          type="text"
          placeholder="搜索 目标 / 规则 / 出口"
          autofocus
        />
        <button class="icon" title="搜索" @click="showSearch = !showSearch; if (!showSearch) query = ''">⌕</button>
        <button class="icon danger" title="断开全部" :disabled="!rows.length" @click="closeAll">⊘</button>
      </div>
    </header>

    <div v-if="!store.status.running" class="empty">代理未运行，暂无连接。</div>
    <div v-else-if="loaded && !filtered.length" class="empty">
      {{ query ? '没有匹配的连接。' : '暂无活动连接。' }}
    </div>
    <div v-else class="table-wrap">
      <table>
        <thead>
          <tr>
            <th class="c-proc">进程</th>
            <th class="c-target">目标</th>
            <th class="c-rule">规则</th>
            <th class="c-out">出口</th>
            <th class="num">下载</th>
            <th class="num">上传</th>
            <th class="num">下载总量</th>
            <th class="num">上传总量</th>
            <th class="c-x"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="r in filtered" :key="r.id">
            <td class="c-proc" :title="r.process">{{ r.process || '—' }}</td>
            <td class="c-target" :title="r.host + (r.port ? ':' + r.port : '')">
              {{ r.host }}<span v-if="r.port" class="port">:{{ r.port }}</span>
            </td>
            <td class="c-rule" :title="r.rule">{{ r.rule }}</td>
            <td class="c-out">
              <span class="chip" :class="{ direct: r.outbound === 'direct' }" :title="r.outbound">{{ r.outbound }}</span>
            </td>
            <td class="num" :class="{ live: r.downRate > 0 }">{{ fmtB(r.downRate) }}/s</td>
            <td class="num" :class="{ live: r.upRate > 0 }">{{ fmtB(r.upRate) }}/s</td>
            <td class="num">{{ fmtB(r.download) }}</td>
            <td class="num">{{ fmtB(r.upload) }}</td>
            <td class="c-x">
              <button class="close" title="断开此连接" @click="closeOne(r)">⊘</button>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>
<style scoped>
.conn-page { display: flex; flex-direction: column; height: 100%; min-height: 0; }
.head { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 14px; }
.head h2 { margin: 0; font-size: 19px; }
.head .sub { margin: 4px 0 0; color: var(--muted); font-size: 12px; }
.tools {
  display: flex; align-items: center; gap: 4px;
  background: var(--panel); border: 1px solid var(--border); border-radius: 999px; padding: 4px;
}
.direct-toggle {
  display: inline-flex; align-items: center; gap: 4px; padding: 0 10px 0 8px;
  color: var(--muted); font-size: 12px; cursor: pointer; user-select: none;
}
.tools .search {
  width: 220px; border: none; outline: none; padding: 6px 10px; font-size: 13px; background: transparent;
}
.tools .icon {
  width: 32px; height: 32px; padding: 0; border: none; border-radius: 999px;
  font-size: 17px; line-height: 32px; background: transparent;
}
.tools .icon:hover { background: #f0f4ff; }
.tools .icon.danger:hover { background: var(--red-bg); color: var(--red); }
.tools .icon:disabled { opacity: .4; cursor: default; background: transparent; }

.table-wrap {
  flex: 1; min-height: 0; overflow: auto;
  background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius);
}
table { width: 100%; min-width: 900px; border-collapse: collapse; font-size: 13px; table-layout: fixed; }
th {
  white-space: nowrap;
  position: sticky; top: 0; z-index: 1; background: #f7f8fa;
  color: var(--muted); font-weight: 600; font-size: 12px; text-align: left;
  padding: 9px 10px; border-bottom: 1px solid var(--border);
}
td {
  padding: 8px 10px; border-bottom: 1px solid #f0f1f3;
  white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
tbody tr:nth-child(even) { background: #fafbfc; }
tbody tr:hover { background: #f0f4ff; }
.num { text-align: right; font-variant-numeric: tabular-nums; }
td.num { color: var(--muted); }
td.num.live { color: var(--text); font-weight: 600; }
.c-proc { width: 90px; color: var(--muted); }
.c-target { width: auto; }
.c-rule { width: 140px; color: var(--muted); font-size: 12px; }
.c-out { width: 170px; }
.c-x { width: 44px; text-align: center; }
th.num { width: 82px; }
.port { color: var(--muted); }
.chip {
  display: inline-block; max-width: 100%; overflow: hidden; text-overflow: ellipsis; vertical-align: middle;
  padding: 2px 8px; border-radius: 6px; font-size: 12px; font-weight: 600;
  background: #eef2ff; color: #3730a3;
}
.chip.direct { background: #f0f2f4; color: var(--muted); }
.close {
  width: 26px; height: 26px; padding: 0; border: none; background: transparent;
  color: var(--muted); font-size: 15px; border-radius: 6px;
}
.close:hover { background: var(--red-bg); color: var(--red); }
</style>