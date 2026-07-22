<script setup lang="ts">
import { reactive, ref, watch } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

const emit = defineEmits<{ (e: 'refresh'): void }>()

const form = reactive({
  name: '',
  url: '',
  interval: 'every30_min',
  enabled: true,
  uaPreset: 'default',
  userAgent: '',
})

// 预置 UA：覆盖 link123 等会拦截浏览器 UA 的订阅站。
// 参考 uif / clash-party / v2rayN / sing-box 默认 UA。
const UA_PRESETS: { value: string; label: string; ua: string }[] = [
  { value: 'default', label: '默认（clash-verge/1.7.0）', ua: 'clash-verge/1.7.0' },
  { value: 'clash-party', label: 'Clash Party (mihomo/1.18.0)', ua: 'mihomo/1.18.0' },
  { value: 'clash-verge', label: 'Clash Verge Rev', ua: 'clash-verge/2.0.0' },
  { value: 'v2rayn', label: 'v2rayN 7', ua: 'v2rayN/7.0.0' },
  { value: 'sing-box', label: 'sing-box', ua: 'sing-box/1.9.0' },
  { value: 'custom', label: '自定义', ua: '' },
]
function pickPreset(v: string) {
  const p = UA_PRESETS.find((x) => x.value === v)
  if (p && v !== 'custom') form.userAgent = p.ua
}
watch(() => form.uaPreset, pickPreset)

const intervals: [string, string][] = [
  ['manual', '仅手动'],
  ['every30_min', '30 分钟'],
  ['hourly', '1 小时'],
  ['every6_hours', '6 小时'],
  ['every12_hours', '12 小时'],
  ['daily', '每天'],
  ['cron', '自定义 Cron'],
]
const intervalLabel = (k: string) => intervals.find(([x]) => x === k)?.[1] ?? k

const statusText: Record<string, string> = {
  idle: '未更新',
  updating: '更新中',
  success: '成功',
  failed: '失败',
}

async function load() {
  try {
    store.subscriptions = await api.subscriptions()
  } catch {}
}

async function add() {
  if (!form.url) { toast('请填写订阅 URL'); return }
  try {
    await api.addSubscription({
      name: form.name || form.url,
      url: form.url,
      interval: form.interval,
      enabled: form.enabled,
      user_agent: form.userAgent || undefined,
    })
    form.name = ''
    form.url = ''
    form.userAgent = ''
    form.uaPreset = 'default'
    toast('订阅已添加')
    // 列表由后端 WS 实时推送，无需主动 load()
  } catch (e: any) { toast(e.message) }
}

async function del(id: string) {
  try {
    await api.deleteSubscription(id)
    delete store.subProgress[id]
    toast('已删除')
    // 列表由后端 WS 实时推送
  } catch (e: any) { toast(e.message) }
}

async function updateNow(id: string) {
  try {
    await api.updateSubscription(id)
    toast('已开始更新（进度见下方）')
  } catch (e: any) { toast(e.message) }
}

async function updateAll() {
  try {
    await api.updateAllSubscriptions()
    toast('已触发全部更新')
  } catch (e: any) { toast(e.message) }
}

async function toggleEnabled(s: any) {
  try {
    await api.updateSubscriptionSettings(s.id, { enabled: !s.enabled })
    // 列表由后端 WS 实时推送
  } catch (e: any) { toast(e.message) }
}

function fmtTime(iso: string | null) {
  if (!iso) return '从未'
  const d = new Date(iso)
  if (isNaN(d.getTime())) return iso
  return d.toLocaleString()
}

load()
</script>

<template>
  <section>
    <h2>订阅管理</h2>
    <div class="form">
      <input v-model="form.name" type="text" placeholder="名称（可选）" />
      <input v-model="form.url" type="text" placeholder="订阅链接 URL" style="min-width: 280px" />
      <select v-model="form.interval">
        <option v-for="[k, l] in intervals" :key="k" :value="k">{{ l }}</option>
      </select>
      <select v-model="form.uaPreset" title="User-Agent 预设">
        <option v-for="p in UA_PRESETS" :key="p.value" :value="p.value">{{ p.label }}</option>
      </select>
      <input
        v-model="form.userAgent"
        type="text"
        :placeholder="form.uaPreset === 'custom' ? '自定义 User-Agent' : '留空使用预设'"
        :disabled="form.uaPreset !== 'custom'"
        style="min-width: 160px"
      />
      <label class="row" style="margin: 0"><input type="checkbox" v-model="form.enabled" /> 启用</label>
      <button class="primary" @click="add">添加订阅</button>
    </div>

    <div class="row" style="margin: 12px 0">
      <button @click="updateAll">全部更新</button>
      <button @click="load">刷新列表</button>
      <span class="hint" style="margin-left: 8px">
        订阅已持久化（重启后台不丢失）；后台每 30s 扫描到期订阅自动更新，失败自动回滚。
      </span>
    </div>

    <ul class="list">
      <li v-for="s in store.subscriptions" :key="s.id">
        <div class="grow">
          <div class="name">
            {{ s.name }}
            <span class="tag">{{ intervalLabel(s.interval) }}</span>
            <span class="tag" :class="{ on: s.enabled }">{{ s.enabled ? '已启用' : '已停用' }}</span>
            <span class="tag" :class="s.last_status">{{ statusText[s.last_status] || s.last_status }}</span>
          </div>
          <div class="meta">{{ s.url }}</div>
          <div class="meta">
            节点数 {{ s.node_count }} · 上次更新 {{ fmtTime(s.last_updated) }}
            <span v-if="s.last_message"> · {{ s.last_message }}</span>
          </div>
          <div
            v-if="store.subProgress[s.id] && store.subProgress[s.id].status === 'updating'"
            class="progress"
          >
            <div class="bar" :style="{ width: store.subProgress[s.id].progress + '%' }"></div>
            <span class="pm">{{ store.subProgress[s.id].message }}</span>
          </div>
        </div>
        <div class="actions">
          <button class="primary" @click="updateNow(s.id)">立即更新</button>
          <button @click="toggleEnabled(s)">{{ s.enabled ? '停用' : '启用' }}</button>
          <button class="danger" @click="del(s.id)">删除</button>
        </div>
      </li>
      <li v-if="!store.subscriptions.length" class="empty">暂无订阅，添加一个开始吧。</li>
    </ul>
  </section>
</template>

<style scoped>
.grow { flex: 1; min-width: 0; }
.actions { display: flex; flex-direction: column; gap: 6px; }
.progress { margin-top: 6px; position: relative; height: 16px; background: #eee; border-radius: 4px; overflow: hidden; }
.bar { height: 100%; background: #3b82f6; transition: width .3s; }
.pm { position: absolute; left: 6px; top: 0; font-size: 11px; line-height: 16px; color: #333; }
.tag.on { background: #dcfce7; color: #166534; }
.tag.success { background: #dcfce7; color: #166534; }
.tag.failed { background: #fee2e2; color: #991b1b; }
.tag.updating { background: #dbeafe; color: #1e40af; }
button.danger { color: #b91c1c; }
</style>
