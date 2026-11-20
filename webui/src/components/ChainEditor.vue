<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import Modal from './Modal.vue'
import { api } from '../api'
import { store, toast } from '../store'
import { chainChanges, chainPath, type ChainEdge, type ChainNode } from '../chain'

const props = defineProps<{ nodes: ChainNode[]; initial: string[] }>()
const emit = defineEmits<{ (e: 'close'): void; (e: 'saved'): void }>()
const points = ref(props.initial.map((id, i) => ({ id, x: 50 + i * 270, y: 160 })))
const edges = ref<ChainEdge[]>(props.initial.slice(1).map((id, i) => ({ from: props.initial[i], to: id })))
const query = ref('')
const selected = ref('')
const selectedWire = ref<ChainEdge | null>(null)
const linking = ref('')
const cursor = ref({ x: 0, y: 0 })
const zoom = ref(1)
const saving = ref(false)
const error = ref('')
const viewport = ref<HTMLElement | null>(null)
let drag: { id: string; x: number; y: number } | null = null

const filtered = computed(() => props.nodes.filter(node =>
  `${node.name} ${node.type || node.kind || ''}`.toLowerCase().includes(query.value.trim().toLowerCase()),
))
const route = computed(() => {
  try {
    const path = chainPath(points.value.map(point => point.id), edges.value)
    chainChanges(props.initial, path, props.nodes)
    return { path, error: '' }
  } catch (e: any) { return { path: [] as string[], error: e.message } }
})
const chainName = computed(() => (route.value.path.length ? route.value.path : points.value.map(point => point.id)).map(nameOf).join(' → '))
const width = computed(() => Math.max(800, ...points.value.map(point => point.x + 260)))
const height = computed(() => Math.max(360, ...points.value.map(point => point.y + 110)))
function nodeOf(id: string) { return props.nodes.find(node => node.id === id) }
function nameOf(id: string) { return nodeOf(id)?.name || id }
function pointOf(id: string) { return points.value.find(point => point.id === id)! }
function selectNode(id: string) { selected.value = id; selectedWire.value = null }
function coordinates(event: MouseEvent | PointerEvent | DragEvent) {
  const element = viewport.value!
  const rect = element.getBoundingClientRect()
  return { x: (event.clientX - rect.left + element.scrollLeft) / zoom.value, y: (event.clientY - rect.top + element.scrollTop) / zoom.value }
}
function addNode(id: string, position?: { x: number; y: number }) {
  if (saving.value || points.value.some(point => point.id === id)) return
  let path: string[] = []
  try { path = chainPath(points.value.map(point => point.id), edges.value) } catch {}
  const last = path.at(-1)
  const previous = last ? pointOf(last) : null
  points.value.push({ id, x: Math.max(20, position?.x ?? (previous ? previous.x + 270 : 50)), y: Math.max(20, position?.y ?? 160) })
  if (last) edges.value.push({ from: last, to: id })
  selectNode(id)
  error.value = ''
}
function dropNode(event: DragEvent) {
  const id = event.dataTransfer?.getData('text/plain')
  if (id && props.nodes.some(node => node.id === id)) addNode(id, coordinates(event))
}
function removeSelected() {
  if (saving.value) return
  if (selectedWire.value) {
    edges.value = edges.value.filter(edge => edge !== selectedWire.value)
    selectedWire.value = null
    return
  }
  if (!selected.value) return
  const before = edges.value.find(edge => edge.to === selected.value)?.from
  const after = edges.value.find(edge => edge.from === selected.value)?.to
  edges.value = edges.value.filter(edge => edge.from !== selected.value && edge.to !== selected.value)
  points.value = points.value.filter(point => point.id !== selected.value)
  if (before && after) edges.value.push({ from: before, to: after })
  selected.value = linking.value = ''
}
function connectTo(id: string) {
  if (!linking.value || saving.value) return
  if (linking.value === id) { error.value = '不能连接节点自身'; linking.value = ''; return }
  edges.value = [...edges.value.filter(edge => edge.from !== linking.value && edge.to !== id), { from: linking.value, to: id }]
  linking.value = ''
  selectedWire.value = null
  error.value = ''
}
function startDrag(event: PointerEvent, id: string) {
  if (event.button !== 0 || saving.value) return
  selectNode(id)
  const position = coordinates(event), point = pointOf(id)
  drag = { id, x: position.x - point.x, y: position.y - point.y }
  viewport.value!.setPointerCapture(event.pointerId)
  viewport.value!.focus({ preventScroll: true })
}
function startWire(event: PointerEvent, id: string) {
  if (event.button !== 0 || saving.value) return
  linking.value = id
  cursor.value = coordinates(event)
  viewport.value!.setPointerCapture(event.pointerId)
}
function movePointer(event: PointerEvent) {
  const position = coordinates(event)
  cursor.value = position
  if (drag) Object.assign(pointOf(drag.id), { x: Math.max(20, position.x - drag.x), y: Math.max(20, position.y - drag.y) })
}
function endPointer(event: PointerEvent) {
  if (linking.value) {
    const input = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>('[data-input]')
    if (input?.dataset.input) connectTo(input.dataset.input)
  }
  drag = null
  if (viewport.value?.hasPointerCapture(event.pointerId)) viewport.value.releasePointerCapture(event.pointerId)
}
function curve(from: { x: number; y: number }, to: { x: number; y: number }) {
  return `M${from.x},${from.y} C${from.x + 65},${from.y} ${to.x - 65},${to.y} ${to.x},${to.y}`
}
function edgePath(edge: ChainEdge) {
  const from = pointOf(edge.from), to = pointOf(edge.to)
  return curve({ x: from.x + 220, y: from.y + 38 }, { x: to.x, y: to.y + 38 })
}
const pendingPath = computed(() => {
  if (!linking.value) return ''
  const from = pointOf(linking.value)
  return curve({ x: from.x + 220, y: from.y + 38 }, cursor.value)
})
async function fit() {
  await nextTick()
  if (!viewport.value) return
  zoom.value = Math.min(1, (viewport.value.clientWidth - 16) / width.value, (viewport.value.clientHeight - 16) / height.value)
  viewport.value.scrollTo(0, 0)
}
function arrange() {
  const ids = route.value.path.length ? route.value.path : points.value.map(point => point.id)
  ids.forEach((id, i) => Object.assign(pointOf(id), { x: 50 + i * 270, y: 160 }))
  void fit()
}
function onKey(event: KeyboardEvent) {
  if (event.key === 'Delete' || event.key === 'Backspace') { event.preventDefault(); removeSelected() }
  if (event.key === 'Escape' && linking.value) { event.stopPropagation(); linking.value = '' }
}
async function save() {
  if (saving.value || route.value.error) return
  saving.value = true
  error.value = ''
  try {
    const changes = chainChanges(props.initial, route.value.path, props.nodes)
    await api.setNodeDetours(changes)
    for (const change of changes) {
      const node = store.profile.nodes.find(node => node.id === change.id)
      if (node) node.detour = change.detour || ''
    }
    toast('代理链已保存')
    emit('saved')
  } catch (e: any) { error.value = e.message || '保存失败' }
  finally { saving.value = false }
}
void fit()
</script>

<template>
  <Modal :open="true" :title="initial.length ? '编辑代理链' : '新建代理链'" :width="1140" @close="!saving && emit('close')">
    <div class="chain-editor">
      <div class="chain-name"><span>链路</span><input :value="chainName" readonly aria-label="链路名称（由节点自动生成）" placeholder="选择节点后自动生成" /></div>
      <p class="editor-hint">点击左侧节点追加到末尾，或拖到画布。拖动右侧圆点连接下一跳的左侧圆点；选中节点后按 Delete 移除。</p>
      <div class="editor-workspace" :class="{ saving }">
        <aside class="node-palette">
          <div class="palette-title">节点 <span>{{ filtered.length }}</span></div>
          <input v-model="query" type="search" placeholder="搜索节点名 / 协议…" aria-label="搜索链路节点" :disabled="saving" />
          <div class="palette-list">
            <button v-for="node in filtered" :key="node.id" class="palette-node" :draggable="!saving"
                    :disabled="saving || points.some(point => point.id === node.id)" :title="node.name"
                    @click="addNode(node.id)" @dragstart="$event.dataTransfer?.setData('text/plain', node.id)">
              <span>{{ node.name }}</span><small>{{ node.type || node.kind }}{{ points.some(point => point.id === node.id) ? ' · 已添加' : '' }}</small>
            </button>
            <p v-if="!filtered.length" class="editor-hint">没有匹配的节点</p>
          </div>
          <div class="palette-actions"><button :disabled="(!selected && !selectedWire) || saving" @click="removeSelected">移除选中</button><button :disabled="saving || !points.length" @click="arrange">整理布局</button></div>
        </aside>
        <div class="canvas-column">
          <div class="canvas-frame">
            <div ref="viewport" class="chain-canvas" tabindex="0" aria-label="代理链画布" @keydown="onKey"
                 @dragover.prevent @drop.prevent="dropNode" @pointermove="movePointer" @pointerup="endPointer"
                 @pointercancel="drag = null; linking = ''">
              <div class="canvas-size" :style="{ width: width * zoom + 'px', height: height * zoom + 'px' }">
                <div class="canvas-world" :style="{ width: width + 'px', height: height + 'px', transform: `scale(${zoom})` }">
                  <svg class="chain-wires" :width="width" :height="height">
                    <defs><marker id="chain-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" /></marker></defs>
                    <g v-for="edge in edges" :key="edge.from + ':' + edge.to">
                      <path class="wire-line" :class="{ selected: selectedWire === edge }" :d="edgePath(edge)" marker-end="url(#chain-arrow)" />
                      <path class="wire-hit" :d="edgePath(edge)" tabindex="0" role="button" :aria-label="nameOf(edge.from) + ' 到 ' + nameOf(edge.to) + ' 的连线'"
                            @click="selectedWire = edge; selected = ''" @keydown.enter="selectedWire = edge; selected = ''" @focus="selectedWire = edge; selected = ''" />
                    </g>
                    <path v-if="pendingPath" :d="pendingPath" class="pending" />
                  </svg>
                  <div v-for="point in points" :key="point.id" class="chain-node" :class="{ selected: selected === point.id }"
                       :style="{ left: point.x + 'px', top: point.y + 'px' }" @pointerdown.prevent="startDrag($event, point.id)">
                    <button class="port input" :data-input="point.id" :aria-label="'连接到 ' + nameOf(point.id)" :disabled="saving" @pointerdown.stop @click.stop="connectTo(point.id)"></button>
                    <button class="node-content" :disabled="saving" :aria-pressed="selected === point.id" @click="selectNode(point.id)">
                      <strong :title="nameOf(point.id)">{{ nameOf(point.id) }}</strong><small>{{ (nodeOf(point.id)?.type || nodeOf(point.id)?.kind || '').toUpperCase() }}</small>
                    </button>
                    <span class="hop">{{ route.path.indexOf(point.id) >= 0 ? route.path.indexOf(point.id) + 1 : '·' }}</span>
                    <button class="port output" :class="{ linking: linking === point.id }" :aria-label="'从 ' + nameOf(point.id) + ' 连线'" :disabled="saving"
                            @pointerdown.stop.prevent="startWire($event, point.id)" @keydown.enter.prevent="linking = point.id" @keydown.space.prevent="linking = point.id"></button>
                  </div>
                  <div v-if="!points.length" class="canvas-empty">从左侧添加节点，开始连接代理链</div>
                </div>
              </div>
            </div>
            <div class="canvas-tools"><button title="放大" aria-label="放大画布" @click="zoom = Math.min(1.5, zoom + .15)">+</button><button title="缩小" aria-label="缩小画布" @click="zoom = Math.max(.2, zoom - .15)">−</button><button title="适应画布" aria-label="适应画布" @click="fit">⛶</button></div>
          </div>
          <div class="route-status" :class="{ invalid: route.error }" role="status">{{ route.error || `✓ 单线路径 · ${route.path.length} 跳 · 客户端 → 互联网` }}</div>
        </div>
      </div>
      <p class="editor-hint">UDP/QUIC 节点只能作为第一跳。修改节点的前置也会影响使用该节点的其他链路。</p>
      <div v-if="error" class="save-error" role="alert">{{ error }}</div>
      <footer class="editor-footer"><button :disabled="saving" @click="emit('close')">取消</button><button class="save-chain" :disabled="saving || !!route.error" @click="save">{{ saving ? '保存中…' : '保存' }}</button></footer>
    </div>
  </Modal>
</template>

<style scoped>
.chain-editor { max-height: calc(80vh - 100px); overflow: auto; }
.chain-name { display: flex; align-items: center; gap: 12px; }
.chain-name input { min-width: 0; flex: 1; padding: 10px 12px; border: 1px solid var(--border); border-radius: 9px; background: #fff; font: inherit; }
.editor-hint { color: #747b85; font-size: 12px; line-height: 1.6; margin: 12px 0; }
.editor-workspace { display: grid; grid-template-columns: 215px minmax(0, 1fr); gap: 16px; }
.node-palette { display: flex; flex-direction: column; gap: 9px; min-height: 0; }
.palette-title { padding: 7px 14px; border-radius: 999px; background: #edf0f4; font-weight: 600; }
.palette-title span { float: right; color: var(--muted); font-size: 12px; }
.node-palette input { width: 100%; border: 1px solid var(--border); border-radius: 999px; padding: 9px 12px; font: inherit; font-size: 12px; }
.palette-list { height: clamp(180px, 34vh, 320px); overflow: auto; border: 1px solid var(--border); border-radius: 14px; background: #fff; }
.palette-node { width: 100%; padding: 12px; text-align: left; border: 0; border-radius: 0; background: none; cursor: grab; }
.palette-node:hover:not(:disabled) { background: #f1eef9; color: #7354b8; }
.palette-node span { display: block; font-size: 12px; overflow-wrap: anywhere; }
.palette-node small { display: block; margin-top: 4px; color: var(--muted); font-size: 10px; }
.palette-actions { display: flex; gap: 8px; }
.palette-actions button { flex: 1; padding: 7px 8px; border-radius: 999px; font-size: 12px; }
.canvas-column { min-width: 0; }
.canvas-frame { position: relative; border: 1px solid #dadce4; border-radius: 18px; overflow: hidden; background: #f0f1f5; }
.chain-canvas { height: clamp(290px, 48vh, 440px); overflow: auto; background-image: radial-gradient(#ccd0db .8px, transparent .8px); background-size: 24px 24px; touch-action: none; }
.chain-canvas:focus-visible { outline: 2px solid #8a70bd; outline-offset: -3px; }
.canvas-size { position: relative; }
.canvas-world { position: absolute; transform-origin: 0 0; }
.chain-wires { position: absolute; inset: 0; pointer-events: none; overflow: visible; }
.chain-wires .wire-line, .chain-wires .pending { fill: none; stroke: #8060bd; stroke-width: 2.5; stroke-dasharray: 7 5; }
.chain-wires .wire-line.selected { stroke-width: 4; }
.chain-wires .wire-hit { fill: none; stroke: transparent; stroke-width: 14; pointer-events: stroke; cursor: pointer; }
.chain-wires marker path { fill: #8060bd; }
.chain-wires .pending { opacity: .5; }
.chain-node { position: absolute; display: flex; align-items: center; width: 220px; height: 76px; border: 1px solid #d8dbe3; border-radius: 13px; background: #fafbfc; box-shadow: 0 1px 3px #00000009; user-select: none; touch-action: none; }
.chain-node.selected { border: 2px solid #8060bd; box-shadow: 0 0 0 3px #8060bd26; }
.node-content { min-width: 0; flex: 1; text-align: left; border: 0; background: none; padding: 12px 15px; cursor: grab; }
.node-content strong { display: block; font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.node-content small { display: block; margin-top: 7px; font-size: 11px; color: #7b8089; }
.hop { margin-right: 16px; border-radius: 50%; background: #e1eef0; color: #399782; min-width: 23px; height: 23px; text-align: center; line-height: 23px; font-size: 12px; }
.port { position: absolute; z-index: 1; top: 30px; width: 14px; height: 14px; border: 3px solid #8060bd; padding: 0; border-radius: 50%; background: white; cursor: crosshair; }
.port.input { left: -7px; }
.port.output { right: -7px; }
.port.linking { background: #8060bd; }
.port:focus-visible { outline: 2px solid #8060bd; outline-offset: 3px; }
.canvas-tools { position: absolute; left: 14px; bottom: 14px; display: flex; flex-direction: column; box-shadow: 0 1px 4px #0001; }
.canvas-tools button { width: 30px; height: 30px; padding: 0; border-radius: 0; font-size: 21px; line-height: 1; }
.canvas-empty { padding: 60px 30px; color: #8b8f9b; font-size: 13px; }
.route-status { color: #299b80; margin-top: 10px; font-size: 12px; }
.route-status.invalid { color: #8a7d61; }
.save-error { color: #b91c1c; margin-top: 10px; font-size: 12px; }
.editor-footer { display: flex; justify-content: flex-end; gap: 10px; padding-top: 10px; }
.editor-footer button { border-radius: 999px; padding: 7px 22px; }
.save-chain { background: #e3f4ee; color: #278a70; border-color: #a6d4c5; font-weight: 600; }
.saving { opacity: .7; }
@media (max-width: 760px) {
  .editor-workspace { grid-template-columns: 1fr; }
  .palette-list { height: 130px; }
  .palette-node { padding: 8px 12px; }
  .chain-canvas { height: 290px; }
}
</style>
