<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { store, toast } from '../store'
import { api } from '../api'
import Modal from '../components/Modal.vue'

const form = reactive({ ...store.config })
watch(() => store.config, (c) => Object.assign(form, c), { deep: true })

async function save() {
  // TUN 从关→开 且 未提权 → 弹提权引导
  if (form.enable_tun && !store.config.enable_tun && !store.status.elevated) {
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

/* ---------- 实时日志（原仪表盘，现已迁出到 Logs.vue） ---------- */
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

    <!-- 实时日志已迁出到 Logs.vue（左侧“日志”导航） -->

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
/* 实时日志样式已迁出到 Logs.vue */

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