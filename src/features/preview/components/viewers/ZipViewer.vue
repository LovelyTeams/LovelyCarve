<template>
  <div class="zip-viewer">
    <PreviewState v-if="loading" tone="loading" :title="tr('正在解析压缩包结构…')" />
    <PreviewState v-else-if="error" tone="error" :icon="CircleAlert" :title="tr('无法解析压缩包')" :description="error" mono-description />

    <template v-else-if="report">
      <!-- 概况放进状态栏；注释与解析警告留在内容顶部。 -->
      <PreviewChromeSlot area="statusStart">
        <span class="ui-footbar-stat"><strong>{{ report.file_count.toLocaleString() }}</strong> {{ tr("个文件") }}</span>
        <span class="ui-footbar-stat"><strong>{{ report.dir_count.toLocaleString() }}</strong> {{ tr("个目录") }}</span>
        <span class="ui-footbar-stat" :title="tr('压缩总量 {0}，压缩率 {1}', { 0: formatBytes(report.total_compressed), 1: ratioLabel })">
          {{ tr("解压总量") }} <strong>{{ formatBytes(report.total_uncompressed) }}</strong>
        </span>
        <span v-if="report.encrypted_count > 0" class="ui-badge sm warn"><Lock :size="11" />{{ tr("加密项") }} {{ report.encrypted_count.toLocaleString() }}</span>
      </PreviewChromeSlot>

      <div v-if="report.comment || report.warnings.length" class="zip-notes">
        <p v-if="report.comment" class="zip-comment mono" :title="report.comment">{{ tr("注释：{0}", { 0: report.comment }) }}</p>
        <p v-for="(w, i) in report.warnings" :key="i" class="ui-alert warn" role="note"><AlertTriangle :size="14" /><span>{{ w }}</span></p>
      </div>

      <!-- 成员树 + 成员预览：宽面板左右并排，窄面板上下排列。 -->
      <div class="zip-body">
        <ZipMemberTree :report="report" :selected="selected" @select="selectMember" />
        <div class="zip-preview-pane">
          <PreviewState v-if="!selected" :icon="FileSearch" :title="tr('选择一个文件以预览其内容')" />
          <template v-else>
            <div class="member-head">
              <component :is="iconFor(selected)" :size="16" class="mh-icon" :class="iconClass(selected)" />
              <div class="mh-text">
                <span class="mh-name mono" :title="selected.path">{{ selected.name }}</span>
                <span class="mh-meta mono">{{ memberMeta }}</span>
              </div>
              <div v-if="canPreview" class="ui-seg mh-tabs" role="group" :aria-label="tr('成员视图')">
                <button type="button" class="ui-seg-item" :aria-pressed="memberTab === 'text'" @click="memberTab = 'text'">{{ tr("文本") }}</button>
                <button type="button" class="ui-seg-item" :aria-pressed="memberTab === 'hex'" @click="memberTab = 'hex'">{{ tr("十六进制") }}</button>
              </div>
            </div>

            <!-- 符号链接：只显示目标，不读取内容 -->
            <PreviewState v-if="selected.kind === 'symlink'" :icon="Link2" :title="tr('符号链接')"
              :description="selected.link_target || tr('（未知目标）')" mono-description />
            <!-- 加密 / 不支持压缩方法：不可预览 -->
            <PreviewState v-else-if="selected.kind === 'other'" :icon="Lock" :title="selected.encrypted ? tr('成员已加密') : tr('不支持的压缩方法')" />

            <template v-else>
              <PreviewState v-if="memberLoading" tone="loading" :title="tr('解压中…')" />
              <PreviewState v-else-if="memberError" tone="error" :icon="CircleAlert" :title="tr('解压失败')" :description="memberError" mono-description />
              <template v-else-if="memberBytes">
                <p v-if="memberTruncated" class="ui-alert warn trunc-note" role="note">
                  <AlertTriangle :size="14" />
                  <span>{{ tr("已截断：仅读取前 {0}（成员共 {1}）", { 0: formatBytes(memberBytes.length), 1: formatBytes(selected.size) }) }}</span>
                </p>

                <div v-if="memberTab === 'text'" class="text-view mono">
                  <div v-for="(chunk, ci) in textChunks" :key="ci" class="text-chunk">{{ chunk }}</div>
                </div>

                <div v-else class="hex-view mono">
                  <div v-for="r in hexRows" :key="r.offset" class="hex-row">
                    <span class="hx-off">{{ r.offset }}</span>
                    <span class="hx-bytes">{{ r.bytes }}</span>
                    <span class="hx-ascii">{{ r.ascii }}</span>
                  </div>
                  <p v-if="hexCapped" class="ui-alert warn trunc-note" role="note">
                    <AlertTriangle :size="14" />
                    <span>{{ tr("十六进制仅显示前 {0}", { 0: formatBytes(HEX_VIEW_CAP) }) }}</span>
                  </p>
                </div>
              </template>
            </template>
          </template>
        </div>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

import { computed } from 'vue';
import { AlertTriangle, CircleAlert, FileSearch, Folder, File, Link2, Lock } from 'lucide-vue-next';
import type { ZipPreviewEntry } from '../../../../lib/ipc/zip';
import { formatBytes } from '../../../../utils/format';
import { formatTimestamp } from '../../../../utils/time';
import { useZipArchive } from '../../composables/useZipArchive';
import { useZipMember } from '../../composables/useZipMember';
import ZipMemberTree from './ZipMemberTree.vue';
import PreviewChromeSlot from '../PreviewChromeSlot.vue';
import PreviewState from '../PreviewState.vue';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';
const props = defineProps<{ imageId: ImageId; nodeId: NodeId }>();
const { loading, error, report } = useZipArchive({ imageId: () => props.imageId, nodeId: () => props.nodeId });
const { selected, memberBytes, memberLoading, memberError, memberTab, canPreview, memberTruncated, textChunks, hexCapped, hexRows, HEX_VIEW_CAP, selectMember } = useZipMember({ imageId: () => props.imageId, nodeId: () => props.nodeId });
const ratioLabel = computed(() => {
  const r = report.value;
  if (!r || r.total_uncompressed === 0) return '—';
  const saved = 1 - r.total_compressed / r.total_uncompressed;
  return `${(saved * 100).toFixed(0)}%`;
});

/** 大小、压缩方法、CRC 与修改时间，一行写在成员名下面。 */
const memberMeta = computed(() => {
  const entry = selected.value;
  if (!entry) return '';
  return [
    formatBytes(entry.size),
    entry.method,
    entry.crc32 ? `CRC ${entry.crc32}` : '',
    entry.modified ? formatTimestamp(entry.modified) : '',
  ].filter(Boolean).join(' · ');
});

function iconFor(e: ZipPreviewEntry) {
  if (e.kind === 'dir') return Folder;
  if (e.kind === 'symlink') return Link2;
  return File;
}
function iconClass(e: ZipPreviewEntry) {
  if (e.kind === 'dir') return 'i-dir';
  if (e.kind === 'other') return 'i-locked';
  return 'i-file';
}

</script>

<style scoped src="../../styles/zip-viewer.css"></style>
