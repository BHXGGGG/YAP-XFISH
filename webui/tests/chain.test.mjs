import assert from 'node:assert/strict'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'

test('chain canvas saves client-to-internet order and rejects invalid paths', async () => {
  const server = await createServer({ root: fileURLToPath(new URL('..', import.meta.url)), server: { middlewareMode: true, ws: false }, appType: 'custom' })
  try {
    const { chainPath, chainChanges, existingChain } = await server.ssrLoadModule('/src/chain.ts')
    const nodes = [
      { id: 'a', name: 'A', type: 'hysteria2' },
      { id: 'b', name: 'B', type: 'trojan', detour: 'a' },
      { id: 'c', name: 'C', type: 'vless', detour: 'b' },
    ]
    assert.deepEqual(existingChain('c', nodes), ['a', 'b', 'c'])
    const path = chainPath(['c', 'a', 'b'], [{ from: 'b', to: 'c' }, { from: 'a', to: 'b' }])
    assert.deepEqual(path, ['a', 'b', 'c'])
    assert.deepEqual(chainChanges([], path, nodes), [{ id: 'a', detour: null }, { id: 'b', detour: 'a' }, { id: 'c', detour: 'b' }])
    assert.deepEqual(chainChanges(['a', 'b', 'c'], ['a', 'c'], nodes), [{ id: 'a', detour: null }, { id: 'b', detour: null }, { id: 'c', detour: 'a' }])
    assert.throws(() => chainPath(['a', 'b', 'c'], [{ from: 'a', to: 'b' }, { from: 'a', to: 'c' }]), /分叉/)
    assert.throws(() => chainPath(['a', 'b'], [{ from: 'a', to: 'b' }, { from: 'b', to: 'a' }]), /路径/)
    assert.throws(() => chainPath(['a', 'b'], []), /路径/)
    assert.throws(() => chainChanges([], ['b', 'a'], nodes), /第一跳/)
    assert.throws(() => chainChanges([], ['a'], nodes), /至少/)
    assert.throws(() => existingChain('c', nodes.slice(1)), /不可用/)
  } finally { await server.close() }
})
