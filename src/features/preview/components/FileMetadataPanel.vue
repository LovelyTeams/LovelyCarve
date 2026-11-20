<template>
  <FileDetailLayout :title="node.name">
    <template #icon><FileNodeIcon :id="node.id" :kind="node.kind" :name="node.name" :size="20" /></template>
    <template #badges><span class="ui-badge">{{ kindLabel }}</span><span class="ui-badge" :class="node.deleted ? 'warn' : ''">{{ node.deleted ? tr("已删除") : tr("未删除") }}</span></template>
    <FileDetailSection :title="tr('基本信息')">
      <FileDetailFields :fields="[
        { label: tr('路径'), value: fullPath, copyable: true },
        { label: tr('扩展名'), value: extension, mono: true },
        { label: tr('大小'), value: node.kind === 'dir' && !/^(part:|lvm:)/.test(node.id) ? '—' : formatBytes(node.size), mono: true },
        ...(node.link_target ? [{ label: tr('链接指向'), value: node.link_target, copyable: true }] : []),
      ]" />
    </FileDetailSection>
    <FileDetailSection :title="tr('时间信息')">
      <FileDetailFields :fields="[
        { label: tr('创建时间'), value: formatTimestamp(node.timestamps?.created || node.timestamps?.birth), mono: true },
        { label: tr('修改时间'), value: formatTimestamp(node.timestamps?.modified), mono: true },
        { label: tr('访问时间'), value: formatTimestamp(node.timestamps?.accessed), mono: true },
      ]" />
    </FileDetailSection>
    <FileDetailSection v-if="ident" :title="tr('类型识别')" wide>
      <FileDetailFields :fields="[
        { label: tr('识别类型'), value: ident.label, mono: true },
        { label: tr('说明'), value: ident.description },
        { label: tr('判定依据'), value: ident.inferred ? tr('置信度 {0}%', { 0: (ident.score * 100).toFixed(1) }) : tr('规则判定') },
      ]" />
      <p v-if="extMismatch" class="ui-alert warn detail-alert">{{ tr("扩展名 .{0} 与识别类型（{1}）不符，可能是修改扩展名伪装的文件。", { 0: extension, 1: ident.label }) }}</p>
    </FileDetailSection>
    <FileDetailSection v-if="streams.length" :title="tr('NTFS 备用数据流 (ADS) ({0})', { 0: streams.length })" wide>
      <div class="ads-list">
        <div v-for="s in streams" :key="s.name" class="ads-item">
          <div class="ads-row mono">
            <span class="ads-stream-name">:{{ s.name }}</span>
            <span class="ui-badge sm">{{ s.resident ? tr("常驻") : tr("非常驻") }}</span>
            <span>{{ formatBytes(s.size) }}</span>
            <div class="ads-actions">
              <button type="button" class="ui-btn sm" :disabled="streamBusyName === s.name" @click="previewStream(s.name)">
                {{ tr("预览") }}
              </button>
              <button type="button" class="ui-btn sm" :disabled="streamBusyName === s.name" @click="exportStream(s.name)">
                {{ tr("导出") }}
              </button>
            </div>
          </div>
          <pre v-if="streamPreviews[s.name]" class="ads-preview mono">{{ streamPreviews[s.name] }}</pre>
        </div>
      </div>
    </FileDetailSection>
    <FileDetailSection v-if="hashRows.length" :title="tr('文件哈希')" wide>
      <FileDetailFields :fields="hashRows.map(row => ({ ...row, mono: true, copyable: true }))" />
    </FileDetailSection>
    <FileDetailSection :key="node.id" :title="tr('技术信息')" collapsible wide>
      <FileDetailFields :fields="[
        { label: tr('元数据地址'), value: node.inode, mono: true, copyable: true },
        { label: tr('加密类型'), value: node.kind === 'file' ? tr('未检测') : '—' },
      ]" />
    </FileDetailSection>
  </FileDetailLayout>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, ref, watch } from 'vue';
import FileDetailLayout from '../../../components/layout/FileDetailLayout.vue';
import FileDetailSection from '../../../components/layout/FileDetailSection.vue';
import FileDetailFields from '../../../components/layout/FileDetailFields.vue';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import { formatTimestamp } from '../../../utils/time';
import type { FsNode } from '../../../lib/ipc/fs/types';
import type { ImageId } from '../../../lib/ipc/ids';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import type { FsTypeIdent, StoredDigest } from '../../../lib/ipc/fs/derived';
import { fsStreams, fsStreamPreview, fsStreamExport, type NtfsDataStream } from '../../../lib/ipc/fs/streams';
import { notifyError, notifySuccess } from '../../../composables/useToast';
import { formatPreviewBytes as formatBytes } from '../model/preview';

const props = defineProps<{
  imageId?: ImageId;
  indexOnly?: boolean;
  node: FsNode;
  fullPath: string;
  extension: string;
  ident: FsTypeIdent | null;
  digest?: StoredDigest | null;
  extMismatch: boolean;
}>();

const streams = ref<NtfsDataStream[]>([]);
const streamBusyName = ref<string | null>(null);
const streamPreviews = ref<Record<string, string>>({});
let streamsFetchGen = 0;

watch(
  () => [props.imageId, props.node.id, props.node.kind, props.indexOnly] as const,
  async ([imageId, nodeId, kind, indexOnly]) => {
    const gen = ++streamsFetchGen;
    streams.value = [];
    streamPreviews.value = {};
    streamBusyName.value = null;
    if (!imageId || indexOnly || kind !== 'file' || /^(?:diskgap|unalloc):/.test(nodeId)) return;
    try {
      const list = await fsStreams({ imageId, nodeId });
      if (gen === streamsFetchGen) streams.value = Array.isArray(list) ? list : [];
    } catch {
      if (gen === streamsFetchGen) streams.value = [];
    }
  },
  { immediate: true },
);

async function previewStream(streamName: string) {
  if (!props.imageId) return;
  streamBusyName.value = streamName;
  try {
    const bytes = await fsStreamPreview({
      imageId: props.imageId,
      nodeId: props.node.id,
      streamName,
      maxBytes: 65536,
    });
    const arr = new Uint8Array(bytes);
    let text = '';
    if (!arr.includes(0)) {
      try {
        text = new TextDecoder('utf-8', { fatal: false }).decode(arr);
      } catch {
        text = '';
      }
    }
    if (!text) {
      text = Array.from(arr.slice(0, 512), b => b.toString(16).padStart(2, '0')).join(' ');
    }
    streamPreviews.value = { ...streamPreviews.value, [streamName]: text || tr('（空数据流）') };
  } catch (err) {
    notifyError(tr('数据流预览失败'), String(err));
  } finally {
    if (streamBusyName.value === streamName) streamBusyName.value = null;
  }
}

async function exportStream(streamName: string) {
  if (!props.imageId) return;
  const cleanStream = streamName.replace(/[^a-zA-Z0-9._-]/g, '_');
  const destination = await saveDialog({
    title: tr('导出命名数据流 :{0}', { 0: streamName }),
    defaultPath: `${props.node.name}_${cleanStream || 'stream'}.bin`,
  });
  if (typeof destination !== 'string' || !destination) return;
  streamBusyName.value = streamName;
  try {
    const written = await fsStreamExport({
      imageId: props.imageId,
      nodeId: props.node.id,
      streamName,
      destination,
    });
    notifySuccess(tr('数据流导出成功'), `${written.toLocaleString()} B → ${destination}`);
  } catch (err) {
    notifyError(tr('数据流导出失败'), String(err));
  } finally {
    if (streamBusyName.value === streamName) streamBusyName.value = null;
  }
}

const kindLabel = computed(() => ({ dir: tr("目录"), file: tr("文件"), symlink: tr("符号链接"), unallocated: tr("未分配") }[props.node.kind]));
const hashRows = computed(() => {
  const digest = props.digest;
  if (!digest) return [];
  return [
    { key: 'md5', label: 'MD5', value: digest.md5 }, { key: 'sha1', label: 'SHA1', value: digest.sha1 },
    { key: 'sha256', label: 'SHA256', value: digest.sha256 }, { key: 'sm3', label: 'SM3', value: digest.sm3 },
  ].filter((row): row is { key: string; label: string; value: string } => !!row.value);
});
</script>

<style scoped src="../../../styles/analysis-detail.css"></style>
<style scoped>
.ads-list { display: grid; gap: 6px; }
.ads-item { display: grid; gap: 4px; padding: 6px 8px; border-radius: var(--radius-sm); background: var(--surface-secondary); border: 1px solid var(--border-color); }
.ads-row { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; font-size: 12px; }
.ads-stream-name { color: var(--primary-color); font-weight: 600; }
.ads-actions { margin-left: auto; display: flex; gap: 6px; }
.ads-preview { margin: 0; padding: 6px 8px; border-radius: var(--radius-sm); background: var(--surface-primary); border: 1px solid var(--border-color); font-size: 11px; white-space: pre-wrap; word-break: break-all; max-height: 180px; overflow: auto; }
</style>
