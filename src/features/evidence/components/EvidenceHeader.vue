<template>
<header class="evidence-head">
  <span class="evidence-icon" aria-hidden="true"><component :is="evidenceIcon" :size="20" /></span>
  <div class="evidence-text">
    <div class="evidence-title">
      <h2 class="evidence-name" :title="info.path">{{ fileName }}</h2>
      <span class="ui-badge sm mono">{{ formatLabel }}</span>
    </div>
    <p v-if="info.source_format !== 'assembled'" class="evidence-path">
      <span class="mono">{{ info.path }}</span>
      <button class="ui-icon-btn sm" :title="tr('复制路径')" :aria-label="tr('复制路径')" @click="emit('copy', tr('路径'), info.path)"><Copy :size="12" /></button>
    </p>
    <p class="evidence-size">
      <strong class="mono">{{ formatBytes(info.size_bytes) }}</strong>
      <template v-if="onDiskSummary">{{ ' ' + tr("· 实际占用") }} <span class="mono">{{ onDiskSummary.size }}</span>（{{ onDiskSummary.ratio }}）
      </template>
    </p>
  </div>
  <div class="evidence-actions">
    <AutoDetectBadge :image-id="info.id">
      <button v-if="canExport" class="ui-btn"
        :disabled="exporting" :aria-busy="exporting" :title="tr('导出完整 RAW 镜像；已解锁分区使用解密内容')" @click="exportRawImage">
        <Loader2 v-if="exporting" :size="14" class="spin" />
        <HardDriveDownload v-else :size="14" />{{ tr("导出原始镜像") }}
      </button>
    </AutoDetectBadge>
  </div>
</header>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { Archive, Boxes, Container, Copy, FolderOpen, HardDrive, HardDriveDownload, History, Loader2, LockOpen } from 'lucide-vue-next';
import type { ImageInfo } from '../../../lib/ipc/image';
import { evidenceDisplayName, formatBytes } from '../model/evidenceOverview';
import AutoDetectBadge from './AutoDetectBadge.vue';
import { useImageExport } from '../composables/useImageExport';
import { notifyError } from '../../../composables/useToast';
const props = defineProps<{ info: ImageInfo }>();
const emit = defineEmits<{ copy: [label: string, value: string] }>();

// ── 检材头部 ─────────────────────────────────────────────────────────────
const fileName = computed(() => evidenceDisplayName(props.info, tr));
const LOGICAL_FORMATS = new Set(['folder', 'tar', 'zip', 'sevenz', 'ad1', 'docker', 'android_backup']);
const canExport = computed(() => !props.info.index_only && !props.info.ios_backup && !LOGICAL_FORMATS.has(props.info.source_format));
const { startImageExport, jobFor } = useImageExport();
const exporting = computed(() => !!jobFor(props.info.id));

async function exportRawImage() {
  const info = props.info;
  if (!canExport.value || jobFor(info.id)) return;
  try {
    await startImageExport({
      imageId: info.id,
      defaultFileName: `${fileName.value.replace(/[\\/:*?"<>|]/g, '_').replace(/\s+/g, '_')}.dd`,
      mode: 'unlockedPartitions',
      title: tr("导出原始镜像为 RAW"),
      runningTitle: tr("导出原始镜像中…"),
      doneTitle: tr("原始镜像导出完成"),
    });
  } catch (error) {
    notifyError(tr("导出失败"), String(error));
  }
}

/** 格式徽章：镜像格式写代号（VMDK、EWF），其余写中文名。 */
const FORMAT_LABELS: Record<string, string> = {
  sevenz: '7z',
  docker: 'Docker',
  get folder() { return tr("文件夹"); },
  get physical_disk() { return tr("物理磁盘"); },
  get xiaomi_firmware() { return tr("小米固件"); },
  get android_backup() { return tr("Android 备份"); },
  get ldplayer() { return tr("雷电模拟器备份"); },
  get assembled() { return tr("RAID 重组"); },
  get decrypted() { return tr("解密视图"); },
  get shadow_copy() { return tr("VSS 卷影副本"); },
};
const formatLabel = computed(() => {
  if (props.info.ios_backup) return tr("iOS 备份");
  const format = props.info?.source_format ?? '';
  return FORMAT_LABELS[format] ?? format.toUpperCase();
});

const evidenceIcon = computed(() => {
  switch (props.info?.source_format) {
    case 'folder':
      return FolderOpen;
    case 'zip':
    case 'tar':
    case 'android_backup':
    case 'sevenz':
    case 'ad1':
      return Archive;
    case 'docker':
      return Container;
    case 'decrypted':
      return LockOpen;
    case 'shadow_copy':
      return History;
    case 'assembled':
      return Boxes;
    default:
      return HardDrive;
  }
});

/** 实际占用只在与逻辑大小不同时显示（压缩、稀疏格式）。 */
const onDiskSummary = computed(() => {
  const info = props.info;
  if (!info?.on_disk_bytes || !info.size_bytes || info.on_disk_bytes === info.size_bytes) return null;
  return {
    size: formatBytes(info.on_disk_bytes),
    ratio: `${((info.on_disk_bytes / info.size_bytes) * 100).toFixed(1)}%`,
  };
});


</script>

<style scoped>
/* ── 检材头部 ── */
.evidence-head {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto;
  align-items: start;
  gap: 8px 14px;
  padding: 4px 4px 0;
}
.evidence-icon { display: grid; place-items: center; width: 40px; height: 40px; border-radius: var(--radius-md); background: var(--sev-info-bg); color: var(--primary-color); }
.evidence-text { min-width: 0; }
.evidence-title { display: flex; align-items: center; gap: 8px; min-width: 0; }
.evidence-name { min-width: 0; margin: 0; overflow: hidden; font-size: 16px; font-weight: 600; line-height: 1.4; text-overflow: ellipsis; white-space: nowrap; }
.evidence-path { display: flex; align-items: center; gap: 2px; margin: 2px 0 0; color: var(--text-secondary); font-size: 11px; line-height: 1.6; }
.evidence-path .mono { min-width: 0; overflow-wrap: anywhere; }
.evidence-path .ui-icon-btn { flex-shrink: 0; }
.evidence-size { margin: 2px 0 0; color: var(--text-secondary); font-size: 12px; }
.evidence-size strong { color: var(--text-primary); font-size: 13px; font-weight: 600; font-variant-numeric: tabular-nums; }
.evidence-actions { display: flex; flex-wrap: wrap; align-items: center; justify-content: flex-end; gap: 8px; padding-top: 4px; }

@container image-info (max-width: 640px) {
  .evidence-head { grid-template-columns: auto minmax(0, 1fr); }
  .evidence-actions { grid-column: 2; justify-content: flex-start; padding-top: 0; }
}
</style>
