<script setup lang="ts">
import { reactive, watch } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

const emit = defineEmits<{
  (e: 'added', payload: { id: string; name: string }): void
  (e: 'cancel'): void
}>()

const form = reactive({
  name: '',
  url: '',
  interval: 'every6_hours',
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

function reset() {
  form.name = ''
  form.url = ''
  form.userAgent = ''
  form.uaPreset = 'default'
  form.interval = 'every6_hours'
  form.enabled = true
}

async function submit() {
  if (!form.url) { toast('请填写订阅 URL'); return }
  try {
    const name = (form.name || '').trim() || defaultName(form.url)
    const added = await api.addSubscription({
      name,
      url: form.url,
      interval: form.interval,
      enabled: form.enabled,
      user_agent: form.userAgent || undefined,
    })
    // 保持和“订阅”页一致：添加后立即更新一次
    await api.updateSubscription(added.id)
    toast('订阅已添加')
    emit('added', { id: added.id, name: added.name })
    reset()
  } catch (e: any) { toast(e.message) }
}

function defaultName(url: string): string {
  const rest = (url || '').trim().replace(/^https?:\/\//i, '').replace(/^\/+/, '')
  const name = [...rest].slice(0, 7).join('')
  return name || '订阅'
}

function cancel() {
  reset()
  emit('cancel')
}
</script>

<template>
  <form class="add-sub-form" @submit.prevent="submit">
    <p class="add-intro">
      粘贴一个订阅链接（Clash / sing-box / v2rayN 等格式均可），点「添加订阅」后会立刻下载一次。
    </p>

    <div class="form-grid">
      <label class="row">
        <span>订阅链接 URL</span>
        <input v-model="form.url" type="text" placeholder="https://example.com/sub" required class="field" />
      </label>

      <label class="row">
        <span>订阅名称（可选）</span>
        <input v-model="form.name" type="text" placeholder="留空按 URL 自动生成" class="field" />
      </label>

      <div class="row two-col">
        <label class="row tight">
          <span>更新周期</span>
          <select v-model="form.interval" class="field">
            <option v-for="[k, l] in intervals" :key="k" :value="k">{{ l }}</option>
          </select>
        </label>
        <label class="row tight">
          <span>User-Agent</span>
          <select v-model="form.uaPreset" class="field" title="User-Agent 预设">
            <option v-for="p in UA_PRESETS" :key="p.value" :value="p.value">{{ p.label }}</option>
          </select>
        </label>
      </div>

      <label v-if="form.uaPreset === 'custom'" class="row">
        <span>自定义 UA</span>
        <input v-model="form.userAgent" type="text" placeholder="自定义 User-Agent" class="field" />
      </label>

      <label class="row inline">
        <input id="add-sub-enabled" type="checkbox" v-model="form.enabled" />
        <span>添加后立即启用</span>
      </label>
    </div>

    <div class="actions">
      <button type="button" class="ghost" @click="cancel">取消</button>
      <button type="submit" class="primary">添加订阅</button>
    </div>
  </form>
</template>

<style scoped>
.add-sub-form { display: flex; flex-direction: column; gap: 14px; }
.add-intro {
  margin: 0;
  padding: 10px 12px;
  background: linear-gradient(180deg, #eff6ff 0%, #f5f3ff 100%);
  border: 1px solid #e0e7ff;
  border-radius: 10px;
  color: #4338ca;
  font-size: 12.5px;
  line-height: 1.55;
}
.form-grid { display: flex; flex-direction: column; gap: 12px; }
.row { display: flex; flex-direction: column; gap: 6px; font-size: 12.5px; color: #4b5563; }
.row.tight { gap: 4px; }
.row.inline { flex-direction: row; align-items: center; gap: 8px; }
.row > span { font-weight: 600; color: #334155; letter-spacing: .2px; }
.two-col { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
.field {
  width: 100%;
  padding: 8px 10px;
  font: inherit;
  font-size: 13px;
  color: #0f172a;
  background: #fff;
  border: 1px solid #d1d5db;
  border-radius: 8px;
  transition: border-color .15s, box-shadow .15s, background .15s;
}
.field::placeholder { color: #9ca3af; }
.field:hover  { border-color: #93c5fd; }
.field:focus  {
  outline: none;
  border-color: #3b82f6;
  background: #fff;
  box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.18);
}
select.field { appearance: none; padding-right: 28px;
  background-image: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12'><path fill='%236b7280' d='M2 4l4 4 4-4z'/></svg>");
  background-repeat: no-repeat; background-position: right 8px center;
}
.row.inline input[type="checkbox"] {
  width: 16px; height: 16px; accent-color: #3b82f6;
}
.actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 4px; }
.actions .primary {
  background: linear-gradient(180deg, #3b82f6 0%, #2563eb 100%);
  border: 1px solid #1d4ed8;
  color: #fff;
  padding: 8px 18px;
  border-radius: 8px;
  font-weight: 600;
  box-shadow: 0 4px 12px rgba(37, 99, 235, 0.25);
  transition: transform .12s, box-shadow .12s;
}
.actions .primary:hover { transform: translateY(-1px); box-shadow: 0 6px 16px rgba(37, 99, 235, 0.32); }
.actions .primary:active { transform: translateY(0); }
.actions .ghost {
  background: #fff; color: #475569;
  border: 1px solid #d1d5db;
  padding: 8px 16px; border-radius: 8px; font-weight: 600;
}
.actions .ghost:hover { background: #f8fafc; border-color: #94a3b8; }
@media (max-width: 480px) {
  .two-col { grid-template-columns: 1fr; }
}
</style>
