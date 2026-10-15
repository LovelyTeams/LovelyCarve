<template>
  <aside ref="panel" class="detail-dock" :style="{ width: `${width}px` }" :aria-label="label" @keydown.esc.stop="$emit('close')">
    <div class="dock-resize" role="separator" :aria-label="tr('调整详情宽度')" aria-orientation="vertical"
      tabindex="0" :aria-valuenow="width" :aria-valuemin="MIN_WIDTH" :aria-valuemax="MAX_WIDTH"
      @pointerdown="startResize" @keydown.left.prevent="resizeBy(24)" @keydown.right.prevent="resizeBy(-24)" />
    <header class="dock-header">
      <slot name="leading" />
      <div class="dock-title">
        <strong :title="titleHint || title">{{ label }}</strong>
        <span v-if="subtitle" :title="subtitle">{{ subtitle }}</span>
      </div>
      <slot name="actions" />
      <button class="ui-icon-btn" type="button" :title="closeLabel" :aria-label="closeLabel" @click="$emit('close')"><X :size="16" /></button>
    </header>
    <div ref="body" v-content-motion="contentKey" class="dock-body"><slot /></div>
  </aside>
</template>

<script lang="ts">
import { ref } from 'vue';

const WIDTH_KEY = 'lovelycarve-detail-width';
const MIN_WIDTH = 300;
const MAX_WIDTH = 720;
const DEFAULT_WIDTH = 380;

function clampWidth(value: number): number {
  return Math.round(Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, value)));
}

function storedWidth(): number {
  try {
    const saved = Number(localStorage.getItem(WIDTH_KEY));
    return saved >= MIN_WIDTH && saved <= MAX_WIDTH ? saved : DEFAULT_WIDTH;
  } catch {
    return DEFAULT_WIDTH;
  }
}

/** Shared by every dock, so search, carving, timeline and record pages open at the same width. */
const width = ref(storedWidth());

function saveWidth() {
  try { localStorage.setItem(WIDTH_KEY, String(width.value)); }
  catch { /* Keep the width for this session. */ }
}
</script>

<script setup lang="ts">
import { tr } from '../../composables/useLocale';

import { onBeforeUnmount, watch } from 'vue';
import { X } from 'lucide-vue-next';
import { vContentMotion } from '../../composables/uiMotionDirectives';

const props = defineProps<{
  label: string;
  title: string;
  closeLabel: string;
  /** Full value for the title tooltip when the title is a short name. */
  titleHint?: string;
  subtitle?: string;
  /** Identity of the shown record: a change fades the body and scrolls it back to the top. */
  contentKey?: string | number;
}>();
defineEmits<{ close: [] }>();

const panel = ref<HTMLElement | null>(null);
const body = ref<HTMLElement | null>(null);
watch(() => props.contentKey, () => {
  if (body.value) body.value.scrollTop = 0;
});

function resizeBy(delta: number) {
  width.value = clampWidth(width.value + delta);
  saveWidth();
}

let stopResize: (() => void) | undefined;
function startResize(event: PointerEvent) {
  if (event.button !== 0) return;
  event.preventDefault();
  stopResize?.();
  const handle = event.currentTarget as HTMLElement;
  const startX = event.clientX;
  // Start from the rendered width: max-width may hold the dock below the stored value.
  const startWidth = panel.value?.getBoundingClientRect().width ?? width.value;
  const move = (next: PointerEvent) => { width.value = clampWidth(startWidth + startX - next.clientX); };
  const stop = () => {
    handle.removeEventListener('pointermove', move);
    handle.removeEventListener('pointerup', stop);
    handle.removeEventListener('pointercancel', stop);
    handle.removeEventListener('lostpointercapture', stop);
    if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
    stopResize = undefined;
    saveWidth();
  };
  handle.setPointerCapture(event.pointerId);
  handle.addEventListener('pointermove', move);
  handle.addEventListener('pointerup', stop);
  handle.addEventListener('pointercancel', stop);
  handle.addEventListener('lostpointercapture', stop);
  stopResize = stop;
}
onBeforeUnmount(() => stopResize?.());
</script>

<style scoped>
/* Right-hand detail dock of the search, carving, timeline and record pages, drawn like
   the file list's preview dock (FilePreviewDock): a resizable column with a 48px
   header strip. Hosts wrap it in <UiTransition kind="dock"> and give the row
   holding it `position: relative; container: detail-host / inline-size`.
   The dock shares the current window; in narrow rows it floats over the table.
   Neither opening it nor switching pages changes the native window bounds. */
.detail-dock {
  position: relative; display: flex; flex-direction: column; flex: none;
  min-width: 300px; max-width: 48%; min-height: 0;
  background: var(--surface-secondary);
}
/* The left rule is an overlay rather than a border so the content box keeps
   the full minimum width below. */
.detail-dock::before { content: ''; position: absolute; top: 0; bottom: 0; left: 0; z-index: 4; width: 1px; background: var(--border-color); pointer-events: none; }
.dock-resize { position: absolute; left: -4px; top: 0; bottom: 0; width: 8px; cursor: col-resize; z-index: 5; touch-action: none; transition: background var(--transition-fast); }
.dock-resize:hover, .dock-resize:focus-visible { background: var(--primary-glow); outline: none; }
/* Content never narrows below the dock minimum: the horizontal entrance then
   uncovers it from the right instead of re-wrapping it every frame. */
.dock-header, .dock-body { min-width: 300px; }
.dock-header { display: flex; align-items: center; gap: 8px; flex-shrink: 0; min-height: 48px; padding: 6px 8px 6px 14px; border-bottom: 1px solid var(--border-color); background: var(--surface-primary); }
.dock-title { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.dock-title strong { overflow: hidden; font-size: 13px; font-weight: 600; text-overflow: ellipsis; white-space: nowrap; }
.dock-title span { overflow: hidden; color: var(--text-secondary); font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
.dock-body { flex: 1; min-height: 0; overflow: auto; }
@container detail-host (max-width: 800px) {
  .detail-dock { position: absolute; top: 0; right: 0; bottom: 0; z-index: 6; max-width: 85%; box-shadow: var(--modal-shadow); }
}
</style>
