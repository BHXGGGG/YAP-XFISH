<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

const emit = defineEmits<{ (e: 'refresh'): void }>()

// 「添加订阅」面板默认收起，点工具栏按钮展开。
const showAdd = ref(false)

// 「添加后立即启用」默认值的规则：
//   - 当前已有至少一个启用订阅 → 默认 false（避免一次性启用多个，新订阅先以停用状态添加）
//   - 当前没有任何启用订阅 → 默认 true（首次接入 / 全部停用后，让新订阅直接生效）
function defaultEnabled(): boolean {
  return !(store.subscriptions || []).some((s: any) => s.enabled)
}

const form = reactive({
  name: '',
  url: '',
  interval: 'every6_hours',
  enabled: defaultEnabled(),
  uaPreset: 'default',
  userAgent: '',
})

// 订阅列表变化（添加/删除/切换启用）时同步表单默认值。
// 防止 UI 上看到「启用」打勾的「幽灵」状态，或下次添加时错误地启用。
watch(
  () => (store.subscriptions || []).map((s: any) => `${s.id}:${s.enabled ? 1 : 0}`).join('|'),
  () => {
    if (!form.url) form.enabled = defaultEnabled()
  },
)

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
]
const intervalLabel = (k: string) => intervals.find(([x]) => x === k)?.[1] ?? k

const statusText: Record<string, string> = {
  idle: '未更新',
  updating: '更新中',
  success: '成功',
  failed: '失败',
}

/* ---------- 顶部概览 ---------- */
const subs = computed<any[]>(() => store.subscriptions || [])
const enabledCount = computed(() => subs.value.filter((s) => s.enabled).length)
const nodeTotal = computed(() => subs.value.reduce((n, s) => n + (Number(s.node_count) || 0), 0))

// 周期 → 毫秒；失败时优先显示后端给出的重试时间。
const INTERVAL_MS: Record<string, number> = {
  every30_min: 30 * 60e3,
  hourly: 3600e3,
  every6_hours: 6 * 3600e3,
  every12_hours: 12 * 3600e3,
  daily: 24 * 3600e3,
}
const nextUpdate = computed(() => {
  const times = subs.value
    .filter((s) => s.enabled && s.interval !== 'manual')
    .map((s) => {
      if (s.next_retry_at) return new Date(s.next_retry_at).getTime()
      return s.last_updated ? new Date(s.last_updated).getTime() + INTERVAL_MS[s.interval] : NaN
    })
    .filter((t) => Number.isFinite(t))
  if (!times.length) return '—'
  return new Date(Math.min(...times)).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
})

async function load() {
  try {
    store.subscriptions = await api.subscriptions()
  } catch {}
}

async function add() {
  if (!form.url) { toast('请填写订阅 URL'); return }
  try {
    // 名称留空交给后端：先用 URL 占位，首次更新时换成订阅响应头里的机场名
    const added = await api.addSubscription({
      name: (form.name || '').trim(),
      url: form.url,
      interval: form.interval,
      enabled: form.enabled,
      user_agent: form.userAgent || undefined,
    })
    await api.updateSubscription(added.id)
    form.name = ''
    form.url = ''
    form.userAgent = ''
    form.uaPreset = 'default'
    showAdd.value = false
    toast('订阅已添加')
    // 列表由后端 WS 实时推送，无需主动 load()
  } catch (e: any) { toast(e.message) }
}

function fmtBytes(n: number) {
  const u = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++ }
  return (i ? n.toFixed(n >= 100 ? 0 : 1) : n) + ' ' + u[i]
}

/** Subscription-Userinfo → 「已用 12.3 GB / 60 GB · 到期 2026-12-07」 */
function usageText(info: any) {
  if (!info) return ''
  const used = (Number(info.upload) || 0) + (Number(info.download) || 0)
  const total = Number(info.total) || 0
  let s = '已用 ' + fmtBytes(used) + (total ? ' / ' + fmtBytes(total) : '')
  s += info.expire ? ' · 到期 ' + new Date(info.expire * 1000).toLocaleDateString() : ' · 长期有效'
  return s
}

function usagePct(info: any) {
  const total = Number(info?.total) || 0
  if (!total) return 0
  return Math.min(100, ((Number(info.upload) || 0) + (Number(info.download) || 0)) / total * 100)
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

async function setIntervalOf(s: any, interval: string) {
  try {
    await api.updateSubscriptionSettings(s.id, { interval })
    toast('更新周期已改为 ' + intervalLabel(interval))
  } catch (e: any) { toast(e.message) }
}

async function toggleEnabled(s: any) {
  try {
    await api.updateSubscriptionSettings(s.id, { enabled: !s.enabled })
    // 列表由后端 WS 实时推送
  } catch (e: any) { toast(e.message) }
}

const renamingId = ref<string | null>(null)
const renameValue = ref('')

function startRename(s: any) {
  renamingId.value = s.id
  renameValue.value = s.name
}

function cancelRename() {
  renamingId.value = null
  renameValue.value = ''
}

async function saveRename(s: any) {
  const name = renameValue.value.trim()
  if (!name) { toast('订阅名称不能为空'); return }
  try {
    await api.updateSubscriptionSettings(s.id, { name })
    cancelRename()
    toast('名称已更新')
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
    <header class="page-head">
      <h2>订阅管理</h2>
      <p class="sub">保存到 data/subscriptions.json · 重启不丢失</p>
    </header>

    <div class="summary">
      <div class="sum">
        <div class="k">订阅总数</div>
        <div class="v">{{ subs.length }}</div>
      </div>
      <div class="sum">
        <div class="k">已启用</div>
        <div class="v green">{{ enabledCount }}</div>
      </div>
      <div class="sum">
        <div class="k">节点总数</div>
        <div class="v">{{ nodeTotal }}</div>
      </div>
      <div class="sum">
        <div class="k">下次自动更新</div>
        <div class="v">{{ nextUpdate }}</div>
      </div>
    </div>

    <div class="toolbar">
      <button class="primary add-toggle" :class="{ open: showAdd }" @click="showAdd = !showAdd">
        <span class="ic">＋</span>添加订阅<span v-if="showAdd" class="chev">▴</span>
      </button>
      <button @click="updateAll">↻ 全部更新</button>
      <button @click="load">⟳ 刷新列表</button>
      <span class="hint">后台每 30 秒扫描到期订阅自动更新，失败自动回滚到上一次可用配置</span>
    </div>

    <div v-show="showAdd" class="add-panel">
      <div class="add-head">
        <span class="add-title">添加订阅</span>
        <span class="add-tip">名称留空时，自动使用机场返回的订阅名（拿不到时取 URL 前 7 个字符）</span>
      </div>

      <div class="add-row">
        <label class="field w-name">
          <span>名称（可选）</span>
          <input v-model="form.name" type="text" placeholder="留空自动获取" />
        </label>
        <label class="field grow">
          <span>订阅链接 URL</span>
          <input v-model="form.url" type="text" placeholder="https://example.com/api/v1/client/subscribe?token=..." />
        </label>
        <label class="field w-int">
          <span>更新周期</span>
          <select v-model="form.interval">
            <option v-for="[k, l] in intervals" :key="k" :value="k">{{ l }}</option>
          </select>
        </label>
        <label class="field w-ua">
          <span>User-Agent 预设</span>
          <select v-model="form.uaPreset">
            <option v-for="p in UA_PRESETS" :key="p.value" :value="p.value">{{ p.label }}</option>
          </select>
        </label>
      </div>

      <div class="add-row">
        <label class="field grow">
          <span>自定义 User-Agent（预设选「自定义」时生效）</span>
          <input
            v-model="form.userAgent"
            type="text"
            :placeholder="form.uaPreset === 'custom' ? '自定义 User-Agent' : '留空则使用预设值'"
            :disabled="form.uaPreset !== 'custom'"
          />
        </label>
        <label class="ck">
          <input type="checkbox" v-model="form.enabled" />
          <span>添加后立即启用</span>
        </label>
        <button @click="showAdd = false">取消</button>
        <button class="primary" @click="add">添加订阅</button>
      </div>
    </div>

    <ul class="list">
      <li v-for="s in store.subscriptions" :key="s.id" :class="{ off: !s.enabled, updating: s.last_status === 'updating' }">
        <div class="sub-head">
          <div class="sub-title">
            <span class="sub-name">{{ s.name }}</span>
            <span class="tag">{{ intervalLabel(s.interval) }}</span>
            <span class="tag" :class="s.enabled ? 'green' : 'gray'">{{ s.enabled ? '已启用' : '已停用' }}</span>
            <span class="tag" :class="s.last_status">{{ statusText[s.last_status] || s.last_status }}</span>
          </div>
          <div class="sub-meta">
            节点数 {{ s.node_count }} · 上次更新 {{ fmtTime(s.last_updated) }}
          </div>
        </div>

        <div v-if="s.userinfo" class="sub-usage">
          <div class="usage-track"><div class="usage-bar" :style="{ width: usagePct(s.userinfo) + '%' }"></div></div>
          <span>{{ usageText(s.userinfo) }}</span>
        </div>

        <div class="sub-url">{{ s.url }}</div>

        <div v-if="s.last_status === 'failed' && s.last_message" class="sub-error">
          更新失败：{{ s.last_message }}
        </div>

        <div
          v-if="store.subProgress[s.id] && store.subProgress[s.id].status === 'updating'"
          class="progress"
        >
          <div class="track">
            <div class="bar" :style="{ width: store.subProgress[s.id].progress + '%' }"></div>
          </div>
          <div class="progress-foot">
            <span>{{ store.subProgress[s.id].message }}</span>
            <span>{{ store.subProgress[s.id].progress }}%</span>
          </div>
        </div>

        <div class="sub-actions">
          <button class="primary" @click="updateNow(s.id)">立即更新</button>
          <select
            class="pill-select"
            :value="s.interval"
            title="自动更新周期"
            aria-label="自动更新周期"
            @change="setIntervalOf(s, ($event.target as HTMLSelectElement).value)"
          >
            <option v-for="[k, l] in intervals" :key="k" :value="k">周期 {{ l }}</option>
          </select>
          <button @click="toggleEnabled(s)">{{ s.enabled ? '停用' : '启用' }}</button>
          <template v-if="renamingId === s.id">
            <input
              v-model="renameValue"
              class="rename-input"
              type="text"
              aria-label="订阅名称"
              @keyup.enter="saveRename(s)"
              @keyup.escape="cancelRename"
            />
            <button class="primary" @click="saveRename(s)">保存</button>
            <button @click="cancelRename">取消</button>
          </template>
          <button v-else @click="startRename(s)">重命名</button>
          <button class="danger del" @click="del(s.id)">删除</button>
        </div>
      </li>
      <li v-if="!store.subscriptions.length" class="empty">暂无订阅，添加一个开始吧。</li>
    </ul>
  </section>
</template>

<style scoped>
.page-head { margin-bottom: 16px; }
.page-head h2 { margin: 0; font-size: 19px; }
.page-head .sub { margin: 4px 0 0; color: var(--muted); font-size: 12px; }

.summary { display: grid; grid-template-columns: repeat(4, 1fr); gap: 12px; margin-bottom: 14px; }
.sum { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 12px 14px; }
.sum .k { color: var(--muted); font-size: 12px; margin-bottom: 6px; }
.sum .v { font-size: 20px; font-weight: 600; }
.sum .v.green { color: var(--green); }

.toolbar { display: flex; align-items: center; gap: 8px; margin-bottom: 12px; }
.toolbar .hint { margin: 0 0 0 4px; }
.add-toggle { display: flex; align-items: center; gap: 6px; }
.add-toggle .ic { font-size: 13px; }
.add-toggle.open { background: var(--primary-d); border-color: var(--primary-d); }
.add-toggle .chev { font-size: 10px; margin-left: 2px; }

.add-panel {
  display: flex; flex-direction: column; gap: 12px;
  background: var(--panel); border: 1px solid #bcdcff; border-radius: var(--radius);
  padding: 16px; margin-bottom: 12px;
}
.add-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.add-title { font-weight: 700; font-size: 14px; }
.add-tip { color: var(--muted); font-size: 11px; }
.add-row { display: flex; align-items: flex-end; gap: 10px; }
.field { display: flex; flex-direction: column; gap: 6px; }
.field > span { color: var(--muted); font-size: 12px; }
.field input, .field select {
  border: 1px solid var(--border); border-radius: 8px; padding: 8px 10px;
  font-size: 13px; font-family: inherit; background: #fff; color: var(--text); width: 100%;
}
.field input:disabled { background: #fafbfc; color: var(--muted); }
.w-name { width: 165px; }
.w-int { width: 140px; }
.w-ua { width: 200px; }
.grow { flex: 1; min-width: 0; }
.ck { display: flex; align-items: center; gap: 7px; font-size: 13px; padding-bottom: 9px; }
.ck input { width: 15px; height: 15px; accent-color: var(--primary); }

.list li { display: flex; flex-direction: column; align-items: stretch; gap: 8px; }
.list li.off { background: #fbfbfc; }
.list li.updating { border-color: #bcdcff; }
.sub-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.sub-title { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; min-width: 0; }
.sub-name { font-weight: 600; font-size: 14px; }
.sub-meta { color: var(--muted); font-size: 12px; white-space: nowrap; }
.sub-url {
  color: var(--muted); font-size: 12px;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.sub-error { color: var(--red); font-size: 12px; }
.sub-usage { display: flex; align-items: center; gap: 10px; color: var(--muted); font-size: 12px; }
.usage-track { width: 160px; height: 6px; background: #eef1f4; border-radius: 999px; overflow: hidden; flex-shrink: 0; }
.usage-bar { height: 100%; background: var(--green); }

.tag.green { background: var(--green-bg); color: var(--green); }
.tag.gray { background: #f0f2f4; color: var(--muted); }
.tag.success { background: var(--green-bg); color: var(--green); }
.tag.failed { background: var(--red-bg); color: var(--red); }
.tag.updating { background: #dbeafe; color: #1e40af; }

.progress { display: flex; flex-direction: column; gap: 4px; }
.track { height: 6px; background: #eef1f4; border-radius: 999px; overflow: hidden; }
.track .bar { height: 100%; background: var(--primary); transition: width .3s; }
.progress-foot { display: flex; justify-content: space-between; color: var(--muted); font-size: 11px; }

.sub-actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin-top: 2px; }
.sub-actions button { padding: 6px 12px; font-size: 13px; }
.pill-select { border: 1px solid var(--border); border-radius: 8px; padding: 6px 8px; font-size: 13px; background: #fff; color: var(--text); }
.rename-input { width: 140px; padding: 5px 8px; border: 1px solid var(--border); border-radius: 8px; font-size: 13px; }
.del { margin-left: auto; color: var(--red); }
.del:hover { border-color: var(--red); color: var(--red); }
</style>
