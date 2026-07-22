<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

const emit = defineEmits<{ (e: 'refresh'): void }>()

const name = ref('')
const type = ref('domain_suffix')
const payload = ref('')
const outbound = ref('direct')

// sing-box 合法规则类型（渲染器直接透传为 route.rules 的 type）
const types: [string, string][] = [
  ['domain', '域名(精确)'],
  ['domain_suffix', '域名后缀'],
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
    <div class="form">
      <input v-model="name" type="text" placeholder="名称（可选）" />
      <select v-model="type">
        <option v-for="[k, l] in types" :key="k" :value="k">{{ l }}</option>
      </select>
      <input v-model="payload" type="text" placeholder="匹配内容，如 geosite:cn / domain_suffix:example.com / 192.168.0.0/16" style="min-width: 300px" />
      <select v-model="outbound">
        <option value="direct">direct</option>
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
          {{ p.name }}（{{ p.type }}:{{ p.payload }} → {{ p.outbound }}）
        </option>
      </select>
      <button @click="addPreset">添加预设</button>
    </div>

    <ul class="list">
      <li v-for="r in store.rules" :key="r.id">
        <div>
          <div class="name">{{ r.name }} <span class="tag">{{ r.type }}</span> → <b>{{ r.outbound }}</b></div>
          <div class="meta">{{ r.payload }}</div>
        </div>
        <button @click="del(r.id)">删除</button>
      </li>
      <li v-if="!store.rules.length" class="empty">暂无规则</li>
    </ul>
  </section>
</template>
