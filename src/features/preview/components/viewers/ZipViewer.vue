<template>
  <div class="zip-viewer">
    <div v-if="loading" class="state-row mono"><Loader2 :size="14" class="spin" /> {{ tr("正在解析压缩包结构…") }}</div>
    <div v-else-if="error" class="state-row mono error">{{ error }}</div>

    <template v-else-if="report">
      <!-- 概览 -->
      <header class="zip-overview">
        <div class="ov-title">
          <div class="ov-icon"><FileArchive :size="20" /></div>
          <div class="ov-tiles">
            <div class="ov-tile">
              <span class="ov-num mono">{{ report.file_count.toLocaleString() }}</span>
              <span class="ov-label">{{ tr("文件") }}</span>
            </div>
            <div class="ov-tile">
              <span class="ov-num mono">{{ report.dir_count.toLocaleString() }}</span>
              <span class="ov-label">{{ tr("目录") }}</span>
            </div>
            <div class="ov-tile">
              <span class="ov-num mono">{{ formatBytes(report.total_uncompressed) }}</span>
              <span class="ov-label">{{ tr("解压总量") }}</span>
            </div>
            <div class="ov-tile">
              <span class="ov-num mono">{{ formatBytes(report.total_compressed) }}</span>
              <span class="ov-label">{{ tr("压缩总量") }}</span>
            </div>
            <div class="ov-tile">
              <span class="ov-num mono">{{ ratioLabel }}</span>
              <span class="ov-label">{{ tr("压缩率") }}</span>
            </div>
            <div v-if="report.encrypted_count > 0" class="ov-tile warn">
              <span class="ov-num mono"><Lock :size="13" /> {{ report.encrypted_count.toLocaleString() }}</span>
              <span class="ov-label">{{ tr("加密项") }}</span>
            </div>
          </div>
        </div>
        <div v-if="report.comment" class="ov-comment mono" :title="report.comment">{{ tr("注释：") }}{{ report.comment }}</div>
        <div v-for="(w, i) in report.warnings" :key="i" class="ov-warn mono"><AlertTriangle :size="12" /> {{ w }}</div>
      </header>

      <!-- 左树 + 右预览 -->
      <div class="zip-body">
        <ZipMemberTree :report="report" :selected="selected" @select="selectMember" />
        <div class="zip-preview-pane">
          <div v-if="!selected" class="ui-empty member-empty">
            <span class="ui-empty-icon"><FileSearch :size="22" /></span>
            <p class="ui-empty-desc">{{ tr("选择左侧一个文件以预览其内容") }}</p>
          </div>
          <template v-else>
            <div class="member-head">
              <component :is="iconFor(selected)" :size="15" class="mh-icon" :class="iconClass(selected)" />
              <span class="mh-name mono" :title="selected.path">{{ selected.name }}</span>
              <span class="mh-spacer"></span>
              <div v-if="canPreview" class="ui-seg mh-tabs">
                <button class="ui-seg-item" :class="{ active: memberTab === 'text' }" @click="memberTab = 'text'">{{ tr("文本") }}</button>
                <button class="ui-seg-item" :class="{ active: memberTab === 'hex' }" @click="memberTab = 'hex'">{{ tr("十六进制") }}</button>
              </div>
            </div>
            <div class="member-meta mono">
              <span>{{ formatBytes(selected.size) }}</span>
              <span v-if="selected.method">· {{ selected.method }}</span>
              <span v-if="selected.crc32">· CRC {{ selected.crc32 }}</span>
              <span v-if="selected.modified">· {{ formatTimestamp(selected.modified) }}</span>
            </div>

            <!-- 符号链接：只显示目标，不读取内容 -->
            <div v-if="selected.kind === 'symlink'" class="member-note mono">
              {{ tr("符号链接 →") }}<span class="link-target">{{ selected.link_target || tr("（未知目标）") }}</span>
            </div>
            <!-- 加密 / 不支持压缩方法：不可预览 -->
            <div v-else-if="selected.kind === 'other'" class="member-note mono">
              {{ tr("该成员") }}{{ selected.encrypted ? tr("已加密") : tr("使用了未支持的压缩方法") }}{{ tr("，无法解压预览；可导出后用其他工具处理。") }}
            </div>

            <template v-else>
              <div v-if="memberLoading" class="state-row mono"><Loader2 :size="13" class="spin" /> {{ tr("解压中…") }}</div>
              <div v-else-if="memberError" class="state-row mono error">{{ memberError }}</div>
              <template v-else-if="memberBytes">
                <div v-if="memberTruncated" class="trunc-note mono">
                  {{ tr("已截断：仅读取前") }} {{ formatBytes(memberBytes.length) }}{{ tr("（成员共") }} {{ formatBytes(selected.size) }}）
                </div>

                <div v-if="memberTab === 'text'" class="text-view mono">
                  <div v-for="(chunk, ci) in textChunks" :key="ci" class="text-chunk">{{ chunk }}</div>
                </div>

                <div v-else class="hex-view mono">
                  <div v-for="r in hexRows" :key="r.offset" class="hex-row">
                    <span class="hx-off">{{ r.offset }}</span>
                    <span class="hx-bytes">{{ r.bytes }}</span>
                    <span class="hx-ascii">{{ r.ascii }}</span>
                  </div>
                  <div v-if="hexCapped" class="trunc-note mono">
                    {{ tr("十六进制仅显示前") }} {{ formatBytes(HEX_VIEW_CAP) }}{{ tr("；如需完整内容请导出该文件。") }}
                  </div>
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
import { AlertTriangle, FileArchive, FileSearch, Folder, File, Link2, Loader2, Lock } from 'lucide-vue-next';
import type { ZipPreviewEntry } from '../../../../lib/ipc/zip';
import { formatBytes } from '../../../../utils/format';
import { formatTimestamp } from '../../../../utils/time';
import { useZipArchive } from '../../composables/useZipArchive';
import { useZipMember } from '../../composables/useZipMember';
import ZipMemberTree from './ZipMemberTree.vue';
const props = defineProps<{ imageId: string; nodeId: string }>();
const { loading, error, report } = useZipArchive({ imageId: () => props.imageId, nodeId: () => props.nodeId });
const { selected, memberBytes, memberLoading, memberError, memberTab, canPreview, memberTruncated, textChunks, hexCapped, hexRows, HEX_VIEW_CAP, selectMember } = useZipMember({ imageId: () => props.imageId, nodeId: () => props.nodeId });
const ratioLabel = computed(() => {
  const r = report.value;
  if (!r || r.total_uncompressed === 0) return '—';
  const saved = 1 - r.total_compressed / r.total_uncompressed;
  return `${(saved * 100).toFixed(0)}%`;
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
