<script setup lang="ts">
import { computed, ref } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

const emit = defineEmits<{ (e: 'refresh'): void }>()

const q = ref('')

const nodes = computed(() => {
  const enabled = new Set(
    store.subscriptions.filter((s: any) => s.enabled).map((s: any) => s.id)
  )
  return (store.profile.nodes || []).filter(
    (n: any) => n.subscription_id == null || enabled.has(n.subscription_id)
  )
})

const filtered = computed(() => {
  const s = q.value.trim().toLowerCase()
  if (!s) return nodes.value
  return nodes.value.filter(
    (n: any) =>
      (n.name || '').toLowerCase().includes(s) ||
      (n.server || '').toLowerCase().includes(s) ||
      (n.type || n.kind || '').toLowerCase().includes(s)
  )
})

/** 已配置链式的列表 */
const chained = computed(() =>
  nodes.value.filter((n: any) => n.detour && String(n.detour).trim())
)

function isUdpExit(n: any): boolean {
  const k = String(n.type || n.kind || '').toLowerCase()
  return ['hysteria', 'hysteria2', 'hy2', 'tuic', 'wireguard', 'wg'].includes(k)
}

function nameOf(id: string | null | undefined): string {
  if (!id) return '—'
  const n = nodes.value.find((x: any) => x.id === id)
  return n?.name || id
}

function detourOptions(selfId: string) {
  return nodes.value.filter((n: any) => n.id !== selfId)
}

async function setDetour(id: string, detour: string) {
  const value = detour && detour.trim() ? detour.trim() : null
  try {
    await api.setNodeDetour(id, value)
    const n = store.profile.nodes.find((x: any) => x.id === id)
    if (n) n.detour = value
    toast(value ? '已设置前置代理' : '已清除前置代理')
  } catch (e: any) {
    toast(e.message || '设置失败')
  }
}

async function clearAll() {
  const list = chained.value.slice()
  if (!list.length) {
    toast('当前没有链式配置')
    return
  }
  let ok = 0
  for (const n of list) {
    try {
      await api.setNodeDetour(n.id, null)
      const m = store.profile.nodes.find((x: any) => x.id === n.id)
      if (m) m.detour = null
      ok++
    } catch (e: any) {
      toast(e.message || '清除失败')
    }
  }
  toast(`已清除 ${ok} 条链式配置`)
}
</script>

<template>
  <section>
    <h2>链式代理</h2>
    <p class="hint">
      为「出口节点」指定一个「前置节点」：拨号出口时先经前置再连出口（sing-box
      <code>detour</code>）。
      <br />
      <b>注意：</b>Hysteria2 / TUIC / WireGuard 等 UDP/QUIC 出口无法经前置链式，请用
      VLESS / VMess / Trojan / SS 等 TCP 出口。
    </p>

    <div class="toolbar">
      <input
        v-model="q"
        class="search"
        type="search"
        placeholder="搜索节点名称 / 服务器 / 协议"
      />
      <button @click="clearAll" :disabled="!chained.length">清除全部链式</button>
      <span class="stat">已配置 {{ chained.length }} 条</span>
    </div>

    <div v-if="chained.length" class="panel">
      <h3>当前链路</h3>
      <ul class="chain-list">
        <li v-for="n in chained" :key="'c-' + n.id">
          <span class="exit" :title="n.name">{{ n.name }}</span>
          <span class="arrow">← 经</span>
          <span class="entry" :title="nameOf(n.detour)">{{ nameOf(n.detour) }}</span>
          <button class="link" @click="setDetour(n.id, '')">清除</button>
        </li>
      </ul>
    </div>
    <div v-else class="empty">尚未配置任何链式代理。在下方为出口节点选择前置即可。</div>

    <h3 style="margin-top: 18px">为节点设置前置</h3>
    <div v-if="!filtered.length" class="empty">没有匹配的节点。</div>
    <div v-else class="table-wrap">
      <table class="tbl">
        <thead>
          <tr>
            <th>出口节点</th>
            <th>协议</th>
            <th>服务器</th>
            <th>前置代理</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="n in filtered" :key="n.id" :class="{ udp: isUdpExit(n) }">
            <td class="name" :title="n.name">{{ n.name }}</td>
            <td class="mono">{{ n.type || n.kind || '—' }}</td>
            <td class="mono">{{ n.server }}:{{ n.port }}</td>
            <td>
              <template v-if="isUdpExit(n)">
                <span class="warn" title="UDP/QUIC 出口不支持经前置链式">不支持链式</span>
              </template>
              <select
                v-else
                class="sel"
                :value="n.detour || ''"
                @change="setDetour(n.id, ($event.target as HTMLSelectElement).value)"
              >
                <option value="">无前置</option>
                <option
                  v-for="opt in detourOptions(n.id)"
                  :key="opt.id"
                  :value="opt.id"
                >
                  {{ opt.name }}
                </option>
              </select>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>

<style scoped>
.hint {
  color: var(--muted);
  font-size: 13px;
  line-height: 1.55;
  margin: 0 0 14px;
}
.hint code {
  font-size: 12px;
  background: #f3f4f6;
  padding: 1px 5px;
  border-radius: 4px;
}
.toolbar {
  display: flex;
  gap: 10px;
  align-items: center;
  flex-wrap: wrap;
  margin-bottom: 14px;
}
.search {
  flex: 1 1 220px;
  min-width: 160px;
  max-width: 360px;
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
}
.stat {
  color: var(--muted);
  font-size: 12px;
}
.panel {
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 12px 14px;
  margin-bottom: 12px;
}
.panel h3 {
  margin: 0 0 8px;
  font-size: 14px;
}
.chain-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.chain-list li {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  flex-wrap: wrap;
}
.exit {
  font-weight: 600;
  color: var(--text);
}
.arrow {
  color: var(--muted);
  font-size: 12px;
}
.entry {
  color: #2563eb;
  font-weight: 600;
}
.link {
  margin-left: auto;
  border: none;
  background: transparent;
  color: #dc2626;
  cursor: pointer;
  font-size: 12px;
  padding: 2px 6px;
}
.link:hover {
  text-decoration: underline;
}
.empty {
  padding: 16px;
  color: #9ca3af;
  text-align: center;
  background: var(--panel);
  border-radius: var(--radius);
  border: 1px dashed var(--border);
  font-size: 13px;
}
.table-wrap {
  overflow: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--panel);
}
.tbl {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}
.tbl th,
.tbl td {
  padding: 8px 10px;
  border-bottom: 1px solid var(--border);
  text-align: left;
  vertical-align: middle;
}
.tbl th {
  background: #f9fafb;
  color: var(--muted);
  font-weight: 600;
  font-size: 12px;
  position: sticky;
  top: 0;
}
.tbl tr:last-child td {
  border-bottom: none;
}
.tbl tr.udp td {
  opacity: 0.75;
}
.name {
  max-width: 220px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  font-weight: 600;
}
.mono {
  font-family: ui-monospace, monospace;
  font-size: 12px;
  color: #4b5563;
}
.sel {
  min-width: 160px;
  max-width: 280px;
  padding: 4px 6px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 12px;
  background: #fff;
}
.warn {
  color: #b45309;
  background: #fffbeb;
  border: 1px solid #fcd34d;
  border-radius: 999px;
  padding: 2px 8px;
  font-size: 11px;
  font-weight: 600;
}
</style>
