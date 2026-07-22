<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { store, toast } from './store'
import { api } from './api'
import { connectWs } from './ws'
import Dashboard from './views/Dashboard.vue'
import Profiles from './views/Profiles.vue'
import Subscriptions from './views/Subscriptions.vue'
import Rules from './views/Rules.vue'
import Settings from './views/Settings.vue'

const tabs = [
  { key: 'dashboard', label: '仪表盘', icon: '▦' },
  { key: 'profiles', label: '节点', icon: '⬡' },
  { key: 'subscriptions', label: '订阅', icon: '↻' },
  { key: 'rules', label: '规则', icon: '⚑' },
  { key: 'settings', label: '设置', icon: '⚙' },
]
const views: Record<string, any> = { dashboard: Dashboard, profiles: Profiles, subscriptions: Subscriptions, rules: Rules, settings: Settings }
const active = ref('dashboard')

// 右上角绿色指示：系统代理 / TUN 启用时显示
const indicators = computed(() => {
  const items: { key: string; label: string }[] = []
  if (store.config.system_proxy || store.status.system_proxy) {
    items.push({ key: 'sysproxy', label: '系统代理' })
  }
  if (store.config.enable_tun || store.status.enable_tun) {
    items.push({ key: 'tun', label: 'TUN' })
  }
  return items
})

async function refreshAll() {
  try {
    const [s, p, c, subs, rs] = await Promise.all([
      api.status(), api.profile(), api.config(), api.subscriptions(), api.rules(),
    ])
    Object.assign(store.status, s)
    Object.assign(store.profile, p)
    Object.assign(store.config, c)
    // 与 status 对齐，便于右上角指示灯同时读 config / status
    store.status.system_proxy = !!(s as any).system_proxy || !!(c as any).system_proxy
    store.status.enable_tun = !!(s as any).enable_tun || !!(c as any).enable_tun
    store.subscriptions = subs
    store.rules = rs
  } catch (e: any) {
    toast('加载失败: ' + e.message)
  }
}

onMounted(async () => {
  connectWs()
  await refreshAll()
})
</script>

<template>
  <div class="layout">
    <aside class="side">
      <div class="brand">YAP<span>-XFISH</span></div>
      <nav>
        <button
          v-for="t in tabs"
          :key="t.key"
          :class="{ active: active === t.key }"
          @click="active = t.key"
        >
          <span class="ic">{{ t.icon }}</span>{{ t.label }}
        </button>
      </nav>
      <div class="conn" :class="store.connected ? 'on' : 'off'">
        {{ store.connected ? '● 已连接' : '○ 连接中…' }}
      </div>
    </aside>
    <main class="main">
      <div class="topbar" v-if="indicators.length">
        <span
          v-for="it in indicators"
          :key="it.key"
          class="ind"
          :title="it.label + ' 已启用'"
        >
          <i class="dot"></i>{{ it.label }}
        </span>
      </div>
      <component :is="views[active]" @refresh="refreshAll" />
    </main>
    <div v-if="store.toast" class="toast">{{ store.toast }}</div>
  </div>
</template>

<style scoped>
.topbar {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin: -4px 0 12px;
}
.ind {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 999px;
  background: #ecfdf5;
  color: #166534;
  border: 1px solid #86efac;
  font-size: 12px;
  font-weight: 600;
}
.ind .dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #22c55e;
  box-shadow: 0 0 0 3px rgba(34, 197, 94, 0.2);
  display: inline-block;
}
</style>
