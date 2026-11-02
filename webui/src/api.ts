// 极简 API 客户端：所有请求发往同源（http://127.0.0.1:PORT）的后台。
async function req(method: string, path: string, body?: any) {
  const opts: RequestInit = { method, headers: {} as Record<string, string> }
  if (body !== undefined) {
    opts.headers['Content-Type'] = 'application/json'
    opts.body = JSON.stringify(body)
  }
  const r = await fetch(path, opts)
  if (!r.ok) {
    let msg = r.statusText
    try {
      const t = await r.text()
      if (t) {
        try {
          const detail = JSON.parse(t)
          msg = typeof detail?.error === 'string' ? detail.error : t
        } catch { msg = t }
      }
    } catch {}
    throw new Error(msg)
  }
  if (r.status === 204) return null
  const ct = r.headers.get('content-type') || ''
  return ct.includes('application/json') ? r.json() : r.text()
}

export interface UpdateCheckResult {
  current_version: string
  latest_version: string | null
  status: 'update_available' | 'up_to_date' | 'ahead' | 'no_release'
  release_url: string
}

/** 仪表盘运行信息：核心运行时长、内网 IP、代理端口、内存占用（字节）。 */
export interface DashboardInfo {
  uptime_secs: number | null
  lan_ip: string | null
  proxy_port: number
  app_mem: number
  core_mem: number
}

/** 出口 IP 与 IP 质量（判定规则与 FlClash 一致）。 */
export interface ExitIp {
  ip: string
  country: string | null
  org: string | null
  ip_type: 'residential' | 'mobile' | 'business' | 'hosting' | null
  quality: 'good' | 'normal' | 'risky' | null
  /** 类型只是「未标记为机房 / 移动」推断出来的，可信度较低 */
  inferred: boolean
}

export interface ServiceStatus {
  name: string
  ms: number | null
}

export const api = {
  status: () => req('GET', '/api/status'),
  config: () => req('GET', '/api/config'),
  profile: () => req('GET', '/api/profile'),
  coreStart: () => req('POST', '/api/core/start'),
  coreStop: () => req('POST', '/api/core/stop'),
  coreRestart: () => req('POST', '/api/core/restart'),
  selectNode: (node_id: string) => req('POST', '/api/profile/select', { node_id }),
  setMode: (mode: string) => req('POST', '/api/profile/mode', mode),
  subscriptions: () => req('GET', '/api/subscriptions'),
  addSubscription: (s: any) => req('POST', '/api/subscriptions', s),
  deleteSubscription: (id: string) => req('DELETE', '/api/subscriptions/' + id),
  updateSubscription: (id: string) => req('POST', '/api/subscriptions/' + id + '/update'),
  updateAllSubscriptions: () => req('POST', '/api/subscriptions/update-all'),
  updateSubscriptionSettings: (id: string, s: any) => req('PUT', '/api/subscriptions/' + id, s),
  rules: () => req('GET', '/api/rules'),
  addRule: (r: any) => req('POST', '/api/rules', r),
  deleteRule: (id: string) => req('DELETE', '/api/rules/' + id),
  rulePresets: () => req('GET', '/api/rules/presets'),
  testNodeLatency: (id: string) => req('POST', '/api/nodes/' + id + '/latency'),
  testAllLatency: (node_ids?: string[]) => req('POST', '/api/nodes/latency', { node_ids }),
  /** 设置节点链式前置代理；detour 为空字符串/null 表示清除 */
  setNodeDetour: (id: string, detour: string | null) =>
    req('POST', '/api/nodes/' + id + '/detour', { detour }),
  updateConfig: (c: any) => req('PUT', '/api/config', c),
  adminElevate: () => req('POST', '/api/admin/elevate'),
  memDebug: () => req('GET', '/api/debug/memory'),
  version: () => req('GET', '/api/version'),
  checkUpdate: (): Promise<UpdateCheckResult> => req('GET', '/api/update/check'),
  /** 连接明细：仅打开管理页时按需拉取，后台不常驻轮询 */
  connections: () => req('GET', '/api/connections'),
  closeConnection: (id: string) => req('DELETE', '/api/connections/' + encodeURIComponent(id)),
  closeAllConnections: () => req('DELETE', '/api/connections'),
  /** 30 天每日流量（侧栏统计用） */
  trafficDaily: () => req('GET', '/api/traffic/daily'),
  /** 仪表盘：运行时长 / 内网 IP / 内存（轻量，可轮询） */
  dashboardInfo: (): Promise<DashboardInfo> => req('GET', '/api/dashboard/info'),
  /** 仪表盘：出口 IP + 质量（经本机代理查询，较慢；后端有缓存） */
  exitIp: (refresh = false): Promise<ExitIp | null> =>
    req('GET', '/api/dashboard/exit-ip' + (refresh ? '?refresh=true' : '')),
  /** 仪表盘：经代理访问 Google / GitHub 的延迟 */
  dashboardServices: (): Promise<ServiceStatus[]> => req('GET', '/api/dashboard/services'),
}
