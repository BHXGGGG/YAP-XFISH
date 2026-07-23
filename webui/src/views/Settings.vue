<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'

const form = reactive({ ...store.config })
watch(() => store.config, (c) => Object.assign(form, c), { deep: true })

async function save() {
  try {
    await api.updateConfig({ ...form })
    toast('设置已保存（修改端口需重启后台）')
  } catch (e: any) { toast(e.message) }
}

const elevating = ref(false)
async function elevate() {
  elevating.value = true
  try {
    await api.adminElevate()
    toast('正在以管理员身份重新启动…')
  } catch (e: any) { toast(e.message) }
  finally { elevating.value = false }
}

const mem = ref<{ working_set_mb: number } | null>(null)
async function refreshMem() {
  try { mem.value = await api.memDebug() } catch (e: any) { toast(e.message) }
}

/* ---------- 实时日志（原仪表盘） ---------- */
const SOURCE_COLOR: Record<string, string> = {
  core:    '#3b82f6',
  sub:     '#22c55e',
  http:    '#6b7280',
  config:  '#a855f7',
  latency: '#f59e0b',
  net:     '#06b6d4',
  app:     '#9ca3af',
}
const filteredLogs = computed(() =>
  store.logFilter === 'all'
    ? store.logs
    : store.logs.filter((l: any) => l.level === store.logFilter)
)
function fmtTime(ts: number) {
  const d = new Date(ts)
  return `${String(d.getHours()).padStart(2,'0')}:${String(d.getMinutes()).padStart(2,'0')}:${String(d.getSeconds()).padStart(2,'0')}`
}
function setFilter(f: 'all' | 'info' | 'warn' | 'error') { store.logFilter = f }
function clearLogs() { store.logs.splice(0, store.logs.length) }
const counts = computed(() => {
  const c = { info: 0, warn: 0, error: 0 }
  for (const l of store.logs) {
    if (l.level === 'info' || l.level === 'warn' || l.level === 'error') {
      c[l.level as 'info' | 'warn' | 'error']++
    }
  }
  return c
})
const autoscroll = ref(true)
const logEl = ref<HTMLElement | null>(null)
watch(filteredLogs, () => {
  if (!autoscroll.value) return
  queueMicrotask(() => {
    if (logEl.value) logEl.value.scrollTop = logEl.value.scrollHeight
  })
})
</script>

<template>
  <section>
    <h2>设置</h2>

    <div class="form2">
      <label>Web 端口<input v-model.number="form.web_port" type="number" /></label>
      <label>Clash API 端口<input v-model.number="form.clash_api_port" type="number" /></label>
      <label>代理端口（SOCKS5/HTTP 入口）<input v-model.number="form.proxy_port" type="number" /></label>
      <p class="hint">代理端口供系统/浏览器「代理设置」中填写（如 <code>127.0.0.1:{{ form.proxy_port }}</code>）。它与上面的 Clash API 端口<strong>不能相同</strong>。修改后保存即生效（运行中会自动重建核心）。</p>
      <label>核心程序路径<input v-model="form.core_binary" type="text" /></label>
      <label>数据目录<input v-model="form.data_dir" type="text" disabled /></label>
      <label class="ck"><input type="checkbox" v-model="form.system_proxy" /> 启用系统代理（指向本机代理端口）</label>
      <label class="ck"><input type="checkbox" v-model="form.enable_tun" /> 启用 TUN（需管理员权限）</label>
      <label class="ck"><input type="checkbox" v-model="form.autostart" /> 开机启动</label>
      <p class="hint">
        系统代理会写入 Windows「Internet 选项」，让系统/浏览器流量走
        <code>127.0.0.1:{{ form.proxy_port }}</code>。关闭程序或取消勾选会自动还原。
        TUN 与系统代理可同时开启，但一般二选一即可。
      </p>
    </div>

    <h3 style="margin-top:24px">延迟测试</h3>
    <div class="form2">
      <label>测试 URL<input v-model="form.latency_test_url" type="text" style="min-width:320px" /></label>
      <label>并发数<input v-model.number="form.latency_concurrency" type="number" min="1" max="200" /></label>
      <label>超时（毫秒）<input v-model.number="form.latency_timeout" type="number" min="500" max="30000" step="500" /></label>
      <p class="hint">
        延迟测试对每个节点发请求到「测试 URL」测量往返时延。socks5/http 节点会以其为代理真实走出口；
        其余类型退化为 TCP 连接探测。并发数控制同时测试的节点数，超时内未响应记为不可达。
        颜色分级：&lt; 200ms 绿，200-500ms 黄，≥ 500ms 红。
      </p>
    </div>
    <div style="margin-top: 18px"><button class="primary" @click="save">保存设置</button></div>

    <h3 style="margin-top:24px">系统</h3>
    <div class="form2">
      <label class="ck">
        <span v-if="store.status.elevated" class="badge on">已以管理员身份运行</span>
        <button v-else :disabled="elevating" @click="elevate">以管理员身份运行（启用 TUN 需要）</button>
      </label>
      <label class="ck">
        <button @click="refreshMem">刷新内存占用</button>
        <span v-if="mem"> 工作集：{{ mem.working_set_mb.toFixed(1) }} MB</span>
      </label>
    </div>

    <!-- 实时日志：从仪表盘迁到这里 -->
    <h3 style="margin-top:24px">实时日志</h3>
    <div class="log-filters">
      <button :class="{ active: store.logFilter === 'all' }" @click="setFilter('all')">
        全部 <span class="badge-mini">{{ store.logs.length }}</span>
      </button>
      <button :class="{ active: store.logFilter === 'info' }" @click="setFilter('info')">
        info <span class="badge-mini">{{ counts.info }}</span>
      </button>
      <button :class="{ active: store.logFilter === 'warn' }" @click="setFilter('warn')">
        warn <span class="badge-mini warn">{{ counts.warn }}</span>
      </button>
      <button :class="{ active: store.logFilter === 'error' }" @click="setFilter('error')">
        error <span class="badge-mini error">{{ counts.error }}</span>
      </button>
      <button :class="{ active: autoscroll }" @click="autoscroll = !autoscroll" title="自动滚到最新一行">
        自动滚动
      </button>
      <button class="clear" @click="clearLogs" title="清空当前日志（不影响后端）">清空</button>
    </div>

    <div v-if="!filteredLogs.length" class="log-empty">暂无日志</div>
    <div v-else class="log" ref="logEl">
      <div v-for="(l, i) in filteredLogs" :key="i"
           class="log-row" :class="['lv-' + (l.level || 'info'), 'src-' + (l.source || 'app')]">
        <span class="ts">{{ fmtTime(l.ts) }}</span>
        <span class="src" :style="{ background: SOURCE_COLOR[l.source] || SOURCE_COLOR.app }">
          {{ l.source || 'app' }}
        </span>
        <span class="lvl">{{ l.level || 'info' }}</span>
        <span class="msg">{{ l.message }}</span>
      </div>
    </div>
  </section>
</template>

<style scoped>
/* 复用仪表盘日志样式，集中到这里 */
.log-filters { display: flex; gap: 6px; align-items: center; margin-bottom: 6px; flex-wrap: wrap; }
.log-filters button { padding: 3px 10px; font-size: 12px; }
.log-filters button.active { background: var(--primary); color: #fff; }
.log-filters button.clear  { color: #b91c1c; }
.badge-mini {
  display: inline-block; min-width: 18px; padding: 0 5px; margin-left: 4px;
  background: #e5e7eb; color: #374151; border-radius: 9px;
  font-size: 11px; text-align: center; vertical-align: middle;
}
.badge-mini.warn  { background: #fef3c7; color: #92400e; }
.badge-mini.error { background: #fee2e2; color: #991b1b; }
.log-empty {
  padding: 16px; color: #9ca3af; text-align: center;
  background: var(--panel); border-radius: var(--radius);
}
.log {
  max-height: 480px; overflow-y: auto;
  background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
}
.log-row {
  display: grid; grid-template-columns: 76px 56px 50px 1fr; gap: 8px;
  padding: 3px 10px; border-bottom: 1px solid #f3f4f6; align-items: baseline;
}
.log-row:hover { background: #f9fafb; }
.ts { color: #9ca3af; }
.src { color: #fff; text-align: center; border-radius: 3px; font-size: 10px; font-weight: 700; letter-spacing: .3px; padding: 1px 0; }
.lvl { font-weight: 700; }
.msg { white-space: pre-wrap; word-break: break-all; }
.lv-info  .lvl { color: #2563eb; }
.lv-warn  { background: #fffbeb; }
.lv-warn  .lvl { color: #d97706; }
.lv-error { background: #fef2f2; }
.lv-error .lvl { color: #dc2626; }
</style>