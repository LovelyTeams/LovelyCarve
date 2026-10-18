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

import { computed } from 'vue';
import FileDetailLayout from '../../../components/layout/FileDetailLayout.vue';
import FileDetailSection from '../../../components/layout/FileDetailSection.vue';
import FileDetailFields from '../../../components/layout/FileDetailFields.vue';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import { formatTimestamp } from '../../../utils/time';
import type { FsNode } from '../../../lib/ipc/fs/types';
import type { FsTypeIdent, StoredDigest } from '../../../lib/ipc/fs/derived';
import { formatPreviewBytes as formatBytes } from '../model/preview';

const props = defineProps<{ node: FsNode; fullPath: string; extension: string; ident: FsTypeIdent | null; digest?: StoredDigest | null; extMismatch: boolean }>();
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
