import { store } from './store'

// 连接后台 WebSocket，接收实时事件（状态/流量/日志/订阅进度）并刷新 store。
export function connectWs() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  const ws = new WebSocket(`${proto}://${location.host}/api/ws`)
  ;(ws as any)._store = store
  ws.onopen = () => {
    store.connected = true
  }
  ws.onclose = () => {
    store.connected = false
    setTimeout(connectWs, 2000)
  }
  ws.onmessage = (e) => {
    try {
      const ev = JSON.parse(e.data)
      if (ev.type === 'status') {
        store.status.running = ev.running
        store.status.mode = ev.mode
        store.status.current_node = ev.current_node
      } else if (ev.type === 'traffic') {
        store.status.traffic_up = ev.up
        store.status.traffic_down = ev.down
      } else if (ev.type === 'log') {
        store.logs.unshift({
          ts: Date.now(),
          level: ev.level || 'info',
          source: ev.source || 'app',
          message: ev.message,
        })
        if (store.logs.length > 500) store.logs.pop()
      } else if (ev.type === 'subscription') {
        // 订阅状态由后端 WS 实时推送，避免前端在更新过程中再次 GET /api/subscriptions
        // 拉到的快照早于最终事件，导致列表里仍显示「未更新 / 节点数 0 / 上次更新 从未」。
        store.subProgress[ev.id] = {
          status: ev.status,
          progress: ev.progress,
          message: ev.message,
        }
        const sub = store.subscriptions.find((s: any) => s.id === ev.id)
        if (sub) {
          sub.last_status = ev.status
          sub.last_message = ev.message
          if (ev.node_count !== undefined && ev.node_count !== null) sub.node_count = ev.node_count
          if (ev.last_updated) sub.last_updated = ev.last_updated
        }
      } else if (ev.type === 'subscriptions_refresh') {
        // 后端在订阅「添加 / 删除 / 批量更新」后会广播一次最新列表，避免前端缓存与后端不一致。
        store.subscriptions = ev.subscriptions
      } else if (ev.type === 'latency') {
        const node = store.profile.nodes.find((n: any) => n.id === ev.id)
        if (node) {
          node.latency = ev.latency
          node.latency_status = ev.latency != null ? 'ok' : 'timeout'
          node.testing = false
          if (node._testingTimer) { clearTimeout(node._testingTimer); node._testingTimer = null }
        }
      } else if (ev.type === 'profile') {
        // 配置模型整体更新（节点 / 选中节点 / 规则 / 模式），实时刷新，无需手动刷新页面。
        Object.assign(store.profile, ev.profile)
        store.status.node_count = ev.profile.nodes.length
      }
    } catch {}
  }
}
