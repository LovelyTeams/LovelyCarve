<template>
  <div class="vimswap-viewer">
    <div v-if="loading" class="vsw-state mono">
      <Loader2 :size="16" class="spin" />
      <span>{{ tr("正在解析 Vim Swap (.swp) 未保存缓冲区…") }}</span>
    </div>
    <div v-else-if="error" class="vsw-state error mono">
      <AlertTriangle :size="16" />
      <span>{{ error }}</span>
    </div>
    <template v-else-if="digest">
      <header class="vsw-header">
        <div class="vsw-title-row">
          <FileCode2 :size="15" class="vsw-icon" />
          <span class="vsw-fname mono">{{ digest.meta.fname || tr("未知原文件") }}</span>
          <span class="ui-badge sm mono">Vim {{ digest.meta.version }}</span>
          <span v-if="digest.encrypted" class="ui-badge sm warn">{{ tr("已加密") }}</span>
          <span class="ui-badge sm mono">{{ digest.lines.length }} {{ tr("行") }}</span>
          <button type="button" class="ui-btn sm mono vsw-copy" :disabled="digest.lines.length === 0" @click="copyAllLines">
            <Copy :size="12" />
            <span>{{ tr("复制恢复内容") }}</span>
          </button>
        </div>

        <div class="vsw-meta-grid mono">
          <span>{{ tr("用户") }}: {{ digest.meta.user || '—' }}</span>
          <span>{{ tr("主机") }}: {{ digest.meta.host || '—' }}</span>
          <span>PID: {{ digest.meta.pid }}</span>
          <span>inode: {{ digest.meta.inode }}</span>
          <span>{{ tr("修改时间") }}: {{ digest.meta.mtime_iso || digest.meta.mtime || '—' }}</span>
          <span>{{ tr("页大小") }}: {{ digest.meta.page_size }} B</span>
          <span>{{ tr("数据块/指针块/未知块") }}: {{ digest.data_block_count }} / {{ digest.pointer_block_count }} / {{ digest.unknown_block_count }}</span>
        </div>

        <div v-if="digest.encrypted" class="ui-alert warn vsw-alert">
          {{ tr("检测到 Vim 加密交换文件（header 可读但数据块已加密），暂无法明文提取未保存行。") }}
        </div>
        <div v-for="(note, i) in digest.notes" :key="i" class="vsw-note mono">
          {{ note }}
        </div>
      </header>

      <div class="vsw-lines mono">
        <div v-if="filteredLines.length === 0" class="vsw-empty">
          {{ query ? tr("无匹配关键词的恢复行") : tr("未提取到文本行") }}
        </div>
        <div v-for="entry in filteredLines" :key="entry.idx" class="vsw-line">
          <span class="vsw-ln">{{ entry.idx }}</span>
          <span class="vsw-text">
            <template v-if="!trimmedQuery">{{ entry.text }}</template>
            <template v-else>
              <template v-for="(part, pIdx) in splitHighlight(entry.text)" :key="pIdx">
                <mark v-if="part.hit" class="vsw-mark">{{ part.text }}</mark>
                <template v-else>{{ part.text }}</template>
              </template>
            </template>
          </span>
        </div>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { AlertTriangle, Copy, FileCode2, Loader2 } from 'lucide-vue-next';
import { tr } from '../../../../composables/useLocale';
import { copyText } from '../../../../composables/clipboard';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';
import { fsVimSwapPreview, type VimSwapDigest } from '../../../../lib/ipc/forensicPreview';

const props = defineProps<{
  imageId: ImageId;
  nodeId: NodeId;
  query?: string;
}>();

const loading = ref(false);
const error = ref<string | null>(null);
const digest = ref<VimSwapDigest | null>(null);
let fetchGen = 0;

watch(
  () => [props.imageId, props.nodeId] as const,
  async ([imageId, nodeId]) => {
    const gen = ++fetchGen;
    loading.value = true;
    error.value = null;
    digest.value = null;
    try {
      const res = await fsVimSwapPreview({ imageId, nodeId });
      if (gen === fetchGen) digest.value = res;
    } catch (err) {
      if (gen === fetchGen) error.value = String(err);
    } finally {
      if (gen === fetchGen) loading.value = false;
    }
  },
  { immediate: true },
);

const trimmedQuery = computed(() => (props.query ?? '').trim());

const filteredLines = computed(() => {
  const lines = digest.value?.lines ?? [];
  const q = trimmedQuery.value.toLowerCase();
  const mapped = lines.map((text, i) => ({ idx: i + 1, text }));
  if (!q) return mapped;
  return mapped.filter(item => item.text.toLowerCase().includes(q));
});

function splitHighlight(text: string): Array<{ text: string; hit: boolean }> {
  const q = trimmedQuery.value;
  if (!q) return [{ text, hit: false }];
  const lower = text.toLowerCase();
  const needle = q.toLowerCase();
  const parts: Array<{ text: string; hit: boolean }> = [];
  let pos = 0;
  while (pos < text.length) {
    const idx = lower.indexOf(needle, pos);
    if (idx < 0) {
      parts.push({ text: text.slice(pos), hit: false });
      break;
    }
    if (idx > pos) parts.push({ text: text.slice(pos, idx), hit: false });
    parts.push({ text: text.slice(idx, idx + needle.length), hit: true });
    pos = idx + needle.length;
  }
  return parts;
}

function copyAllLines() {
  if (!digest.value?.lines.length) return;
  void copyText(digest.value.lines.join('\n'), tr('Vim Swap 恢复内容'));
}
</script>

<style scoped>
.vimswap-viewer {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--surface-primary);
}
.vsw-state {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 28px;
  color: var(--text-secondary);
}
.vsw-state.error {
  color: var(--sev-critical-fg);
}
.vsw-header {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-color);
  background: var(--surface-secondary);
  flex-shrink: 0;
}
.vsw-title-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
}
.vsw-icon { color: var(--primary-color); flex-shrink: 0; }
.vsw-fname { font-weight: 600; font-size: 12px; color: var(--text-primary); }
.vsw-copy { margin-left: auto; }
.vsw-meta-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 14px;
  font-size: 11px;
  color: var(--text-secondary);
}
.vsw-alert { font-size: 12px; padding: 6px 10px; }
.vsw-note { font-size: 11px; color: var(--text-secondary); }
.vsw-lines {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 10px 14px;
  font-size: 12px;
  line-height: 1.5;
  user-select: text;
}
.vsw-line {
  display: flex;
  align-items: baseline;
  gap: 12px;
  white-space: pre-wrap;
  word-break: break-all;
}
.vsw-line:hover { background: color-mix(in srgb, var(--primary-color) 6%, transparent); }
.vsw-ln {
  min-width: 34px;
  text-align: right;
  font-size: 11px;
  color: var(--text-secondary);
  user-select: none;
}
.vsw-text { flex: 1; min-width: 0; }
.vsw-mark { background: var(--sev-warn-tag-bg); color: var(--sev-warn-tag-fg); border-radius: 2px; padding: 0 1px; }
.vsw-empty { padding: 24px; text-align: center; color: var(--text-secondary); }
</style>
