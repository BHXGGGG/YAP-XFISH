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
</script>

<template>
  <section>
    <h2>节点管理</h2>
    <p class="hint">提示：节点区已合并到「仪表盘」首页。本页保留以兼容旧入口与深链接。</p>

    <div class="row">
      <span>代理模式：</span>
      <button v-for="[k, l] in modes" :key="k"
              :class="{ active: store.profile.mode === k }"
              @click="setMode(k)">{{ l }}</button>
      <button :class="{ active: sortByLatency }"
              @click="sortByLatency = !sortByLatency">按延迟排序</button>
      <button class="primary" style="margin-left:auto" @click="testAll">测试全部延迟</button>
    </div>

    <div v-if="!visibleNodes.length" class="empty">
      暂无节点。请在「订阅」中添加订阅，或后续手动导入。
    </div>

    <div class="nodes-grid">
      <div v-for="n in displayedNodes" :key="n.id"
           class="node-card"
           :class="{ sel: n.id === store.profile.selected_node, testing: n.testing }"
           role="button" tabindex="0"
           :title="n.id === store.profile.selected_node ? '当前节点' : '点击选择此节点'"
           @click="sel(n.id)"
           @keydown.enter.prevent="sel(n.id)"
           @keydown.space.prevent="sel(n.id)">
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
        <div class="card-server" :title="n.server + ':' + n.port">{{ n.server }}:{{ n.port }}</div>
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
  </section>
</template>

<style scoped>
.hint { color: #6b7280; font-size: 12px; margin: 0 0 10px; }
.row { display: flex; align-items: center; gap: 6px; margin-bottom: 12px; flex-wrap: wrap; }
.row button { padding: 4px 10px; font-size: 12px; }
.row button.active { background: var(--primary); color: #fff; }
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
</style>