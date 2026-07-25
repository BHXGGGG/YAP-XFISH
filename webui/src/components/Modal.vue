<script setup lang="ts">
import { onBeforeUnmount, onMounted } from 'vue'

const props = defineProps<{
  open: boolean
  title?: string
  subtitle?: string
  /** 关闭文案，默认「关闭」 */
  closeLabel?: string
  /** 宽度（px），默认 480 */
  width?: number
  /** 主图标（非必填），用于在标题左侧展示一个小色块 */
  icon?: string
}>()

const emit = defineEmits<{
  (e: 'close'): void
}>()

function close() { emit('close') }

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape' && props.open) close()
}

onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="modal-mask" @mousedown.self="close">
      <div class="modal-card" :style="{ width: (width || 480) + 'px' }" role="dialog" aria-modal="true">
        <header class="modal-head">
          <div class="modal-title-wrap">
            <span v-if="icon" class="modal-title-bar" aria-hidden="true">{{ icon }}</span>
            <div class="modal-title-text">
              <h3>{{ title || '提示' }}</h3>
              <p v-if="subtitle">{{ subtitle }}</p>
            </div>
          </div>
          <button class="modal-close" type="button" :aria-label="closeLabel || '关闭'" @click="close">×</button>
        </header>
        <div class="modal-body">
          <slot />
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.modal-mask {
  position: fixed; inset: 0;
  background: radial-gradient(ellipse at top, rgba(30, 64, 175, 0.18), rgba(15, 23, 42, 0.55));
  display: flex; align-items: flex-start; justify-content: center;
  padding-top: 10vh;
  z-index: 9999;
  animation: modal-mask-in .18s ease-out both;
}
.modal-card {
  background: linear-gradient(180deg, #ffffff 0%, #fbfcff 100%);
  border-radius: 14px;
  box-shadow:
    0 1px 0 rgba(255, 255, 255, 0.6) inset,
    0 24px 48px -12px rgba(15, 23, 42, 0.35),
    0 8px 16px rgba(15, 23, 42, 0.18);
  border: 1px solid rgba(15, 23, 42, 0.08);
  max-width: calc(100vw - 32px);
  overflow: hidden;
  animation: modal-card-in .22s cubic-bezier(.2,.8,.2,1) both;
}
.modal-head {
  display: flex; align-items: center; justify-content: space-between;
  padding: 18px 22px 14px;
  border-bottom: 1px solid var(--border, #eef0f4);
  background: linear-gradient(180deg, #f8fafc 0%, rgba(248, 250, 252, 0) 100%);
}
.modal-title-wrap {
  display: flex; align-items: center; gap: 12px;
  min-width: 0;
}
.modal-title-bar {
  display: inline-flex; align-items: center; justify-content: center;
  width: 36px; height: 36px;
  border-radius: 10px;
  background: linear-gradient(135deg, #6366f1 0%, #3b82f6 100%);
  color: #fff;
  font-size: 18px;
  font-weight: 700;
  box-shadow: 0 4px 12px rgba(59, 130, 246, 0.35);
  flex-shrink: 0;
}
.modal-title-text { min-width: 0; }
.modal-title-text h3 {
  margin: 0;
  font-size: 16px;
  font-weight: 700;
  color: #0f172a;
  letter-spacing: .2px;
}
.modal-title-text p {
  margin: 2px 0 0;
  font-size: 12px;
  color: #6b7280;
  line-height: 1.4;
}
.modal-close {
  border: 0; background: transparent;
  font-size: 22px; line-height: 1; cursor: pointer; color: #94a3b8;
  padding: 4px 8px;
  border-radius: 8px;
  transition: background .15s, color .15s;
}
.modal-close:hover { color: #0f172a; background: #f1f5f9; }
.modal-body {
  padding: 18px 22px 20px;
}

@keyframes modal-mask-in {
  from { opacity: 0; }
  to   { opacity: 1; }
}
@keyframes modal-card-in {
  from { opacity: 0; transform: translateY(-8px) scale(.98); }
  to   { opacity: 1; transform: translateY(0)    scale(1); }
}
</style>
