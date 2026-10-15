<template>
  <div class="toast-host">
    <TransitionGroup name="toast" @before-leave="pinLeaving">
      <div
        v-for="toast in toasts"
        :key="toast.id"
        class="toast-item"
        :class="[toast.kind, { persistent: toast.persistent }]"
        :role="toast.kind === 'error' ? 'alert' : 'status'"
        @pointerenter="pauseToast(toast.id)"
        @pointerleave="onPointerLeave($event, toast.id)"
        @focusin="pauseToast(toast.id)"
        @focusout="onFocusOut($event, toast.id)"
      >
        <span class="toast-icon" aria-hidden="true">
          <component :is="kindIcons[toast.kind]" :size="16" />
        </span>
        <div class="toast-copy">
          <div class="toast-title">{{ toast.title }}</div>
          <div v-if="toast.detail" class="toast-detail">{{ toast.detail }}</div>
          <div
            v-if="typeof toast.progress === 'number'"
            class="ui-progress toast-progress"
            :title="`${toast.progress.toFixed(1)}%`"
          >
            <span :style="{ width: clampProgress(toast.progress) + '%' }"></span>
          </div>
        </div>
        <div class="toast-actions">
          <button
            v-if="toast.action"
            class="ui-btn sm"
            :disabled="toast.action.disabled"
            @click="toast.action.onClick"
          >
            {{ toast.action.label }}
          </button>
          <button class="ui-icon-btn sm" :aria-label="tr('关闭通知')" @click="removeToast(toast.id)">
            <X :size="14" />
          </button>
        </div>
      </div>
    </TransitionGroup>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../composables/useLocale';

import type { Component } from 'vue';
import { AlertCircle, CheckCircle2, Info, X } from 'lucide-vue-next';
import { useToast, type ToastKind } from '../../composables/useToast';

const { toasts, removeToast, pauseToast, resumeToast } = useToast();

const kindIcons: Record<ToastKind, Component> = {
  success: CheckCircle2,
  error: AlertCircle,
  info: Info,
};

/** A timed toast holds while the pointer or keyboard focus is on it and
 *  resumes only once both have left. */
function onPointerLeave(event: PointerEvent, id: number) {
  const toast = event.currentTarget as HTMLElement;
  if (!toast.contains(document.activeElement)) resumeToast(id);
}

function onFocusOut(event: FocusEvent, id: number) {
  const toast = event.currentTarget as HTMLElement;
  if (toast.contains(event.relatedTarget as Node | null) || toast.matches(':hover')) return;
  resumeToast(id);
}

/**
 * Take a leaving toast out of the stack, pinned where it is, so the others
 * glide into its place (`toast-move`) while it slides away. The stack is
 * anchored at the bottom right, so pin from those edges.
 */
function pinLeaving(element: Element) {
  const toast = element as HTMLElement;
  const host = toast.offsetParent as HTMLElement | null;
  if (!host) return;
  const bottom = host.clientHeight - toast.offsetTop - toast.offsetHeight;
  Object.assign(toast.style, {
    position: 'absolute',
    right: '0px',
    bottom: `${bottom}px`,
    width: `${toast.offsetWidth}px`,
  });
}

function clampProgress(n: number): number {
  if (Number.isNaN(n)) return 0;
  if (n < 0) return 0;
  if (n > 100) return 100;
  return n;
}
</script>

<style scoped>
.toast-host {
  position: fixed;
  right: 18px;
  bottom: 46px;
  z-index: 12000;
  display: flex;
  flex-direction: column;
  gap: 10px;
  pointer-events: none;
}

/* The kind reads from the icon tint (--toast-accent). */
.toast-item {
  --toast-accent: var(--sev-info-fg);
  min-width: 320px;
  max-width: 480px;
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 12px 10px 12px 12px;
  overflow: hidden;
  border-radius: var(--radius-lg);
  border: 1px solid var(--border-color);
  background: color-mix(in srgb, var(--surface-secondary) 94%, transparent);
  box-shadow: var(--card-shadow-hover);
  backdrop-filter: blur(14px);
  -webkit-backdrop-filter: blur(14px);
  pointer-events: auto;
}

.toast-item.success { --toast-accent: var(--sev-success-fg); }
.toast-item.error { --toast-accent: var(--sev-critical-fg); }
.toast-item.info { --toast-accent: var(--primary-color); }

/* While a dialog or a menu is open, standing notices (persistent toasts such
   as 「检测到 BitLocker」 or export progress) step aside so they never cover
   its actions — the statusbar theme menu opens right under this stack — and
   return when it closes. Timed feedback still shows on top. */
body:has(.ui-modal-overlay, .ui-menu) .toast-item.persistent {
  display: none;
}

.toast-icon {
  display: grid;
  place-items: center;
  flex-shrink: 0;
  width: 20px;
  height: 20px;
  color: var(--toast-accent);
}

.toast-copy {
  flex: 1;
  min-width: 0;
  padding-top: 1px;
}

.toast-title {
  font-size: 13px;
  font-weight: 600;
  line-height: 1.45;
  color: var(--text-primary);
}

.toast-detail {
  margin-top: 3px;
  font-size: 12px;
  line-height: 1.5;
  color: var(--text-secondary);
  word-break: break-all;
}

.toast-progress {
  margin-top: 8px;
}

.toast-progress > span {
  transition: width var(--motion-normal) var(--ease-out);
}

.toast-actions {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

/* Slides in from the screen edge with a spring, leaves the same way. */
.toast-enter-active {
  transition: opacity var(--motion-fast) var(--ease-out), transform var(--motion-normal) var(--ease-spring);
}

.toast-move {
  transition: transform var(--motion-normal) var(--ease-out);
}

/* After .toast-move: a pinned leaving toast can be tagged as moved too (its
   pixel-rounded position), and must keep fading as well as sliding. */
.toast-leave-active {
  transition: opacity var(--motion-fast) var(--ease-in), transform var(--motion-fast) var(--ease-in);
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateX(32px) scale(0.96);
}

@media (max-width: 640px) {
  .toast-host {
    left: 14px;
    right: 14px;
    bottom: 14px;
  }

  .toast-item {
    min-width: 0;
    max-width: none;
    width: 100%;
  }
}
</style>
