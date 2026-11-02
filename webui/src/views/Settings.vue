<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue'
import { store, toast } from '../store'
import { api, type UpdateCheckResult, type UpdateProgress } from '../api'
import Modal from '../components/Modal.vue'

const form = reactive({ ...store.config })
watch(() => store.config, (c) => Object.assign(form, c), { deep: true })

async function save() {
  // TUN 从关→开 且 未提权 → 先持久化 enable_tun=true，再弹提权引导。
  // 写盘不需要管理员权限；提权后的新实例读到 enable_tun=true 即可自动启用 TUN。
  if (form.enable_tun && !store.config.enable_tun && !store.status.elevated) {
    try {
      const updated = await api.updateConfig({ ...form, enable_tun: true })
      Object.assign(store.config, updated)
    } catch (e: any) {
      toast(e.message || '保存设置失败，请重试')
      return
    }
    showElevateDialog.value = true
    return
  }
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

const releasesUrl = 'https://github.com/BHXGGGG/YAP-XFISH/releases'
const checkingUpdate = ref(false)
const updateResult = ref<UpdateCheckResult | null>(null)
const updateError = ref('')
const updateMessage = computed(() => {
  if (checkingUpdate.value) return '正在检查 GitHub 最新正式版…'
  if (updateError.value) {
    return (updateResult.value ? '更新失败：' : '检查更新失败：') + updateError.value
  }
  const result = updateResult.value
  if (!result) return ''
  switch (result.status) {
    case 'update_available': return `发现新版本 v${result.latest_version}（当前 v${result.current_version}）`
    case 'up_to_date': return `当前已是最新版本 v${result.current_version}`
    case 'ahead': return `当前版本 v${result.current_version} 高于 GitHub 最新正式版 v${result.latest_version}`
    case 'no_release': return `GitHub 暂无正式发布版本（当前 v${result.current_version}）`
  }
})

async function checkForUpdates() {
  if (checkingUpdate.value || updating.value) return
  checkingUpdate.value = true
  updateResult.value = null
  updateError.value = ''
  try {
    updateResult.value = await api.checkUpdate()
    store.appVersion = updateResult.value.current_version
  } catch (e: any) {
    updateError.value = e.message || '无法获取版本信息，请稍后重试'
  } finally {
    checkingUpdate.value = false
  }
}

/* ---------- 一键更新：下载 → 校验 → 重启安装 ---------- */
const updating = ref(false)
const updateProgress = ref<UpdateProgress | null>(null)
let updateTimer: number | null = null

function stopUpdatePolling() {
  if (updateTimer) {
    window.clearInterval(updateTimer)
    updateTimer = null
  }
}

function fmtSize(n: number): string {
  if (!n) return '0 B'
  if (n >= 1024 * 1024 * 1024) return (n / 1024 / 1024 / 1024).toFixed(1) + ' GB'
  if (n >= 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + ' MB'
  if (n >= 1024) return (n / 1024).toFixed(0) + ' KB'
  return n + ' B'
}

const autoUpdateSize = computed(() => {
  const r = updateResult.value
  if (!r || r.status !== 'update_available') return 0
  return r.update_asset?.size || r.full_asset?.size || 0
})
const canAutoUpdate = computed(() =>
  !!updateResult.value && updateResult.value.status === 'update_available' && autoUpdateSize.value > 0,
)
const updatePct = computed(() => {
  const p = updateProgress.value
  if (!p || !p.total) return 0
  return Math.min(100, Math.round((p.downloaded / p.total) * 100))
})
const updateProgressText = computed(() => {
  const p = updateProgress.value
  if (!p) return ''
  switch (p.state) {
    case 'downloading':
      return p.total
        ? `正在下载 ${fmtSize(p.downloaded)} / ${fmtSize(p.total)}`
        : `正在下载 ${fmtSize(p.downloaded)}`
    case 'extracting':
      return '正在校验并解压…'
    case 'ready':
      return '下载完成，正在重启安装…'
    case 'installing':
      return '正在安装，程序即将重启…'
    default:
      return ''
  }
})

async function startUpdate() {
  if (updating.value || !canAutoUpdate.value) return
  updating.value = true
  updateError.value = ''
  updateProgress.value = null
  try {
    await api.updateApply()
    stopUpdatePolling()
    updateTimer = window.setInterval(pollUpdate, 500)
  } catch (e: any) {
    updating.value = false
    updateError.value = e.message || '无法开始更新'
  }
}

async function pollUpdate() {
  try {
    const p = await api.updateProgress()
    updateProgress.value = p
    if (p.state === 'error') {
      stopUpdatePolling()
      updating.value = false
      updateError.value = p.error || '更新失败'
      return
    }
    if (p.state === 'ready') {
      stopUpdatePolling()
      try {
        await api.updateInstall()
        // 程序重启后回到管理面板，显示新版本号
        window.setTimeout(() => location.reload(), 8000)
      } catch (e: any) {
        updating.value = false
        updateError.value = e.message || '启动安装失败'
      }
    }
  } catch {
    // 安装时进程退出会让请求失败，交给下一次轮询或重启判断
  }
}

onBeforeUnmount(stopUpdatePolling)

/* ---------- TUN 提权引导 ---------- */
const showElevateDialog = ref(false)
function onElevateConfirm() {
  showElevateDialog.value = false
  api.adminElevate().then(() => {
    toast('正在以管理员身份重新启动…')
  }).catch((e: any) => {
    toast('提权失败: ' + e.message)
  })
}
function onElevateCancel() {
  showElevateDialog.value = false
}

</script>

<template>
  <section class="settings-page">
    <header class="settings-header">
      <h2>设置</h2>
      <div class="settings-actions">
        <a
          class="github-button"
          href="https://github.com/BHXGGGG/YAP-XFISH"
          target="_blank"
          rel="noopener noreferrer"
          title="在新标签页打开 YAP-XFISH 开源仓库"
        >
          <svg width="18" height="18" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" focusable="false">
            <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.65 7.65 0 0 1 4 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z" />
          </svg>
          GitHub
        </a>
        <button :disabled="checkingUpdate || updating" :aria-busy="checkingUpdate" @click="checkForUpdates">
          {{ checkingUpdate ? '检查中…' : '检查更新' }}
        </button>
        <button class="primary" @click="save">保存设置</button>
      </div>
    </header>

    <div
      v-if="checkingUpdate || updateResult || updateError || updateProgress"
      class="update-status"
      :class="{ 'has-update': updateResult?.status === 'update_available', 'is-error': !!updateError }"
      role="status"
      aria-live="polite"
      aria-atomic="true"
    >
      <span>{{ updateMessage }}</span>
      <span v-if="updateProgressText" class="update-progress-text">{{ updateProgressText }}</span>
      <span
        v-if="updateProgress && (updateProgress.state === 'downloading' || updateProgress.state === 'extracting')"
        class="update-bar"
        role="progressbar"
        :aria-valuenow="updatePct"
        aria-valuemin="0"
        aria-valuemax="100"
      ><i :style="{ width: updatePct + '%' }"></i></span>
      <button v-if="updating" class="update-btn" disabled>更新中…</button>
      <button v-else-if="canAutoUpdate" class="update-btn" @click="startUpdate">
        立即更新（{{ fmtSize(autoUpdateSize) }}）
      </button>
      <a
        v-if="!updating && !checkingUpdate && (updateResult?.status === 'update_available' || updateResult?.status === 'no_release' || updateError)"
        :href="updateResult?.release_url || releasesUrl"
        target="_blank"
        rel="noopener noreferrer"
      >{{ updateResult?.status === 'update_available' ? '前往下载' : '查看发布页' }}</a>
    </div>

    <div class="settings-grid">
      <div class="settings-column">
        <section class="settings-card" aria-labelledby="network-settings-title">
          <h3 id="network-settings-title">网络与核心</h3>
          <div class="form2 settings-form">
            <div class="settings-fields">
              <label>Web 端口<input v-model.number="form.web_port" type="number" /></label>
              <label>Clash API 端口<input v-model.number="form.clash_api_port" type="number" /></label>
            </div>
            <label>代理端口（SOCKS5/HTTP 入口）<input v-model.number="form.proxy_port" type="number" /></label>
            <p class="hint">代理端口供系统/浏览器「代理设置」中填写（如 <code>127.0.0.1:{{ form.proxy_port }}</code>）。它与上面的 Clash API 端口<strong>不能相同</strong>。修改后保存即生效（运行中会自动重建核心）。</p>
            <label>核心程序路径<input v-model="form.core_binary" type="text" /></label>
            <label>数据目录<input v-model="form.data_dir" type="text" disabled /></label>
          </div>
        </section>

        <section class="settings-card" aria-labelledby="proxy-settings-title">
          <h3 id="proxy-settings-title">代理与启动</h3>
          <div class="form2 settings-form">
            <label class="ck"><input type="checkbox" v-model="form.system_proxy" /> 启用系统代理（指向本机代理端口）</label>
            <label class="ck"><input type="checkbox" v-model="form.enable_tun" /> 启用 TUN（需管理员权限）</label>
            <label class="ck"><input type="checkbox" v-model="form.autostart" /> 开机启动</label>
            <p class="hint">
              系统代理会写入 Windows「Internet 选项」，让系统/浏览器流量走
              <code>127.0.0.1:{{ form.proxy_port }}</code>。关闭程序或取消勾选会自动还原。
              TUN 与系统代理可同时开启，但一般二选一即可。
            </p>
          </div>
        </section>
      </div>

      <div class="settings-column">
        <section class="settings-card" aria-labelledby="tun-settings-title">
          <h3 id="tun-settings-title">TUN 性能</h3>
          <div class="form2 settings-form">
            <div class="settings-fields tun-fields">
              <label>协议栈
                <select v-model="form.tun_stack">
                  <option value="gvisor">gvisor（默认：连通性最稳）</option>
                  <option value="mixed">mixed（TCP 系统栈，部分环境更快，也可能上不了网）</option>
                  <option value="system">system（纯系统栈）</option>
                </select>
              </label>
              <label>MTU<input v-model.number="form.tun_mtu" type="number" min="1280" max="65535" step="1" /></label>
            </div>
            <p class="hint">
              默认 <code>gvisor</code> + MTU <code>1500</code> 优先保证能上网。
              若确认连通正常还想冲吞吐，可再试 <code>mixed</code>/<code>system</code> 或把 MTU 调到 <code>9000</code>。
              保存后会重建核心配置。日志默认写在数据目录 <code>logs/</code>。
            </p>
          </div>
        </section>

        <section class="settings-card" aria-labelledby="latency-settings-title">
          <h3 id="latency-settings-title">延迟测试</h3>
          <div class="form2 settings-form">
            <label>测试 URL<input v-model="form.latency_test_url" type="text" /></label>
            <div class="settings-fields">
              <label>并发数<input v-model.number="form.latency_concurrency" type="number" min="1" max="200" /></label>
              <label>超时（毫秒）<input v-model.number="form.latency_timeout" type="number" min="500" max="30000" step="500" /></label>
            </div>
            <p class="hint">
              延迟测试通过节点访问「测试 URL」，完成一个就显示一个结果。默认并发 50，可设置为 1–200；
              调低超时可更快结束不可达节点的等待。链式节点按完整链路测速。
              颜色分级：&lt; 200ms 绿，200-500ms 黄，≥ 500ms 红。
            </p>
          </div>
        </section>

        <section class="settings-card" aria-labelledby="system-settings-title">
          <h3 id="system-settings-title">系统</h3>
          <div class="system-actions">
            <div class="system-row">
              <span v-if="store.status.elevated" class="badge on">已以管理员身份运行</span>
              <button v-else :disabled="elevating" @click="elevate">以管理员身份运行（启用 TUN 需要）</button>
            </div>
            <div class="system-row">
              <button @click="refreshMem">刷新内存占用</button>
              <span v-if="mem" class="memory-value">工作集：{{ mem.working_set_mb.toFixed(1) }} MB</span>
            </div>
          </div>
        </section>
      </div>
    </div>

    <Modal :open="showElevateDialog" title="需要管理员权限" subtitle="TUN 模式需要管理员权限才能创建虚拟网卡" icon="⚡" :width="440" @close="onElevateCancel">
      <div class="elevate-body">
        <p>当前未以管理员身份运行，启用 TUN 后 sing-box 无法创建虚拟网卡。</p>
        <p>是否立即<strong>以管理员身份重新启动</strong> YAP-XFISH？重启后自动启用 TUN。</p>
        <div class="elevate-actions">
          <button class="primary elevate-btn" @click="onElevateConfirm">以管理员身份运行</button>
          <button @click="onElevateCancel">取消</button>
        </div>
      </div>
    </Modal>
  </section>
</template>

<style scoped>
.settings-page { min-width: 0; }
.settings-header {
  position: sticky;
  top: 0;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 14px;
  padding-bottom: 12px;
  margin-bottom: 8px;
  background: var(--bg);
}
.settings-header h2 { margin: 0; }
.settings-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 10px; }
.github-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 8px 16px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel);
  color: var(--text);
  text-decoration: none;
  line-height: 20px;
  transition: .15s;
}
.github-button:hover { border-color: var(--primary); color: var(--primary); }
.github-button:focus-visible { outline: 2px solid var(--primary); outline-offset: 3px; }
.github-button svg { flex-shrink: 0; }
.update-status {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 10px 16px;
  padding: 12px 16px;
  margin-bottom: 20px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--panel);
  line-height: 1.6;
  overflow-wrap: anywhere;
}
.update-status.has-update { border-color: #86efac; background: #ecfdf5; color: #166534; }
.update-status.is-error { border-color: #f0c1bc; background: var(--red-bg); color: var(--red); }
.update-status a { color: var(--primary-d); font-weight: 600; text-underline-offset: 3px; }
.update-status a:focus-visible { outline: 2px solid var(--primary); outline-offset: 3px; }
.update-progress-text { color: var(--muted); font-size: 13px; font-variant-numeric: tabular-nums; }
.update-bar { flex: 1 1 100%; height: 6px; border-radius: 999px; background: #e5e7eb; overflow: hidden; }
.update-bar i { display: block; height: 100%; background: var(--primary); transition: width .2s; }
.update-btn {
  padding: 6px 14px;
  border: 1px solid var(--primary);
  border-radius: 8px;
  background: var(--primary);
  color: #fff;
  font-weight: 600;
  cursor: pointer;
}
.update-btn:hover:not(:disabled) { background: var(--primary-d); border-color: var(--primary-d); }
.update-btn:disabled { opacity: .6; cursor: default; }
.settings-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 20px; align-items: start; }
.settings-column { display: flex; flex-direction: column; gap: 20px; min-width: 0; }
.settings-card {
  min-width: 0;
  padding: 20px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--panel);
}
.settings-card h3 { margin: 0 0 16px; color: var(--text); }
.settings-form { max-width: none; }
.settings-fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px; }
.tun-fields { grid-template-columns: minmax(0, 2fr) minmax(0, 1fr); }
.settings-form label { min-width: 0; }
.settings-form input:not([type="checkbox"]), .settings-form select {
  width: 100%;
  min-width: 0;
  padding: 8px 12px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel);
  color: var(--text);
  font-size: 14px;
  line-height: 20px;
}
.settings-form input:disabled { background: var(--bg); color: var(--muted); }
.settings-form input:focus-visible, .settings-form select:focus-visible { outline: 2px solid var(--primary); outline-offset: 2px; }
.settings-form input[type="checkbox"] { flex-shrink: 0; accent-color: var(--primary); }
.settings-form .hint { margin: 0; line-height: 1.7; overflow-wrap: anywhere; }
.system-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; }
.system-row { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; }
.memory-value { color: var(--muted); font-size: 13px; }

@media (max-width: 1100px) {
  .settings-grid { grid-template-columns: minmax(0, 1fr); }
}
@media (max-width: 640px) {
  .settings-card { padding: 16px; }
  .settings-fields { grid-template-columns: minmax(0, 1fr); }
}

.elevate-body { padding: 4px 0; }
.elevate-body p { margin: 8px 0; line-height: 1.6; color: #374151; font-size: 14px; }
.elevate-actions { display: flex; gap: 10px; margin-top: 18px; justify-content: flex-end; }
.elevate-btn {
  background: linear-gradient(180deg, #3b82f6 0%, #2563eb 100%);
  border: 1px solid #1d4ed8; color: #fff; padding: 8px 18px;
  border-radius: 8px; font-weight: 600;
  box-shadow: 0 4px 12px rgba(37, 99, 235, 0.25);
}
</style>
