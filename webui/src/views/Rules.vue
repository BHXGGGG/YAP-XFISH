<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

const emit = defineEmits<{ (e: 'refresh'): void }>()

const name = ref('')
// 默认用后缀匹配：整站/子域更符合「测 IP 站」这类场景；精确 domain 只匹配主机本身
const type = ref('domain_suffix')
const payload = ref('')
const outbound = ref('direct')

// sing-box 合法规则类型（渲染器映射为对应字段）
const types: [string, string][] = [
  ['domain_suffix', '域名后缀（推荐，含自身与子域）'],
  ['domain', '域名精确（仅主机本身）'],
  ['domain_keyword', '域名关键字'],
  ['domain_regex', '域名正则'],
  ['geosite', 'GeoSite 分类'],
  ['ip_cidr', 'IP CIDR'],
  ['geoip', 'GeoIP 分类'],
  ['process_name', '进程名'],
  ['port', '端口'],
  ['protocol', '协议'],
]

const presets = ref<any[]>([])
const presetKey = ref('')

// 把规则的 outbound（可能是节点 id 哈希）反查为节点显示名。
// 与 Chain.vue 的 nameOf 保持一致：direct/block 直接显示原字面量；
// 找不到的 id 仍保留原字面量，便于排查悬空 tag。
function nameOf(id: string | null | undefined): string {
  if (!id) return '—'
  if (id === 'direct' || id === 'block') return id
  if (id === 'proxy') return '代理（当前节点）'
  const n = (store.profile.nodes || []).find((x: any) => x.id === id)
  return n?.name || id
}

async function loadPresets() {
  try { presets.value = await api.rulePresets() } catch {}
}

onMounted(loadPresets)

async function add() {
  if (!payload.value) { toast('请填写匹配内容'); return }
  try {
    await api.addRule({ name: name.value || payload.value, type: type.value, payload: payload.value, outbound: outbound.value })
    name.value = ''
    payload.value = ''
    toast('规则已添加')
    emit('refresh')
  } catch (e: any) { toast(e.message) }
}

async function addPreset() {
  const p = presets.value.find((x) => x.id === presetKey.value)
  if (!p) { toast('请选择预设'); return }
  try {
    await api.addRule({ name: p.name, type: p.type, payload: p.payload, outbound: p.outbound })
    toast('已添加预设规则')
    emit('refresh')
  } catch (e: any) { toast(e.message) }
}

async function del(id: string) {
  try { await api.deleteRule(id); toast('已删除'); emit('refresh') } catch (e: any) { toast(e.message) }
}
</script>

<template>
  <section>
    <h2>路由规则</h2>
    <p class="hint" style="margin:8px 0 12px">
      <b>域名匹配说明：</b>
      <code>域名精确</code> 只匹配主机本身（如 <code>ip.skk.moe</code>），
      不会匹配 <code>ip.api.skk.moe</code> / <code>cdn.skk.moe</code>。
      测 IP 站这类页面通常还有 API/CDN 子域，请用
      <code>域名后缀</code> 填 <code>skk.moe</code>（匹配所有 <code>*.skk.moe</code> 与自身）。
      粘贴完整 URL 也会自动提取主机名。规则只改匹配到的流量，其它仍走默认节点。
    </p>
    <div class="form">
      <input v-model="name" type="text" placeholder="名称（可选）" />
      <select v-model="type">
        <option v-for="[k, l] in types" :key="k" :value="k">{{ l }}</option>
      </select>
      <input v-model="payload" type="text" placeholder="如 skk.moe（后缀）/ ip.skk.moe（精确）/ geosite:cn / 192.168.0.0/16" style="min-width: 300px" />
      <select v-model="outbound">
        <option value="direct">direct</option>
        <option value="proxy">代理（当前节点）</option>
        <option value="block">block</option>
        <option v-for="n in store.profile.nodes" :key="n.id" :value="n.id">{{ n.name }}</option>
      </select>
      <button class="primary" @click="add">添加规则</button>
    </div>

    <div class="form" style="margin-top:10px">
      <span class="hint">常用预设：</span>
      <select v-model="presetKey">
        <option value="">— 选择预设 —</option>
        <option v-for="p in presets" :key="p.id" :value="p.id">
          {{ p.name }}（{{ p.type }}:{{ p.payload }} → {{ nameOf(p.outbound) }}）
        </option>
      </select>
      <button @click="addPreset">添加预设</button>
    </div>

    <ul class="list">
      <li v-for="r in store.rules" :key="r.id">
        <div>
          <div class="name">{{ r.name }} <span class="tag">{{ r.type }}</span> → <b>{{ nameOf(r.outbound) }}</b></div>
          <div class="meta">{{ r.payload }}</div>
        </div>
        <button @click="del(r.id)">删除</button>
      </li>
      <li v-if="!store.rules.length" class="empty">暂无规则</li>
    </ul>
  </section>
</template>
