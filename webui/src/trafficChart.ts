export const TRAFFIC_WINDOW_MS = 10 * 60 * 1000

export type TrafficSample = { at: number; up: number; down: number }

export function appendTrafficSample(history: TrafficSample[], sample: TrafficSample): TrafficSample[] {
  return [...history.filter(point => point.at >= sample.at - TRAFFIC_WINDOW_MS), sample]
}

// 两条曲线共用时间轴与速率刻度，浏览器暂停采样时留下空隙。
export function trafficChart(history: TrafficSample[], now: number) {
  const start = now - TRAFFIC_WINDOW_MS
  const points = history.filter(point => point.at >= start && point.at <= now)
  const max = Math.max(1024, ...points.flatMap(point => [point.up, point.down]))
  function path(key: 'up' | 'down') {
    return points.map((point, i) => {
      const command = i === 0 || point.at - points[i - 1].at > 5000 ? 'M' : 'L'
      return `${command}${((point.at - start) / TRAFFIC_WINDOW_MS * 600).toFixed(1)},${(100 - point[key] / max * 100).toFixed(1)}`
    }).join(' ')
  }
  return { max, up: path('up'), down: path('down') }
}
