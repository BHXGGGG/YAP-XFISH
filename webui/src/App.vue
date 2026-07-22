<script setup lang="ts">
import { ref, onMounted } from 'vue'
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

async function refreshAll() {
  try {
    const [s, p, c, subs, rs] = await Promise.all([
      api.status(), api.profile(), api.config(), api.subscriptions(), api.rules(),
    ])
    Object.assign(store.status, s)
    Object.assign(store.profile, p)
    Object.assign(store.config, c)
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
      <div class="brand">Proxy<span>面板</span></div>
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
      <component :is="views[active]" @refresh="refreshAll" />
    </main>
    <div v-if="store.toast" class="toast">{{ store.toast }}</div>
  </div>
</template>
