<template>
  <aside ref="panel" class="preview-dock" :style="{ width: `${width}px` }" :aria-label="tr('文件预览')" @keydown.esc.stop="$emit('close')">
    <div class="dock-resize" role="separator" :aria-label="tr('调整预览宽度')" aria-orientation="vertical"
      tabindex="0" :aria-valuenow="width" :aria-valuemin="320" :aria-valuemax="800"
      @pointerdown="startResize" @keydown.left.prevent="setWidth(width + 24)"
      @keydown.right.prevent="setWidth(width - 24)" />
    <header class="dock-header" :class="{ dragging }" :title="tr('拖动标题栏可拆分为独立窗口')" @pointerdown="startDrag">
      <GripVertical :size="16" class="grip" />
      <div class="dock-title">
        <strong :title="name">{{ name }}</strong>
        <span v-if="dragging">{{ tr("松开以拆分为独立窗口") }}</span>
      </div>
      <button class="ui-icon-btn" :disabled="opening || !canDetach" :title="tr('拆分为独立窗口')" :aria-label="tr('拆分为独立窗口')" @click="$emit('detach')">
        <Loader2 v-if="opening" :size="16" class="spin" /><ExternalLink v-else :size="16" />
      </button>
      <button class="ui-icon-btn" :title="tr('关闭预览')" :aria-label="tr('关闭预览')" @click="$emit('close')"><X :size="16" /></button>
    </header>
    <div class="dock-content"><slot /></div>
  </aside>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { onBeforeUnmount, ref } from 'vue';
import { ExternalLink, GripVertical, Loader2, X } from 'lucide-vue-next';

const props = defineProps<{ name: string; opening: boolean; canDetach: boolean }>();
const emit = defineEmits<{
  detach: [position?: { x: number; y: number }];
  close: [];
}>();
const panel = ref<HTMLElement | null>(null);
const width = ref(460);
const dragging = ref(false);
try {
  const saved = Number(localStorage.getItem('lovelycarve-preview-width'));
  if (saved >= 320 && saved <= 800) width.value = saved;
} catch { /* Storage is optional. */ }

function setWidth(value: number) {
  width.value = Math.max(320, Math.min(800, value));
  try { localStorage.setItem('lovelycarve-preview-width', String(width.value)); }
  catch { /* Keep the current session's width. */ }
}

let cleanupGesture: (() => void) | undefined;
function trackPointer(event: PointerEvent, move: (event: PointerEvent) => void, end: (event: PointerEvent) => void) {
  cleanupGesture?.();
  const target = event.currentTarget as HTMLElement;
  target.setPointerCapture(event.pointerId);
  const cleanup = () => {
    target.removeEventListener('pointermove', move);
    target.removeEventListener('pointerup', finish);
    target.removeEventListener('pointercancel', cancel);
    target.removeEventListener('lostpointercapture', cancel);
    if (target.hasPointerCapture(event.pointerId)) target.releasePointerCapture(event.pointerId);
    dragging.value = false;
    cleanupGesture = undefined;
  };
  const finish = (next: PointerEvent) => { end(next); cleanup(); };
  const cancel = () => cleanup();
  target.addEventListener('pointermove', move);
  target.addEventListener('pointerup', finish);
  target.addEventListener('pointercancel', cancel);
  target.addEventListener('lostpointercapture', cancel);
  cleanupGesture = cleanup;
}

function startResize(event: PointerEvent) {
  if (event.button !== 0) return;
  event.preventDefault();
  const startX = event.clientX;
  const startWidth = panel.value?.getBoundingClientRect().width ?? width.value;
  trackPointer(event, next => setWidth(startWidth + startX - next.clientX), () => {});
}

function startDrag(event: PointerEvent) {
  if (!props.canDetach || props.opening || event.button !== 0 || (event.target as HTMLElement).closest('button')) return;
  event.preventDefault();
  const startX = event.clientX;
  const startY = event.clientY;
  trackPointer(event, next => {
    dragging.value = Math.hypot(next.clientX - startX, next.clientY - startY) > 48;
  }, next => {
    if (dragging.value) emit('detach', { x: next.screenX - 160, y: next.screenY - 20 });
  });
}
onBeforeUnmount(() => cleanupGesture?.());
</script>

<style scoped>
.preview-dock {
  position: relative; display: flex; flex-direction: column; flex: none;
  min-width: 320px; max-width: min(40%, calc(100% - 880px)); min-height: 0;
  border-left: 1px solid var(--border-color); background: var(--surface-secondary);
}
.dock-resize { position: absolute; left: -4px; top: 0; bottom: 0; width: 8px; cursor: col-resize; z-index: 5; touch-action: none; transition: background var(--transition-fast); }
.dock-resize:hover, .dock-resize:focus-visible { background: var(--primary-glow); outline: none; }
.dock-header { display: flex; align-items: center; gap: 8px; min-height: 48px; padding: 6px 8px 6px 10px; border-bottom: 1px solid var(--border-color); background: var(--surface-primary); cursor: grab; user-select: none; touch-action: none; }
.dock-header.dragging { background: var(--sidebar-active-bg); cursor: grabbing; }
.grip { color: var(--text-secondary); flex: none; opacity: 0.7; }
.dock-title { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.dock-title strong { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; font-size: 13px; font-weight: 600; }
.dock-title span { font-size: 11px; color: var(--primary-color); }
.dock-header .ui-icon-btn:disabled { cursor: wait; }
.dock-content { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
/* Keep room for navigation and the default file columns in the actual host. */
@container file-workspace (max-width: 1200px) {
  .preview-dock { position: absolute; top: 0; right: 0; bottom: 0; max-width: 85%; z-index: 6; box-shadow: var(--modal-shadow); }
}
</style>
