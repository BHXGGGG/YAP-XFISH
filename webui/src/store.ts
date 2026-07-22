import { reactive } from 'vue'

// 全局响应式状态，由 API 拉取 + WebSocket 事件更新。浏览器关闭后后台不受影响。
export const store = reactive({
  connected: false,
  status: {
    running: false,
    mode: 'global',
    current_node: null as string | null,
    traffic_up: 0,
    traffic_down: 0,
    node_count: 0,
    elevated: false,
    mem_mb: 0,
  },
  profile: {
    nodes: [] as any[],
    selected_node: null as string | null,
    rules: [] as any[],
    mode: 'global',
  },
  config: {
    web_port: 9527,
    core_binary: '',
    data_dir: '',
    clash_api_port: 9090,
    proxy_port: 7890,
    api_secret: '',
    enable_tun: false,
    autostart: false,
    latency_test_url: 'https://www.gstatic.com/generate_204',
    latency_concurrency: 50,
    latency_timeout: 5000,
  },
  subscriptions: [] as any[],
  rules: [] as any[],
  // 日志条目结构: { ts, level, source, message }；上限 500，超过自动丢弃最旧。
  logs: [] as { ts: number; level: string; source: string; message: string }[],
  toast: '' as string,
  // 订阅更新进度（由 WebSocket 的 subscription 事件实时写入）
  subProgress: {} as Record<string, { status: string; progress: number; message: string }>,
  // 日志过滤（'all' | 'info' | 'warn' | 'error'）
  logFilter: 'all' as 'all' | 'info' | 'warn' | 'error',
})

let toastTimer: any = null
export function toast(msg: string) {
  store.toast = msg
  if (toastTimer) clearTimeout(toastTimer)
  toastTimer = setTimeout(() => (store.toast = ''), 2500)
}
