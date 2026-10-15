<template>
  <div class="preview-window-root">
    <!-- Custom titlebar (window has decorations: false). data-tauri-drag-region
         makes the strip a drag handle; buttons opt out automatically. -->
    <div class="pw-titlebar" data-tauri-drag-region>
      <div class="pw-title-left" data-tauri-drag-region>
        <FileText :size="14" class="pw-title-icon" />
        <span class="pw-title-text">{{ node?.name ?? tr("文件预览") }}</span>
      </div>
      <div class="pw-title-right">
        <button v-if="ownerId && previewId" class="pw-btn dock-btn" :disabled="docking"
          :title="tr('收回原文件列表的右侧预览栏')" @click="dockWindow">
          <PanelRightClose :size="14" /><span>{{ docking ? tr("正在停靠…") : tr("停靠到右侧") }}</span>
        </button>
        <button class="pw-btn" :title="tr('最小化')" @click="minimizeWindow">
          <Minus :size="14" />
        </button>
        <button class="pw-btn" :title="isMaximized ? tr('还原') : tr('最大化')" @click="toggleMaximizeWindow">
          <component :is="isMaximized ? Square : Maximize2" :size="13" />
        </button>
        <button class="pw-btn close" :title="tr('关闭 (Ctrl+W)')" @click="closeWindow">
          <X :size="14" />
        </button>
      </div>
    </div>

    <div class="pw-body">
      <DiskFileDetail
        v-if="node && imageId"
        ref="detailPreview"
        standalone
        :image-id="imageId"
        :index-only="indexOnly"
        :node="node"
        :reading-state="readingState"
        :full-path="fullPath"
        :ident="ident"
        :digest="digest"
        :focus-target="focusTarget"
        @identified="onIdentified"
        @close="closeWindow"
      />
      <div v-else class="pw-empty mono">{{ tr("无法加载文件预览参数。") }}</div>
    </div>

    <ToastHost />
    <ConfirmDialogHost />
  </div>
</template>

<script setup lang="ts">
import { tr } from '../composables/useLocale';

import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { FileText, Maximize2, Minus, PanelRightClose, Square, X } from 'lucide-vue-next';
import { getCurrentWindow } from '@tauri-apps/api/window';
import DiskFileDetail from '../features/preview/components/DiskFileDetail.vue';
import ToastHost from '../components/feedback/ToastHost.vue';
import ConfirmDialogHost from '../components/feedback/ConfirmDialogHost.vue';
import { useTheme, type ThemeName } from '../composables/useTheme';
import { broadcastPreview, onPreviewBroadcast } from '../features/preview/api';
import { notifyError } from '../composables/useToast';
import type { FsTypeIdent, StoredDigest } from '../lib/ipc/fs/derived';
import type { UnlistenFn } from '../lib/ipc/events';
import { onPreviewSelection, requestPreviewDock, requestPreviewSelection } from '../lib/ipc/previewDock';
import { focusMainWindow } from '../lib/ipc/windows';
import { parseFilePreviewReadingState, type FilePreviewReadingState } from '../lib/models/filePreviewReadingState';

interface FsNode {
  id: string;
  name: string;
  parent_id: string | null;
  kind: 'dir' | 'file' | 'symlink' | 'unallocated';
  size: number;
  deleted: boolean;
  inode: number | null;
  timestamps: {
    modified: string | null;
    accessed: string | null;
    created: string | null;
    mft_changed: string | null;
    birth: string | null;
  };
}

// main.ts already applied the persisted theme before mounting; `setTheme`
// follows later changes made in the main window (see `onStorage`).
const { setTheme } = useTheme();

// ── Parse the launch parameters passed via the window URL ──────────
const params = new URLSearchParams(window.location.search);
const ownerId = params.get('ownerId');
const previewId = params.get('previewId');
const detailPreview = ref<{ getReadingState: () => FilePreviewReadingState } | null>(null);
const readingState = ref<FilePreviewReadingState>();
try { readingState.value = parseFilePreviewReadingState(JSON.parse(params.get('readingState') ?? 'null')); }
catch { /* Invalid optional context does not prevent previewing the file. */ }
const docking = ref(false);
async function dockWindow() {
  if (!ownerId || !previewId || docking.value) return;
  docking.value = true;
  try {
    await requestPreviewDock(ownerId, previewId, detailPreview.value?.getReadingState());
    await focusMainWindow();
    await closeWindow();
  } catch (error) {
    notifyError(tr("停靠失败"), String(error));
  } finally {
    docking.value = false;
  }
}
const imageId = ref(params.get('imageId') ?? '');
const indexOnly = ref(params.get('indexOnly') === 'true');
const fullPath = ref(params.get('fullPath') ?? '');
const node = ref<FsNode | null>(null);
const focusTarget = ref<{ offset: number; length: number }>();
try {
  const value = JSON.parse(params.get('focusTarget') ?? 'null');
  if (value && Number.isSafeInteger(value.offset) && value.offset >= 0 && Number.isSafeInteger(value.length) && value.length > 0) {
    focusTarget.value = { offset: value.offset, length: value.length };
  }
} catch {
  // An invalid optional focus does not prevent opening the file itself.
}
try {
  const raw = params.get('node');
  if (raw) node.value = JSON.parse(raw) as FsNode;
} catch {
  node.value = null;
}

// 预览数据（真实类型 ident、内容哈希 digest）：开窗时由 URL 携带主窗口已知的值
// （列表缓存 / 面板就地识别）作初值，开窗后则经 usePreviewSync 的跨窗口通道随
// 主窗口实时更新——保证内嵌面板与独立窗显示一致。本窗自己识别时回传
// onIdentified，再广播给主窗口（哈希计算只在主窗口发起，本窗只收不发）。
const ident = ref<FsTypeIdent | null>(null);
try {
  const raw = params.get('ident');
  if (raw) ident.value = JSON.parse(raw) as FsTypeIdent;
} catch {
  ident.value = null;
}

const digest = ref<StoredDigest | null>(null);
try {
  const raw = params.get('digest');
  if (raw) digest.value = JSON.parse(raw) as StoredDigest;
} catch {
  digest.value = null;
}

function onIdentified(payload: { nodeId: string; ident: FsTypeIdent }) {
  if (payload.nodeId !== node.value?.id) return;
  ident.value = payload.ident;
  broadcastPreview({ kind: 'ident', imageId: imageId.value, nodeId: payload.nodeId, ident: payload.ident });
}

onPreviewBroadcast((msg) => {
  if (msg.imageId !== imageId.value || msg.nodeId !== node.value?.id) return;
  if (msg.kind === 'ident') ident.value = msg.ident;
  else digest.value = msg.digest;
});

// ── Theme sync with the main window (via shared localStorage) ──────
function onStorage(e: StorageEvent) {
  if (e.key !== 'lovelycarve-theme') return;
  const next = e.newValue;
  if (next === 'light' || next === 'dark' || next === 'pink') {
    setTheme(next as ThemeName);
  }
}

// ── Window controls ────────────────────────────────────────────────
const appWindow = getCurrentWindow();
const isMaximized = ref(false);
let unlistenSelection: UnlistenFn | undefined;
let disposed = false;
let selectionRevision = -1;

async function bindSelection() {
  if (!ownerId || !previewId) return;
  try {
    const cleanup = await onPreviewSelection(ownerId, previewId, selection => {
      if (disposed || !Number.isSafeInteger(selection.revision) || selection.revision <= selectionRevision) return;
      selectionRevision = selection.revision;
      const args = selection.args;
      const changedFile = imageId.value !== args.imageId || node.value?.id !== args.nodeId;
      if (changedFile) readingState.value = parseFilePreviewReadingState(args.readingState);
      imageId.value = args.imageId;
      indexOnly.value = args.indexOnly === true;
      node.value = args.node as FsNode;
      fullPath.value = args.fullPath;
      focusTarget.value = args.focusTarget;
      ident.value = (args.ident as FsTypeIdent | undefined) ?? null;
      digest.value = (args.digest as StoredDigest | undefined) ?? null;
    });
    if (disposed) { cleanup(); return; }
    unlistenSelection = cleanup;
    await requestPreviewSelection(ownerId, previewId);
  } catch (error) {
    if (!disposed) notifyError(tr("预览同步初始化失败"), String(error));
  }
}

watch(() => node.value?.name, name => {
  if (name) void appWindow.setTitle(name).catch(error => notifyError(tr("更新预览标题失败"), String(error)));
}, { immediate: true });

async function refreshMaximized() {
  try {
    isMaximized.value = await appWindow.isMaximized();
  } catch {
    isMaximized.value = false;
  }
}

async function minimizeWindow() {
  try { await appWindow.minimize(); } catch (e) { notifyError(tr("最小化失败"), String(e)); }
}

async function toggleMaximizeWindow() {
  try {
    await appWindow.toggleMaximize();
    await refreshMaximized();
  } catch (e) {
    notifyError(tr("窗口操作失败"), String(e));
  }
}

async function closeWindow() {
  try { await appWindow.close(); } catch (e) { notifyError(tr("关闭失败"), String(e)); }
}

let unlistenResize: UnlistenFn | null = null;
async function bindResizeListener() {
  unlistenResize = await appWindow.onResized(() => { void refreshMaximized(); });
}

function onKeydown(e: KeyboardEvent) {
  const mod = e.ctrlKey || e.metaKey;
  const target = e.target as HTMLElement | null;
  const isTextField = !!target && /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName);
  if (mod && e.key.toLowerCase() === 'w' && !isTextField) {
    e.preventDefault();
    void closeWindow();
  }
}

onMounted(() => {
  void bindSelection();
  void bindResizeListener();
  void refreshMaximized();
  window.addEventListener('storage', onStorage);
  window.addEventListener('keydown', onKeydown);
});

onBeforeUnmount(() => {
  disposed = true;
  unlistenSelection?.();
  window.removeEventListener('storage', onStorage);
  window.removeEventListener('keydown', onKeydown);
  unlistenResize?.();
  unlistenResize = null;
});
</script>

<style scoped>
.preview-window-root {
  display: flex;
  flex-direction: column;
  height: 100vh;
  background: var(--bg-primary);
  color: var(--text-primary);
  font-family: var(--font-ui);
  font-size: 13px;
}

.pw-titlebar {
  display: flex;
  align-items: stretch;
  height: 38px;
  padding: 0 4px 0 12px;
  background: var(--surface-primary);
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
  user-select: none;
  -webkit-user-select: none;
}

.pw-title-left {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: 1;
  min-width: 0;
}

.pw-title-icon { color: var(--primary-color); flex-shrink: 0; }

.pw-title-text {
  color: var(--text-primary);
  font-weight: 600;
  font-size: 12px;
  font-family: var(--font-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pw-title-right {
  display: flex;
  align-items: stretch;
  flex-shrink: 0;
}

.pw-btn {
  width: 44px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: 0;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast);
}
.pw-btn:hover {
  background: var(--hover-bg);
  color: var(--text-primary);
}
.pw-btn.close:hover {
  background: var(--danger-solid);
  color: var(--accent-contrast);
}
/* Dock action reads like the main window's titlebar chips. */
.pw-btn.dock-btn {
  width: auto;
  height: 26px;
  align-self: center;
  margin-right: 6px;
  padding: 0 10px;
  gap: 6px;
  border: 1px solid color-mix(in srgb, var(--primary-color) 45%, var(--border-color));
  border-radius: var(--radius-md);
  font: inherit;
  font-size: 12px;
  color: var(--primary-color);
}
.pw-btn.dock-btn:hover:not(:disabled) { background: var(--sidebar-active-bg); color: var(--primary-color); }
.pw-btn:disabled { opacity: .5; cursor: wait; }

.pw-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.pw-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-secondary);
}
</style>
