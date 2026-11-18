<template>
  <aside ref="panel" class="preview-dock" :class="placement" :style="sizeStyle" :aria-label="tr('文件预览')" @keydown.esc.stop="$emit('close')">
    <div class="dock-resize" role="separator" :aria-label="bottom ? tr('调整预览高度') : tr('调整预览宽度')"
      :title="bottom ? tr('拖动调整高度，双击恢复默认高度') : tr('拖动调整宽度，双击恢复默认宽度')"
      :aria-orientation="bottom ? 'horizontal' : 'vertical'" tabindex="0" :aria-valuenow="shownSize" :aria-valuemin="range.min" :aria-valuemax="range.max"
      @pointerdown="startResize" @dblclick="resetSize" @keydown="onResizeKey" />
    <header class="dock-header" :class="{ dragging }" :title="tr('拖动标题栏空白处可拆分为独立窗口')" @pointerdown="startDrag">
      <GripVertical :size="16" class="grip" aria-hidden="true" />
      <div class="dock-title">
        <span v-if="dragging" class="dock-drag-hint">{{ tr("松开以拆分为独立窗口") }}</span>
        <slot v-else name="tabs"><strong :title="name">{{ name }}</strong></slot>
      </div>
      <button v-if="canMove" class="ui-icon-btn" :disabled="moveLocked"
        :title="moveLocked ? tr('工作区较窄，预览显示在列表下方') : bottom ? tr('移到右侧') : tr('移到下方')"
        :aria-label="bottom ? tr('移到右侧') : tr('移到下方')" @click="$emit('move', bottom ? 'right' : 'bottom')">
        <PanelRight v-if="bottom" :size="16" /><PanelBottom v-else :size="16" />
      </button>
      <button class="ui-icon-btn" :class="{ busy: opening }" :disabled="opening || !canDetach" :title="tr('拆分为独立窗口')" :aria-label="tr('拆分为独立窗口')" @click="$emit('detach')">
        <Loader2 v-if="opening" :size="16" class="spin" /><ExternalLink v-else :size="16" />
      </button>
      <button class="ui-icon-btn" :title="tr('关闭预览')" :aria-label="tr('关闭预览')" @click="$emit('close')"><X :size="16" /></button>
    </header>
    <div class="dock-content"><slot /></div>
  </aside>
</template>

<script lang="ts">
/** Beside the file list or below it. */
export type FilePreviewPlacement = 'right' | 'bottom';
</script>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, nextTick, onBeforeUnmount, ref } from 'vue';
import { ExternalLink, GripVertical, Loader2, PanelBottom, PanelRight, X } from 'lucide-vue-next';
import { animateUi } from '../../../composables/useUiMotion';

const props = withDefaults(defineProps<{
  name?: string;
  opening: boolean;
  canDetach: boolean;
  /** Beside the list (width) or below it (height). */
  placement?: FilePreviewPlacement;
  /** Room the host can give along the resize axis; the saved size is kept for larger hosts. */
  limit?: number;
  /** Shows the control that moves the preview to the other side. */
  canMove?: boolean;
  /** The host has no room beside the list, so the control stays disabled. */
  moveLocked?: boolean;
}>(), { name: '', placement: 'right', limit: undefined });
const emit = defineEmits<{
  detach: [position?: { x: number; y: number }];
  close: [];
  move: [placement: FilePreviewPlacement];
}>();
const SIZES = {
  right: { key: 'lovelycarve-preview-width', fallback: 460, min: 320, max: 800, cssProperty: 'width' },
  bottom: { key: 'lovelycarve-preview-height', fallback: 400, min: 200, max: 1200, cssProperty: 'height' },
} as const;
const panel = ref<HTMLElement | null>(null);
const dragging = ref(false);
const bottom = computed(() => props.placement === 'bottom');
const axis = computed(() => SIZES[props.placement]);
function stored(placement: FilePreviewPlacement) {
  const { key, fallback, min, max } = SIZES[placement];
  try {
    const saved = Number(localStorage.getItem(key));
    if (saved >= min && saved <= max) return saved;
  } catch { /* Storage is optional. */ }
  return fallback;
}
const sizes = ref<Record<FilePreviewPlacement, number>>({ right: stored('right'), bottom: stored('bottom') });
const range = computed(() => ({ min: axis.value.min, max: Math.max(axis.value.min, Math.min(axis.value.max, props.limit ?? axis.value.max)) }));
const shownSize = computed(() => Math.min(range.value.max, Math.max(range.value.min, sizes.value[props.placement])));
const sizeStyle = computed(() => ({ [axis.value.cssProperty]: `${shownSize.value}px` }));

function setSize(value: number) {
  const { key, min, max } = axis.value;
  const next = Math.round(Math.max(min, Math.min(range.value.max, max, value)));
  sizes.value = { ...sizes.value, [props.placement]: next };
  try { localStorage.setItem(key, String(next)); }
  catch { /* Keep the current session's size. */ }
}

/** Double-clicking the divider eases the preview back to the default size. */
async function resetSize() {
  const element = panel.value;
  const property = axis.value.cssProperty;
  const from = element?.getBoundingClientRect()[property] ?? 0;
  setSize(axis.value.fallback);
  await nextTick();
  const to = element?.getBoundingClientRect()[property] ?? 0;
  if (element && from && Math.abs(to - from) >= 1) {
    animateUi(element, [{ [property]: `${from}px` }, { [property]: `${to}px` }], undefined, { speed: 'normal', easing: 'expand' });
  }
}

/** The divider sits on the list side: towards the list grows the preview. */
function onResizeKey(event: KeyboardEvent) {
  const grow = bottom.value ? 'ArrowUp' : 'ArrowLeft';
  const shrink = bottom.value ? 'ArrowDown' : 'ArrowRight';
  if (event.key !== grow && event.key !== shrink) return;
  event.preventDefault();
  setSize(shownSize.value + (event.key === grow ? 24 : -24));
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
  const vertical = bottom.value;
  const start = vertical ? event.clientY : event.clientX;
  const startSize = panel.value?.getBoundingClientRect()[axis.value.cssProperty] ?? shownSize.value;
  trackPointer(event, next => setSize(startSize + start - (vertical ? next.clientY : next.clientX)), () => {});
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
  min-width: 0; min-height: 0; background: var(--surface-secondary);
}
.preview-dock.right { border-left: 1px solid var(--border-color); }
.preview-dock.bottom { border-top: 1px solid var(--border-color); }
.dock-resize { position: absolute; z-index: 5; touch-action: none; transition: background var(--transition-fast); }
.right > .dock-resize { left: -4px; top: 0; bottom: 0; width: 8px; cursor: col-resize; }
.bottom > .dock-resize { top: -4px; left: 0; right: 0; height: 8px; cursor: row-resize; }
.dock-resize:hover, .dock-resize:focus-visible { background: var(--primary-glow); outline: none; }
/* 并排时与目录树、筛选条同高（48px），三栏的分隔线连成一线。 */
.dock-header { display: flex; align-items: center; gap: 4px; min-height: 48px; padding: 4px 8px 4px 6px; border-bottom: 1px solid var(--border-color); background: var(--surface-primary); cursor: grab; user-select: none; touch-action: none; }
.dock-header.dragging { background: var(--sidebar-active-bg); cursor: grabbing; }
.grip { color: var(--text-secondary); flex: none; opacity: 0.7; }
.dock-title { flex: 1; min-width: 0; display: flex; align-items: center; align-self: stretch; }
.dock-title strong { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; padding-left: 4px; font-size: 13px; font-weight: 600; }
.dock-drag-hint { padding-left: 4px; font-size: 12px; color: var(--primary-color); }
.dock-header .ui-icon-btn { flex: none; }
.dock-header .ui-icon-btn.busy { cursor: wait; }
.dock-content { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
</style>
