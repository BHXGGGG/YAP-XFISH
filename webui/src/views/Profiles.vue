<script setup lang="ts">
import { computed, ref } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

const modes: [string, string][] = [['global', '全局'], ['rule', '规则'], ['direct', '直连']]

// 协议短名 → { 标签, 颜色 }
const PROTOCOL_META: Record<string, { label: string; color: string }> = {
  shadowsocks: { label: 'SS',       color: '#0ea5e9' },
  ss:          { label: 'SS',       color: '#0ea5e9' },
  vmess:       { label: 'VMess',    color: '#a855f7' },
  vless:       { label: 'VLESS',    color: '#ec4899' },
  trojan:      { label: 'Trojan',   color: '#f97316' },
  hysteria2:   { label: 'Hy2',      color: '#22c55e' },
  hysteria:    { label: 'Hy2',      color: '#22c55e' },
  tuic:        { label: 'TUIC',     color: '#14b8a6' },
  socks:       { label: 'SOCKS',    color: '#64748b' },
  socks5:      { label: 'SOCKS',    color: '#64748b' },
  http:        { label: 'HTTP',     color: '#64748b' },
  https:       { label: 'HTTP',     color: '#64748b' },
  wireguard:   { label: 'WG',       color: '#3b82f6' },
  ssh:         { label: 'SSH',      color: '#737373' },
}
function protocolMeta(kind: string) {
  return PROTOCOL_META[kind] || { label: kind || '?', color: '#64748b' }
}

// 仅显示「启用订阅 + 手动节点」；停用订阅的节点不出现在列表中（其数据仍保留，重新启用即可恢复）。
const visibleNodes = computed(() => {
  const enabled = new Set(
    store.subscriptions.filter((s: any) => s.enabled).map((s: any) => s.id)
  )
  return store.profile.nodes.filter(
    (n: any) => n.subscription_id == null || enabled.has(n.subscription_id)
  )
})

// 按延迟排序（未测速的排最后）。开关由「按延迟排序」按钮控制。
const sortByLatency = ref(false)
const displayedNodes = computed(() => {
  if (!sortByLatency.value) return visibleNodes.value
  return [...visibleNodes.value].sort((a: any, b: any) => {
    // ok 的按数值升序；超时的排 ok 之后；未测速永远最后
    const rank = (n: any) => (n.latency_status === 'ok' ? 0 : n.latency_status === 'timeout' ? 1 : 2)
    const ra = rank(a), rb = rank(b)
    if (ra !== rb) return ra - rb
    const la = a.latency == null ? Infinity : a.latency
    const lb = b.latency == null ? Infinity : b.latency
    return la - lb
  })
})

async function sel(id: string) {
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
async function testOne(id: string) {
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
    <div class="row">
      <span>代理模式：</span>
      <button v-for="[k, l] in modes" :key="k" :class="{ active: store.profile.mode === k }" @click="setMode(k)">{{ l }}</button>
      <button :class="{ active: sortByLatency }" @click="sortByLatency = !sortByLatency">按延迟排序</button>
      <button class="primary" style="margin-left:auto" @click="testAll">测试全部延迟</button>
    </div>
    <div v-if="!visibleNodes.length" class="empty">
      暂无节点。请在「订阅」中添加订阅，或后续手动导入。
    </div>

    <!-- 3 列网格。每个节点卡片：协议 tag + 名称（首行）、server:port（次行）、延迟（第三行）、底部两按钮。 -->
    <div class="nodes-grid">
      <div
        v-for="n in displayedNodes"
        :key="n.id"
        class="node-card"
        :class="{ sel: n.id === store.profile.selected_node, testing: n.testing }"
      >
        <div class="card-head">
          <span class="proto-tag" :style="{ background: protocolMeta(n.type).color }">
            {{ protocolMeta(n.type).label }}
          </span>
          <span class="node-name" :title="n.name">{{ n.name }}</span>
        </div>
        <div class="card-server" :title="n.server + ':' + n.port">
          {{ n.server }}:{{ n.port }}
        </div>
        <div class="card-latency">
          <template v-if="n.testing">
            <span class="meta-testing">● 测速中…</span>
          </template>
          <template v-else-if="n.latency_status === 'ok' && n.latency != null">
            <b :class="{ good: n.latency < 200, mid: n.latency >= 200 && n.latency < 500, bad: n.latency >= 500 }">{{ n.latency }} ms</b>
          </template>
          <span v-else-if="n.latency_status === 'timeout'" class="meta-timeout">不可达</span>
          <span v-else class="meta-untested">未测速</span>
        </div>
        <div class="card-actions">
          <button class="mini" @click="testOne(n.id)" :disabled="n.testing">测速</button>
          <button
            class="mini"
            :class="{ primary: n.id !== store.profile.selected_node }"
            :disabled="n.id === store.profile.selected_node"
            @click="sel(n.id)"
          >{{ n.id === store.profile.selected_node ? '当前' : '选择' }}</button>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
/* 3 列网格（响应式：宽屏 3 列 / 中屏 2 列 / 窄屏 1 列） */
.nodes-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 10px;
}
@media (max-width: 1100px) {
  .nodes-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
@media (max-width: 640px) {
  .nodes-grid { grid-template-columns: 1fr; }
}

.node-card {
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
  transition: border-color .15s, box-shadow .15s;
}
.node-card.sel {
  border-color: var(--primary);
  box-shadow: 0 0 0 1px var(--primary) inset;
}
.node-card.testing {
  border-color: #93c5fd;
  background: #eff6ff;
}

.card-head { display: flex; align-items: center; gap: 6px; min-width: 0; }
.node-name {
  font-size: 13px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
  flex: 1;
}
.card-server {
  font-size: 11px;
  color: #6b7280;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.card-latency { font-size: 12px; min-height: 18px; }
.card-actions { display: flex; gap: 6px; margin-top: 4px; }
.card-actions .mini { flex: 1; padding: 4px 8px; font-size: 12px; }

/* 协议标签：醒目彩色（实色块 + 白色字） */
.proto-tag {
  display: inline-block;
  padding: 1px 7px;
  border-radius: 3px;
  font-size: 10px;
  font-weight: 700;
  color: #fff;
  letter-spacing: 0.4px;
  flex-shrink: 0;
}

/* 延迟颜色分级（< 200 绿 / 200-500 黄 / ≥ 500 红） */
.good { color: #16a34a; font-weight: 700; }
.mid  { color: #f59e0b; font-weight: 700; }
.bad  { color: #dc2626; font-weight: 700; }

.meta-timeout  { color: #dc2626; }
.meta-untested { color: #9ca3af; }
.meta-testing  { color: #2563eb; }
</style>
