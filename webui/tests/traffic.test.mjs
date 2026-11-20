import assert from 'node:assert/strict'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'

test('dashboard traffic uses a shared scale and a rolling ten-minute window', async (t) => {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    server: { middlewareMode: true, ws: false },
    appType: 'custom',
  })
  const originalLocation = globalThis.location
  const originalWebSocket = globalThis.WebSocket
  try {
    const { appendTrafficSample, trafficChart } = await server.ssrLoadModule('/src/trafficChart.ts')
    await t.test('constant and zero rates still advance by time and expire', () => {
      let history = []
      for (let at = 0; at <= 601_000; at += 1000) {
        history = appendTrafficSample(history, { at, up: 0, down: 2048 })
      }
      assert.equal(history.length, 601)
      assert.equal(history[0].at, 1000)
      const chart = trafficChart(history, 601_000)
      assert.equal(chart.max, 2048)
      assert.ok(chart.up.startsWith('M0.0,100.0 L1.0,100.0'))
      assert.ok(chart.down.endsWith('L600.0,0.0'))
    })
    await t.test('both directions share the scale; sampling gaps and empty history stay honest', () => {
      const chart = trafficChart([
        { at: 600_000, up: 1024, down: 2048 },
        { at: 601_000, up: 0, down: 0 },
        { at: 610_000, up: 0, down: 0 },
      ], 610_000)
      assert.equal(chart.up, 'M590.0,50.0 L591.0,100.0 M600.0,100.0')
      assert.equal(chart.down, 'M590.0,0.0 L591.0,100.0 M600.0,100.0')
      assert.deepEqual(trafficChart([], 610_000), { max: 1024, up: '', down: '' })
      assert.deepEqual(trafficChart([{ at: 0, up: 10000, down: 20000 }], 610_000), { max: 1024, up: '', down: '' })
      assert.equal(trafficChart([{ at: 610_000, up: 0, down: 0 }], 610_000).max, 1024)
    })
    await t.test('live traffic updates the connection count and clears it on stop', async () => {
      const { store } = await server.ssrLoadModule('/src/store.ts')
      const { connectWs } = await server.ssrLoadModule('/src/ws.ts')
      let socket
      globalThis.location = { protocol: 'http:', host: 'localhost' }
      globalThis.WebSocket = class { constructor() { socket = this } }
      connectWs()
      socket.onmessage({ data: JSON.stringify({ type: 'traffic', up: 1024, down: 2048, up_rate: 100, down_rate: 200, active_connections: 7 }) })
      assert.equal(store.status.active_connections, 7)
      assert.equal(store.status.down_rate, 200)
      socket.onmessage({ data: JSON.stringify({ type: 'traffic', up: 1024, down: 2048, up_rate: 0, down_rate: 0, active_connections: 0 }) })
      assert.equal(store.status.active_connections, 0)
      assert.equal(store.status.down_rate, 0)
      assert.equal(store.status.traffic_down, 2048)
    })
  } finally {
    if (originalLocation === undefined) delete globalThis.location
    else globalThis.location = originalLocation
    globalThis.WebSocket = originalWebSocket
    await server.close()
  }
})
