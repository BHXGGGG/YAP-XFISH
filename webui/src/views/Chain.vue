<script setup lang="ts">
import { computed, ref } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'
import ChainEditor from '../components/ChainEditor.vue'
import { existingChain } from '../chain'

const query = ref('')
const editing = ref<string[] | null>(null)
const busy = ref(false)
const nodes = computed(() => {
  const enabled = new Set(store.subscriptions.filter(sub => sub.enabled).map(sub => sub.id))
  return store.profile.nodes.filter(node => node.subscription_id == null || enabled.has(node.subscription_id))
})
const chained = computed(() => nodes.value.filter(node => node.detour && String(node.detour).trim()))
const filtered = computed(() => chained.value.filter(node => pathLabel(node.id).toLowerCase().includes(query.value.trim().toLowerCase())))
function nameOf(id: string) { return nodes.value.find(node => node.id === id)?.name || id }
function pathLabel(id: string) {
  try { return existingChain(id, nodes.value).map(nameOf).join(' → ') }
  catch { return `前置不可用 → ${nameOf(id)}` }
}
function edit(id: string) {
  try { editing.value = existingChain(id, nodes.value) }
  catch (e: any) { toast(e.message) }
}
async function clear(ids: string[]) {
  busy.value = true
  try {
    await api.setNodeDetours(ids.map(id => ({ id, detour: null })))
    for (const id of ids) {
      const node = store.profile.nodes.find(node => node.id === id)
      if (node) node.detour = ''
    }
    toast(`已清除 ${ids.length} 个节点的前置`)
  } catch (e: any) { toast(e.message || '清除失败') }
  finally { busy.value = false }
}
</script>

<template>
  <section>
    <div class="chain-heading"><h2>链式代理</h2><button :disabled="busy || !nodes.length" @click="editing = []">＋ 新建代理链</button></div>
    <p class="hint">从客户端到互联网依次连接节点，使用最后一跳作为出口节点。</p>
    <div class="chain-toolbar">
      <input v-model="query" type="search" placeholder="搜索链路节点…" aria-label="搜索代理链" />
      <span>已配置 {{ chained.length }} 个出口</span>
      <button :disabled="busy || !chained.length" @click="clear(chained.map(node => node.id))">清除全部链式</button>
    </div>
    <div v-if="!filtered.length" class="empty">{{ chained.length ? '没有匹配的链路。' : '尚未配置代理链，点击“新建代理链”添加节点。' }}</div>
    <div class="chain-list">
      <article v-for="node in filtered" :key="node.id" class="chain-summary">
        <div><strong>{{ node.name }}</strong><p>{{ pathLabel(node.id) }}</p></div>
        <button :disabled="busy" @click="edit(node.id)">编辑链路</button>
        <button :disabled="busy" @click="clear([node.id])">清除前置</button>
      </article>
    </div>
    <ChainEditor v-if="editing !== null" :nodes="nodes" :initial="editing" @close="editing = null" @saved="editing = null" />
  </section>
</template>

<style scoped>
.chain-heading { display: flex; gap: 16px; align-items: center; flex-wrap: wrap; margin-bottom: 14px; }
.chain-heading h2 { margin: 0; }
.chain-heading button { border-radius: 999px; }
.chain-toolbar { display: flex; align-items: center; flex-wrap: wrap; gap: 12px; margin: 18px 0; }
.chain-toolbar input { min-width: 0; flex: 1; max-width: 340px; padding: 8px 12px; border: 1px solid var(--border); border-radius: 999px; font: inherit; }
.chain-toolbar span { color: var(--muted); font-size: 12px; }
.chain-toolbar button { margin-left: auto; font-size: 12px; }
.chain-list { display: grid; gap: 10px; }
.chain-summary { display: flex; align-items: center; gap: 10px; padding: 18px; border: 1px solid var(--border); border-radius: 13px; background: var(--panel); }
.chain-summary > div { flex: 1; min-width: 0; }
.chain-summary strong { font-size: 14px; overflow-wrap: anywhere; }
.chain-summary p { color: #8060bd; font-size: 12px; line-height: 1.6; margin: 8px 0 0; overflow-wrap: anywhere; }
.chain-summary button { flex-shrink: 0; font-size: 12px; border-radius: 999px; }
@media (max-width: 760px) { .chain-summary { flex-wrap: wrap; } .chain-summary > div { flex-basis: 100%; } }
</style>
