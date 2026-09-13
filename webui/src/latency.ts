import { store } from './store'

/** 排队时间也计入 UI 等待期限，避免大批量测速尚未完成就提前清掉状态。 */
export function markTesting(nodes: any[]) {
  const concurrency = Math.max(1, Math.min(200, Number(store.config.latency_concurrency) || 1))
  const timeout = Math.max(500, Number(store.config.latency_timeout) || 5000)
  const wait = Math.max(30_000, 3500 + Math.ceil(nodes.length / concurrency) * (timeout + 1500))
  for (const node of nodes) {
    node.testing = true
    if (node._testingTimer) clearTimeout(node._testingTimer)
    node._testingTimer = setTimeout(() => clearTesting([node]), wait)
  }
}

export function clearTesting(nodes: any[]) {
  for (const node of nodes) {
    node.testing = false
    if (node._testingTimer) clearTimeout(node._testingTimer)
    node._testingTimer = null
  }
}
