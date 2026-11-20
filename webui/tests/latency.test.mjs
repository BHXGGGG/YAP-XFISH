import assert from 'node:assert/strict'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'

test('remaining latency tests follow results, failures and timeouts', async (t) => {
  const server = await createServer({
    root: fileURLToPath(new URL('..', import.meta.url)),
    server: { middlewareMode: true },
    appType: 'custom',
  })
  const originalLocation = globalThis.location
  const originalWebSocket = globalThis.WebSocket
  try {
    const { store, updateProfile } = await server.ssrLoadModule('/src/store.ts')
    const { markTesting, clearTesting, testingCount } = await server.ssrLoadModule('/src/latency.ts')
    const { connectWs } = await server.ssrLoadModule('/src/ws.ts')
    let socket
    globalThis.location = { protocol: 'http:', host: 'localhost' }
    globalThis.WebSocket = class { constructor() { socket = this } }
    t.mock.timers.enable({ apis: ['setTimeout'] })
    connectWs()
    store.profile.nodes = [{ id: 'a', detour: 'b' }, { id: 'b' }, { id: 'c' }]
    assert.equal(testingCount.value, 0)

    markTesting(store.profile.nodes)
    assert.equal(testingCount.value, 3)
    const targets = store.profile.nodes.slice()
    updateProfile({ nodes: [{ id: 'a' }, { id: 'b' }, { id: 'c' }], selected_node: 'a', rules: [], mode: 'global' })
    assert.equal(testingCount.value, 3)
    assert.equal(store.profile.nodes[0], targets[0])
    assert.equal(store.profile.nodes[0].detour, undefined)
    socket.onmessage({ data: JSON.stringify({ type: 'profile', profile: {
      nodes: [{ id: 'a' }, { id: 'b' }, { id: 'c' }], selected_node: 'b', rules: [], mode: 'global',
    } }) })
    assert.equal(testingCount.value, 3)
    socket.onmessage({ data: JSON.stringify({ type: 'latency', id: 'a', latency: 42 }) })
    assert.equal(testingCount.value, 2)
    socket.onmessage({ data: JSON.stringify({ type: 'latency', id: 'b', latency: null }) })
    assert.equal(testingCount.value, 1)
    clearTesting([targets[2]])
    assert.equal(testingCount.value, 0)

    markTesting(store.profile.nodes)
    t.mock.timers.tick(30_000)
    assert.equal(testingCount.value, 0)
  } finally {
    if (originalLocation === undefined) delete globalThis.location
    else globalThis.location = originalLocation
    globalThis.WebSocket = originalWebSocket
    t.mock.restoreAll()
    t.mock.timers.reset()
    await server.close()
  }
})
