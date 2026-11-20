<template>
  <div class="report-viewer vim-swap-viewer">
    <PreviewState v-if="loading" tone="loading" :title="tr('正在解析 Vim 交换文件…')" />
    <PreviewState v-else-if="error" tone="error" :icon="CircleAlert" :title="tr('无法解析 Vim 交换文件')" :description="error" mono-description />

    <template v-else-if="digest">
      <header class="report-head">
        <div class="report-icon"><FilePen :size="20" /></div>
        <div class="report-title">
          <h3 class="report-name">{{ digest.meta.fname || tr("未记录原文件名") }}</h3>
          <div class="report-badges">
            <span v-if="digest.meta.version" class="ui-badge sm mono">{{ digest.meta.version }}</span>
            <span v-if="digest.encrypted" class="ui-badge sm warn">{{ tr("疑似加密") }}</span>
            <span v-if="digest.truncated" class="ui-badge sm warn">{{ tr("已截断") }}</span>
          </div>
        </div>
      </header>

      <dl class="report-fields">
        <div><dt>{{ tr("用户") }}</dt><dd>{{ digest.meta.user || '—' }}</dd></div>
        <div><dt>{{ tr("主机") }}</dt><dd>{{ digest.meta.host || '—' }}</dd></div>
        <div><dt>PID</dt><dd>{{ digest.meta.pid }}</dd></div>
        <div><dt>inode</dt><dd>{{ digest.meta.inode }}</dd></div>
        <div><dt>{{ tr("修改时间") }}</dt><dd>{{ modified }}</dd></div>
        <div><dt>{{ tr("页大小") }}</dt><dd>{{ formatBytes(digest.meta.page_size) }}</dd></div>
        <div><dt>{{ tr("数据块") }}</dt><dd>{{ digest.data_block_count.toLocaleString() }}</dd></div>
        <div><dt>{{ tr("指针块") }}</dt><dd>{{ digest.pointer_block_count.toLocaleString() }}</dd></div>
        <div><dt>{{ tr("未知块") }}</dt><dd>{{ digest.unknown_block_count.toLocaleString() }}</dd></div>
        <div><dt>{{ tr("读取字节") }}</dt><dd>{{ formatBytes(digest.bytes_read) }}</dd></div>
      </dl>

      <section v-if="digest.notes.length" class="ui-alert warn report-warnings" role="note">
        <AlertTriangle :size="14" />
        <div class="warn-body">
          <p class="warn-title">{{ tr("解析警告") }}</p>
          <p v-for="(note, index) in digest.notes" :key="index" class="warn-line mono">{{ note }}</p>
        </div>
      </section>

      <section class="section lines">
        <div class="section-head">
          <h4 class="section-title">{{ tr("恢复的行") }} <span class="ui-count">{{ digest.lines.length.toLocaleString() }}</span></h4>
          <template v-if="digest.lines.length">
            <label class="ui-search section-filter">
              <Search :size="13" aria-hidden="true" />
              <input v-model="query" type="text" :placeholder="tr('高亮关键词')" :aria-label="tr('高亮关键词')" spellcheck="false" />
            </label>
            <button class="ui-icon-btn sm" type="button" :title="tr('复制恢复的行')" :aria-label="tr('复制恢复的行')" @click="copyLines"><Copy :size="14" /></button>
          </template>
        </div>
        <CodeViewer v-if="digest.lines.length" class="lines-code" :code="code" :filename="digest.meta.fname" :dark="dark" :query="query" />
        <div v-else class="empty-row">{{ tr("没有恢复出文本行") }}</div>
      </section>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

// Vim 交换文件（.swp / .swo / … 以 `b0VIM ` 开头）的「原类型视图」：fs_vim_swap_preview
// 解析块 0 的原文件、用户、主机与进程，并按数据块取出未保存缓冲区的行。行按块内的
// 存放顺序列出，用原文件名选择语法高亮。
import { computed, defineAsyncComponent, ref, watch } from 'vue';
import { AlertTriangle, CircleAlert, Copy, FilePen, Search } from 'lucide-vue-next';
import { copyText } from '../../../../composables/clipboard';
import { fsVimSwapPreview, type VimSwapDigest } from '../../../../lib/ipc/vimSwap';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';
import { formatBytes } from '../../../../utils/format';
import { formatTimestamp } from '../../../../utils/time';
import PreviewState from '../PreviewState.vue';

const CodeViewer = defineAsyncComponent(() => import('./CodeViewer.vue'));

const props = withDefaults(defineProps<{ imageId: ImageId; nodeId: NodeId; dark?: boolean }>(), { dark: false });

const loading = ref(false);
const error = ref<string | null>(null);
const digest = ref<VimSwapDigest | null>(null);
const query = ref('');
let generation = 0;

const code = computed(() => digest.value?.lines.join('\n') ?? '');
/** The swap header stores seconds since the epoch, so the time is UTC. */
const modified = computed(() => {
  const iso = digest.value?.meta.mtime_iso;
  return iso ? `${formatTimestamp(iso)} UTC` : '—';
});

async function load() {
  const current = ++generation;
  loading.value = true;
  error.value = null;
  digest.value = null;
  query.value = '';
  try {
    const result = await fsVimSwapPreview({ imageId: props.imageId, nodeId: props.nodeId });
    if (current === generation) digest.value = result;
  } catch (err) {
    if (current === generation) error.value = String(err);
  } finally {
    if (current === generation) loading.value = false;
  }
}

function copyLines() {
  if (code.value) void copyText(code.value, tr("恢复的行"));
}

watch(() => [props.imageId, props.nodeId], load, { immediate: true });
</script>

<style scoped src="../../../../styles/report-viewer.css"></style>
<style scoped>
.lines-code {
  height: min(60vh, 560px);
  min-height: 240px;
  overflow: hidden;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
}
</style>
