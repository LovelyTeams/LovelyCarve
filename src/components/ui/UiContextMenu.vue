<template>
  <Teleport to="body">
    <template v-if="open">
      <div
        v-for="panel in panels"
        :key="panel.key"
        :ref="element => bindPanel(panel.key, element)"
        class="ui-menu ui-context-menu"
        :class="{ 'ui-context-submenu': panel.key === 'sub' }"
        popover="manual"
        role="menu"
        tabindex="-1"
        :aria-label="panel.label"
        :aria-describedby="panel.key === 'root' && ($slots.header || heading) ? headerId : undefined"
        @keydown="onKeydown($event, panel.key)"
        @focusin="onFocusIn"
        @focusout="onFocusOut"
        @pointerenter="panel.key === 'sub' && cancelSubmenuClose()"
        @pointerleave="panel.key === 'root' && onRootLeave()"
        @contextmenu.prevent
      >
        <template v-if="panel.key === 'root'">
          <div v-if="$slots.header || heading" :id="headerId" class="ui-context-menu-header" role="none">
            <slot name="header">
              <slot name="heading-icon">
                <component :is="headingIcon" v-if="headingIcon" :size="15" class="ui-context-menu-heading-icon" aria-hidden="true" />
              </slot>
              <span class="ui-context-menu-heading">
                <span class="ui-context-menu-title" :title="heading">{{ heading }}</span>
                <span v-if="subheading" class="ui-context-menu-subtitle" :title="subheading">{{ subheading }}</span>
              </span>
            </slot>
          </div>
          <slot name="before" />
          <div v-if="($slots.header || heading || $slots.before) && panel.entries.length" class="ui-menu-sep" role="separator"></div>
        </template>
        <template v-for="(entry, index) in panel.entries" :key="entryKey(entry, index)">
          <div v-if="entry.kind === 'separator'" class="ui-menu-sep" role="separator"></div>
          <div v-else-if="entry.kind === 'label'" class="ui-menu-label" role="presentation">{{ entry.label }}</div>
          <button
            v-else-if="entry.kind === 'submenu'"
            type="button"
            class="ui-menu-item ui-context-menu-item"
            role="menuitem"
            aria-haspopup="menu"
            :aria-expanded="submenuId === entry.id"
            :data-submenu="entry.id"
            :disabled="entry.disabled"
            :title="entry.title"
            @pointerenter="onSubmenuTriggerEnter(entry, $event)"
            @click="onSubmenuTriggerClick(entry, $event)"
          >
            <component :is="entry.icon" v-if="entry.icon" :size="13" aria-hidden="true" />
            <span class="ui-context-menu-text">{{ entry.label }}</span>
            <ChevronRight :size="12" class="ui-context-menu-caret" aria-hidden="true" />
          </button>
          <button
            v-else
            type="button"
            class="ui-menu-item ui-context-menu-item"
            :class="{ danger: entry.danger }"
            :role="entry.checked === undefined ? 'menuitem' : 'menuitemcheckbox'"
            :aria-checked="entry.checked"
            :disabled="entry.disabled"
            :title="entry.title"
            @pointerenter="onItemEnter(panel.key, $event)"
            @click="choose(entry)"
          >
            <span v-if="entry.checked !== undefined" class="ui-context-menu-check" aria-hidden="true"><Check v-if="entry.checked" :size="13" /></span>
            <Loader2 v-else-if="entry.busy" :size="13" class="spin" aria-hidden="true" />
            <component :is="entry.icon" v-else-if="entry.icon" :size="13" aria-hidden="true" />
            <span class="ui-context-menu-text">{{ entry.label }}</span>
            <span v-if="entry.hint" class="ui-context-menu-hint">{{ entry.hint }}</span>
          </button>
        </template>
      </div>
    </template>
  </Teleport>
</template>

<script lang="ts">
import type { Component } from 'vue';

export interface UiContextMenuItem {
  kind?: 'item';
  id: string;
  label: string;
  icon?: Component;
  /** Right-aligned shortcut or secondary text. */
  hint?: string;
  title?: string;
  disabled?: boolean;
  danger?: boolean;
  /** Shows a spinner in place of the icon while the action runs. */
  busy?: boolean;
  /** Makes the item a toggle (`menuitemcheckbox`) showing its state. */
  checked?: boolean;
  /** Runs after the menu has closed and returned the focus. */
  action?: () => void;
}

export interface UiContextMenuSubmenu {
  kind: 'submenu';
  id: string;
  label: string;
  icon?: Component;
  title?: string;
  disabled?: boolean;
  items: readonly UiContextMenuLeaf[];
}

export interface UiContextMenuSeparator {
  kind: 'separator';
}

export interface UiContextMenuLabel {
  kind: 'label';
  label: string;
}

export type UiContextMenuLeaf = UiContextMenuItem | UiContextMenuSeparator | UiContextMenuLabel;
export type UiContextMenuEntry = UiContextMenuLeaf | UiContextMenuSubmenu;

/** One context menu is open at a time: opening another closes this one. */
let closeOpenMenu: (() => void) | null = null;
</script>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onDeactivated, onMounted, ref, useId, watch, type ComponentPublicInstance } from 'vue';
import { Check, ChevronRight, Loader2 } from 'lucide-vue-next';
import { placeContextMenu, placeSubmenu, type MenuPlacement } from '../../utils/menuPlacement';

/** The app's context menu: a `ui-menu` in the top layer at a viewport point,
 *  with one level of submenus. Focus moves into the menu (arrow keys, Home /
 *  End, → / ← for submenus, Escape and Tab close) and returns to where it was
 *  when the menu closes; an outside press, a scroll, a resize or another
 *  context menu opening closes it too. */
const props = defineProps<{
  x: number;
  y: number;
  entries: readonly UiContextMenuEntry[];
  /** Accessible name of the menu. */
  label: string;
  /** Names what the menu acts on, above the entries. */
  heading?: string;
  subheading?: string;
  headingIcon?: Component;
}>();
const open = defineModel<boolean>('open', { required: true });
const emit = defineEmits<{ select: [id: string] }>();

type PanelKey = 'root' | 'sub';

const SUBMENU_CLOSE_DELAY_MS = 200;
const ITEM_SELECTOR = '[role="menuitem"], [role="menuitemcheckbox"]';
const headerId = useId();

const submenuId = ref<string | null>(null);
const submenu = computed(() => {
  const id = submenuId.value;
  if (!id) return null;
  const entry = props.entries.find((candidate): candidate is UiContextMenuSubmenu => candidate.kind === 'submenu' && candidate.id === id);
  return entry && !entry.disabled ? entry : null;
});
const panels = computed(() => {
  const list: { key: PanelKey; label: string; entries: readonly UiContextMenuEntry[] }[] = [
    { key: 'root', label: props.label, entries: props.entries },
  ];
  if (submenu.value) list.push({ key: 'sub', label: submenu.value.label, entries: submenu.value.items });
  return list;
});

const elements: Record<PanelKey, HTMLElement | null> = { root: null, sub: null };
const shown = new WeakSet<HTMLElement>();
let active = false;
let returnFocus: HTMLElement | null = null;
let submenuTrigger: HTMLElement | null = null;
let submenuTimer: ReturnType<typeof setTimeout> | null = null;

function bindPanel(key: PanelKey, element: Element | ComponentPublicInstance | null) {
  elements[key] = element instanceof HTMLElement ? element : null;
}

function entryKey(entry: UiContextMenuEntry, index: number): string {
  return entry.kind === 'separator' || entry.kind === 'label' ? `${entry.kind}:${index}` : `item:${entry.id}`;
}

function isInside(node: EventTarget | null): boolean {
  return node instanceof Node && Boolean(elements.root?.contains(node) || elements.sub?.contains(node));
}

function viewport() {
  return { width: window.innerWidth, height: window.innerHeight };
}

/** The top layer keeps the menu above dialogs and outside clipping containers. */
function showPanel(panel: HTMLElement) {
  if (shown.has(panel)) return;
  shown.add(panel);
  if (typeof panel.showPopover !== 'function') return;
  try {
    panel.showPopover();
  } catch {
    // Already shown or detached: the fixed layer and z-index still place it.
  }
}

function applyPlacement(panel: HTMLElement, placement: MenuPlacement, key: PanelKey) {
  panel.style.left = `${placement.left}px`;
  panel.style.top = `${placement.top}px`;
  panel.style.maxHeight = `${placement.maxHeight}px`;
  // ui-pop-in (motion.css) grows from the corner at the pointer.
  panel.style.transformOrigin = `${placement.leftward ? 'right' : 'left'} ${placement.upward ? 'bottom' : 'top'}`;
  panel.style.setProperty('--ui-pop-offset', key === 'sub' ? '0px' : placement.upward ? '4px' : '-4px');
}

function placeRoot() {
  const panel = elements.root;
  if (!panel) return;
  panel.style.maxHeight = 'none';
  // Offset sizes are layout sizes, unaffected by the entrance animation's scale.
  const size = { width: panel.offsetWidth, height: panel.offsetHeight };
  applyPlacement(panel, placeContextMenu({ x: props.x, y: props.y }, size, viewport()), 'root');
}

function placeSub() {
  const panel = elements.sub;
  const trigger = submenuTrigger;
  if (!panel || !trigger) return;
  panel.style.maxHeight = 'none';
  const size = { width: panel.offsetWidth, height: panel.offsetHeight };
  applyPlacement(panel, placeSubmenu(trigger.getBoundingClientRect(), size, viewport()), 'sub');
}

function activate() {
  const root = elements.root;
  if (!open.value || !root || active) return;
  if (closeOpenMenu) closeOpenMenu();
  active = true;
  closeOpenMenu = close;
  const focused = document.activeElement;
  returnFocus = focused instanceof HTMLElement && focused !== document.body ? focused : null;
  showPanel(root);
  placeRoot();
  root.focus({ preventScroll: true });
  document.addEventListener('pointerdown', onOutsidePointer, true);
  document.addEventListener('scroll', onScroll, { capture: true, passive: true });
  window.addEventListener('resize', close);
  window.addEventListener('blur', close);
}

/** Runs before the panels unmount, while it can still tell whether the
 *  focus is inside them. */
function deactivate() {
  if (!active) return;
  active = false;
  if (closeOpenMenu === close) closeOpenMenu = null;
  document.removeEventListener('pointerdown', onOutsidePointer, true);
  document.removeEventListener('scroll', onScroll, true);
  window.removeEventListener('resize', close);
  window.removeEventListener('blur', close);
  cancelSubmenuClose();
  submenuId.value = null;
  submenuTrigger = null;
  const focused = document.activeElement;
  const target = returnFocus;
  returnFocus = null;
  if ((isInside(focused) || focused === document.body || focused === null) && target?.isConnected) {
    target.focus({ preventScroll: true });
  }
}

function close() {
  deactivate();
  if (open.value) open.value = false;
}

watch(open, value => {
  if (value) void nextTick(activate);
  else deactivate();
}, { flush: 'sync' });
watch(() => [props.x, props.y], () => {
  if (!active) return;
  closeSubmenu(false);
  void nextTick(placeRoot);
});
watch(() => props.entries, () => {
  if (!active) return;
  void nextTick(() => {
    if (submenuId.value && !submenu.value) closeSubmenu(false);
    placeRoot();
    placeSub();
  });
});
onMounted(() => {
  if (open.value) void nextTick(activate);
});
onDeactivated(close);
onBeforeUnmount(deactivate);

function onOutsidePointer(event: PointerEvent) {
  if (!isInside(event.target)) close();
}

function onScroll(event: Event) {
  if (!isInside(event.target)) close();
}

// ── Highlight ──
// Mouse and keyboard share one highlight: the focused item. Hovering an item
// focuses it, so the arrow keys continue from where the pointer is.

function onFocusIn(event: FocusEvent) {
  const element = event.target;
  if (element instanceof HTMLElement && element.matches(ITEM_SELECTOR)) element.setAttribute('data-highlighted', '');
}

function onFocusOut(event: FocusEvent) {
  const element = event.target;
  if (element instanceof HTMLElement) element.removeAttribute('data-highlighted');
  // Focus taken by another control (not one of ours, not just lost) closes the menu.
  const next = event.relatedTarget;
  if (active && next instanceof Node && !isInside(next)) close();
}

function enabledItems(panel: HTMLElement): HTMLElement[] {
  return [...panel.querySelectorAll<HTMLElement>(ITEM_SELECTOR)].filter(
    element => !(element instanceof HTMLButtonElement && element.disabled) && element.getAttribute('aria-disabled') !== 'true',
  );
}

function focusStep(panel: HTMLElement, step: 1 | -1) {
  const items = enabledItems(panel);
  if (!items.length) return;
  const current = items.indexOf(document.activeElement as HTMLElement);
  const next = current < 0 ? (step === 1 ? 0 : items.length - 1) : (current + step + items.length) % items.length;
  items[next].focus();
}

function focusEdge(panel: HTMLElement, edge: 'first' | 'last') {
  const items = enabledItems(panel);
  items[edge === 'first' ? 0 : items.length - 1]?.focus();
}

function onItemEnter(key: PanelKey, event: PointerEvent) {
  if (key === 'root') scheduleSubmenuClose();
  const element = event.currentTarget;
  if (!(element instanceof HTMLButtonElement)) return;
  if (element.disabled) elements[key]?.focus({ preventScroll: true });
  else element.focus({ preventScroll: true });
}

function onRootLeave() {
  const root = elements.root;
  if (submenuId.value || !root || !root.contains(document.activeElement)) return;
  root.focus({ preventScroll: true });
}

// ── Submenus ──

function submenuEntry(id: string | undefined): UiContextMenuSubmenu | null {
  const entry = props.entries.find(candidate => candidate.kind === 'submenu' && candidate.id === id);
  return entry?.kind === 'submenu' && !entry.disabled ? entry : null;
}

function openSubmenu(entry: UiContextMenuSubmenu, trigger: HTMLElement, focusFirst: boolean) {
  cancelSubmenuClose();
  if (entry.disabled) return;
  submenuTrigger = trigger;
  submenuId.value = entry.id;
  void nextTick(() => {
    const panel = elements.sub;
    if (!panel || submenuId.value !== entry.id) return;
    showPanel(panel);
    placeSub();
    if (focusFirst) focusEdge(panel, 'first');
  });
}

function closeSubmenu(returnToTrigger: boolean) {
  cancelSubmenuClose();
  if (!submenuId.value) return;
  const trigger = submenuTrigger;
  submenuId.value = null;
  submenuTrigger = null;
  if (returnToTrigger && trigger?.isConnected) trigger.focus({ preventScroll: true });
}

function scheduleSubmenuClose() {
  if (!submenuId.value || submenuTimer !== null) return;
  submenuTimer = setTimeout(() => {
    submenuTimer = null;
    closeSubmenu(false);
  }, SUBMENU_CLOSE_DELAY_MS);
}

function cancelSubmenuClose() {
  if (submenuTimer === null) return;
  clearTimeout(submenuTimer);
  submenuTimer = null;
}

function onSubmenuTriggerEnter(entry: UiContextMenuSubmenu, event: PointerEvent) {
  const trigger = event.currentTarget;
  if (!(trigger instanceof HTMLButtonElement)) return;
  if (entry.disabled) {
    scheduleSubmenuClose();
    elements.root?.focus({ preventScroll: true });
    return;
  }
  trigger.focus({ preventScroll: true });
  if (submenuId.value === entry.id) {
    cancelSubmenuClose();
    return;
  }
  closeSubmenu(false);
  openSubmenu(entry, trigger, false);
}

/** A click keeps a hovered submenu open; Enter or Space (a click without a
 *  pointer) opens it and moves into it. */
function onSubmenuTriggerClick(entry: UiContextMenuSubmenu, event: MouseEvent) {
  const trigger = event.currentTarget;
  if (!(trigger instanceof HTMLButtonElement)) return;
  const fromKeyboard = event.detail === 0;
  if (submenuId.value === entry.id && !fromKeyboard) return;
  openSubmenu(entry, trigger, fromKeyboard);
}

function choose(entry: UiContextMenuItem) {
  if (entry.disabled) return;
  close();
  entry.action?.();
  emit('select', entry.id);
}

function onKeydown(event: KeyboardEvent, key: PanelKey) {
  const panel = elements[key];
  if (!panel || event.isComposing) return;
  switch (event.key) {
    case 'ArrowDown':
      focusStep(panel, 1);
      break;
    case 'ArrowUp':
      focusStep(panel, -1);
      break;
    case 'Home':
      focusEdge(panel, 'first');
      break;
    case 'End':
      focusEdge(panel, 'last');
      break;
    case 'ArrowRight': {
      if (key !== 'root') return;
      const trigger = document.activeElement instanceof HTMLElement ? document.activeElement.closest<HTMLElement>('[data-submenu]') : null;
      const entry = submenuEntry(trigger?.dataset.submenu);
      if (!trigger || !entry) return;
      openSubmenu(entry, trigger, true);
      break;
    }
    case 'ArrowLeft':
      if (key !== 'sub') return;
      closeSubmenu(true);
      break;
    case 'Escape':
      if (key === 'sub') closeSubmenu(true);
      else close();
      break;
    case 'Tab':
      close();
      break;
    default:
      return;
  }
  // Handled here only: the panel and global shortcuts behind the menu do not see it.
  event.preventDefault();
  event.stopPropagation();
}
</script>

<style scoped>
/* Normally in the top layer through the Popover API; the fixed layer and
   z-index only matter where that API is missing. */
.ui-context-menu {
  position: fixed;
  inset: auto;
  z-index: 9100;
  margin: 0;
  min-width: 200px;
  max-width: min(360px, calc(100vw - 16px));
  overflow-x: hidden;
  overflow-y: auto;
  overscroll-behavior: contain;
  outline: none;
}

.ui-context-submenu {
  min-width: 140px;
}

.ui-context-menu-header {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  padding: 6px 10px 4px;
  font-family: var(--font-ui);
}

.ui-context-menu-heading-icon {
  flex-shrink: 0;
  color: var(--text-secondary);
}

.ui-context-menu-heading {
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.ui-context-menu-title {
  overflow: hidden;
  color: var(--text-primary);
  font-size: 12px;
  font-weight: 600;
  line-height: 18px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ui-context-menu-subtitle {
  overflow: hidden;
  color: var(--text-secondary);
  font-size: 11px;
  line-height: 16px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ui-context-menu-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.ui-context-menu-hint {
  flex-shrink: 0;
  margin-left: 16px;
  color: var(--text-secondary);
  font-size: 11px;
}

.ui-context-menu-caret {
  margin-left: 8px;
}

.ui-context-menu-check {
  display: inline-grid;
  place-items: center;
  flex-shrink: 0;
  width: 13px;
  height: 13px;
  color: var(--primary-color);
}

/* One highlight for mouse and keyboard: a resting pointer does not keep a
   second one after the arrow keys move on. */
.ui-context-menu .ui-menu-item:hover:not([data-highlighted]) {
  background: transparent;
}

/* Keyboard focus keeps the app's focus ring, drawn inside the row so it stays
   within the menu frame and does not touch the neighbouring rows. */
.ui-context-menu .ui-menu-item:focus-visible {
  outline-offset: -2px;
}
</style>
