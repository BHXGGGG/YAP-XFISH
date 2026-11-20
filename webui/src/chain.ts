export type ChainNode = { id: string; name: string; type?: string; kind?: string; detour?: string | null }
export type ChainEdge = { from: string; to: string }

export function chainPath(ids: string[], edges: ChainEdge[]): string[] {
  if (!ids.length) return []
  const incoming = new Map<string, string>()
  const outgoing = new Map<string, string>()
  for (const { from, to } of edges) {
    if (!ids.includes(from) || !ids.includes(to)) throw new Error('连线包含不存在的节点')
    if (from === to || incoming.has(to) || outgoing.has(from)) throw new Error('代理链必须是单线路径，不能分叉或连接自身')
    incoming.set(to, from)
    outgoing.set(from, to)
  }
  const starts = ids.filter(id => !incoming.has(id))
  if (starts.length !== 1) throw new Error('请将所有节点连接成一条路径')
  const path: string[] = []
  let id: string | undefined = starts[0]
  while (id) {
    if (path.includes(id)) throw new Error('代理链不能形成循环')
    path.push(id)
    id = outgoing.get(id)
  }
  if (path.length !== ids.length) throw new Error('请将所有节点连接成一条路径，不能形成循环')
  return path
}

export function chainChanges(original: string[], path: string[], nodes: ChainNode[]) {
  if (path.length < 2) throw new Error('至少选择两个节点')
  if (path.length > 32) throw new Error('代理链最多支持 32 跳')
  for (const [index, id] of path.entries()) {
    const node = nodes.find(node => node.id === id)
    if (!node) throw new Error('节点已不存在，请重新选择')
    if (index && ['hysteria', 'hysteria2', 'hy2', 'tuic', 'wireguard', 'wg'].includes((node.type || node.kind || '').toLowerCase())) {
      throw new Error(`「${node.name}」是 UDP/QUIC 节点，只能放在第一跳`)
    }
  }
  return [...new Set([...original, ...path])].map(id => {
    const index = path.indexOf(id)
    return { id, detour: index > 0 ? path[index - 1] : null }
  })
}

export function existingChain(exitId: string, nodes: ChainNode[]): string[] {
  const path: string[] = []
  let id: string | null | undefined = exitId
  while (id) {
    if (path.includes(id)) throw new Error('已有链路存在循环，请先清除对应前置')
    const node = nodes.find(node => node.id === id)
    if (!node) throw new Error('已有链路的前置节点不可用，请先启用对应订阅或清除前置')
    path.unshift(id)
    id = node.detour
  }
  return path
}
