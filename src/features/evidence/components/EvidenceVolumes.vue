<template>
<section ref="sectionEl" class="ui-card volume-card" data-section="volumes" aria-labelledby="volumes-title">
  <header class="ui-card-header">
    <h3 id="volumes-title" class="ui-card-title"><Layers :size="15" />{{ isFirmwareContainer ? tr("固件段布局") : tr("卷与分区") }}</h3>
    <span v-if="schemeLabel" class="ui-badge sm mono">{{ schemeLabel }}</span>
    <span v-if="info.partitions.length > 0" class="ui-count">{{ info.partitions.length }}</span>
    <button v-if="canUnlockContainer" class="ui-btn sm head-action" type="button" @click="emit('unlock', null)">
      <LockKeyhole :size="13" />{{ tr("解锁加密容器") }}
    </button>
  </header>

  <!-- 磁盘图：斜纹底即未分配空间，色块与分区行同色，点击色块定位到行。 -->
  <div v-if="layoutSegments.length > 0" class="disk-map">
    <div class="layout-bar ui-stagger" role="group" :aria-label="tr('磁盘布局，点击色块定位分区')">
      <button
        v-for="seg in layoutSegments"
        :key="seg.key"
        class="layout-seg"
        :class="{ active: highlightedPartition === seg.partition.id, hover: hoveredPartition === seg.partition.id }"
        :style="{ left: `${seg.leftPct}%`, width: `${seg.widthPct}%`, '--seg-color': seg.color }"
        :title="seg.tooltip"
        :aria-label="seg.tooltip"
        @click="scrollToPartition(seg.partition.id)"
        @mouseenter="hoveredPartition = seg.partition.id"
        @mouseleave="hoveredPartition = null"
      ><span v-if="seg.widthPct >= 14" class="seg-label">{{ partitionTitle(seg.partition, tr) }}<small class="mono">{{ formatBytes(seg.lenBytes) }}</small></span></button>
    </div>
    <p v-if="unallocatedBytes > 0" class="disk-map-note">
      <span class="unalloc-swatch" aria-hidden="true"></span>{{ tr("未分配") }} <span class="mono">{{ formatBytes(unallocatedBytes) }}</span>
    </p>
  </div>

  <div v-if="info.partitions.length === 0 && syntheticRoot" class="synthetic-root-mount">
    <div class="synthetic-root-row">
      <span class="synthetic-root-title">{{ syntheticRoot.label }}</span>
      <EvidenceMountControl :view="mountViewFor(syntheticRoot.id)" :busy="busyPartitions.has(SYNTHETIC_MOUNT_KEY)" :share-busy="!!mountViewFor(syntheticRoot.id) && busyShares.has(mountViewFor(syntheticRoot.id)!.id)"
        :mount-target="mountTarget" :picker-loading="pickerLoading" :picker-open="pickerOpenFor === SYNTHETIC_MOUNT_KEY" :available-drives="availableDrives"
        @toggle="beginMount(syntheticRoot.id, SYNTHETIC_MOUNT_KEY, syntheticRoot.label)" @share="toggleShare(mountViewFor(syntheticRoot.id)!, syntheticRoot.label)"
        @mount="drive => syntheticRoot && finishMountAt(syntheticRoot.id, SYNTHETIC_MOUNT_KEY, syntheticRoot.label, drive)" @copy="value => emit('copy', tr('共享名'), value)" />
    </div>
    <p class="synthetic-root-hint">
      {{ tr("逻辑检材以单一合成根呈现（无分区表），可只读挂载为 Windows 盘符或文件夹后用资源管理器浏览。") }}
    </p>
  </div>
  <div v-else-if="info.partitions.length === 0" class="ui-empty volume-empty">
    <span class="ui-empty-icon"><Layers :size="24" /></span>
    <p class="ui-empty-title">{{ tr("未检测到分区") }}</p>
  </div>
  <!-- 分区表：每个分区一行。结束扇区、扇区数和字节偏移在起始扇区的悬停提示里；
       卷影副本在所在分区行下展开。 -->
  <div v-else class="partition-table" role="table" aria-labelledby="volumes-title">
    <div class="part-head" role="row">
      <span class="cell col-name" role="columnheader">{{ tr("名称") }}</span>
      <span class="cell col-fs" role="columnheader">{{ isFirmwareContainer ? tr("类型") : tr("文件系统") }}</span>
      <span class="cell col-start" role="columnheader">{{ tr("起始") }}{{ addressUnit }}</span>
      <span class="cell col-size" role="columnheader">{{ tr("大小") }}</span>
      <span class="cell col-actions" role="columnheader"><span class="sr-only">{{ tr("操作") }}</span></span>
    </div>
    <div class="part-body ui-stagger" role="rowgroup">
      <template v-for="row in partitionRows" :key="`${info.id}:${row.partition.id}`">
        <div
          :id="`part-row-${row.partition.id}`"
          class="partition-row"
          :class="{ active: highlightedPartition === row.partition.id, hover: hoveredPartition === row.partition.id, quiet: QUIET_KINDS.has(row.partition.kind) }"
          :style="{ '--part-color': row.color }"
          role="row"
          tabindex="-1"
          @mouseenter="hoveredPartition = row.partition.id"
          @mouseleave="hoveredPartition = null"
        >
          <span class="cell col-name" role="cell">
            <span class="part-swatch" aria-hidden="true"></span>
            <span class="part-label" :title="partitionTitle(row.partition, tr)">{{ partitionTitle(row.partition, tr) }}</span>
            <span v-if="row.partition.label" class="part-index mono">#{{ row.partition.id }}</span>
          </span>
          <span class="cell col-fs" role="cell"><span class="ui-badge sm mono">{{ KIND_LABELS[row.partition.kind] || row.partition.kind }}</span><span v-if="info.unlocked_partitions?.includes(row.partition.id)" class="ui-badge sm success">{{ tr("已解锁") }}</span></span>
          <span class="cell col-start mono" role="cell" :title="geometryTitle(row.partition)"><span class="cell-label">{{ addressUnit }}</span>{{ row.partition.start_lba.toLocaleString() }}</span>
          <span class="cell col-size mono" role="cell">{{ formatBytes(row.partition.length_lba * info.sector_size) }}</span>
          <div class="cell col-actions" role="cell">
            <template v-if="row.shadow">
              <span
                v-if="!row.shadow.error && row.shadow.shadow_copies.length === 0"
                class="shadow-none"
                :title="tr('该卷有 VSS 区，但没有卷影副本（未创建，或已被删除）')"
              >{{ tr("无卷影副本") }}</span>
              <button
                v-else
                class="ui-btn sm ghost shadow-toggle"
                :class="{ active: expandedShadows.has(row.partition.id), failed: row.shadow.error }"
                type="button"
                :aria-expanded="expandedShadows.has(row.partition.id)"
                :aria-controls="expandedShadows.has(row.partition.id) ? `shadow-list-${row.partition.id}` : undefined"
                @click="toggleShadow(row.partition.id)"
              >
                <TriangleAlert v-if="row.shadow.error" :size="13" />
                <History v-else :size="13" />
                <span>{{ row.shadow.error ? tr("卷影副本读取失败") : tr("卷影副本 {0}", { 0: row.shadow.shadow_copies.length }) }}</span>
                <ChevronDown :size="13" class="shadow-chevron" />
              </button>

</template>
                  <BitlockerUnlockControl
                    v-if="row.partition.kind === 'bitlocker'"
                    :image-id="info.id"
                    :partition="row.partition"
                    @unlocked="onBitlockerUnlocked"
                  />
                  <button
                    v-if="['luks', 'filevault'].includes(row.partition.kind) && canUnlockContainer"
                    class="ui-btn sm"
                    type="button"
                    :title="tr('解锁该加密分区并以只读方式浏览文件')"
                    @click="emit('unlock', row.partition.id)"
                  >
                    <KeyRound :size="13" />{{ tr("解锁") }}
                  </button>
                  <EvidenceMountControl v-if="isMountable(row.partition) && canShowMount(row.partition)" :view="mountFor(row.partition)"
                    :busy="busyPartitions.has(row.partition.id)" :share-busy="!!mountFor(row.partition) && busyShares.has(mountFor(row.partition)!.id)"
                    :mount-target="mountTarget" :picker-loading="pickerLoading" :picker-open="pickerOpenFor === row.partition.id" :available-drives="availableDrives"
                    @toggle="toggleMount(row.partition)" @share="toggleShare(mountFor(row.partition)!, partitionTitle(row.partition, tr))"
                    @mount="drive => mountAt(row.partition, drive)" @copy="value => emit('copy', tr('共享名'), value)" />
                </div>
              </div>
              <div
                v-if="row.shadow && expandedShadows.has(row.partition.id)"
                :id="`shadow-list-${row.partition.id}`"
                class="shadow-row"
                role="row"
              >
                <div class="cell" role="cell" aria-colspan="5">
                  <ShadowCopyList
                    :volume="row.shadow"
                    :opening="shadowOpening"
                    :error="shadowOpenError?.partitionId === row.partition.id ? shadowOpenError.message : null"
                    @open="copy => openShadowCopy(row.shadow!, copy)"
                  />
                </div>
              </div>
            </template>
          </div>
        </div>
        <p v-if="shadowLoadError" class="ui-alert danger shadow-alert" role="alert">
          <span>{{ tr("读取卷影副本失败：{0}", { 0: shadowLoadError }) }}</span>
          <button class="ui-btn sm" type="button" @click="loadShadowCopies()">{{ tr("重试") }}</button>
        </p>
      </section>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, nextTick, onScopeDispose, ref } from 'vue';
import { ChevronDown, History, KeyRound, Layers, LockKeyhole, TriangleAlert } from 'lucide-vue-next';
import type { ImageInfo, DiskPartition } from '../../../lib/ipc/image';
import type { ShadowCopyInfo, ShadowVolumeInfo } from '../../../lib/ipc/shadow';
import { useUiPreferences } from '../../../composables/useUiPreferences';
import { useEvidenceMounts } from '../composables/useEvidenceMounts';
import { useShadowCopies } from '../composables/useShadowCopies';
import { KIND_LABELS, QUIET_KINDS, SCHEME_LABELS, evidenceLayout, formatBytes, partColor, partitionGeometry, partitionTitle } from '../model/evidenceOverview';
import EvidenceMountControl from './EvidenceMountControl.vue';
import BitlockerUnlockControl from './BitlockerUnlockControl.vue';
import ShadowCopyList from './ShadowCopyList.vue';
const props = defineProps<{ info: ImageInfo; canUnlockContainer: boolean; caseDirectory?: string | null }>();
const emit = defineEmits<{ unlock: [partitionId: number | null]; 'open-image': [path: string]; copy: [label: string, value: string] }>();
const sectionEl = ref<HTMLElement | null>(null);
let disposed = false;
onScopeDispose(() => { disposed = true; });

const highlightedPartition = ref<number | null>(null);
/** 磁盘图色块与分区行互相高亮（悬停任一侧，另一侧同步）。 */
const hoveredPartition = ref<number | null>(null);

const { mountTarget, mountFolderRoot } = useUiPreferences();

const isFirmwareContainer = computed(() => props.info?.source_format === 'xiaomi_firmware');
/** 分区起点的单位：固件段按字节寻址。 */
const addressUnit = computed(() => (isFirmwareContainer.value || props.info?.optical_disc ? tr("字节") : tr("扇区")));

/** 逻辑证据没有块设备层，不会有加密容器或卷影副本。 */
const canUnlockContainer = computed(() => props.canUnlockContainer);

// ── 卷与分区 ─────────────────────────────────────────────────────────────
const schemeLabel = computed(() => {
  const scheme = props.info?.table_scheme;
  if (!scheme) return null;
  return tr(SCHEME_LABELS[scheme] || scheme.toUpperCase());
});

const geometryTitle = (partition: DiskPartition) => partitionGeometry(partition, props.info?.sector_size ?? 0, addressUnit.value, tr);

const layoutSegments = computed(() => evidenceLayout(props.info, tr));

const unallocatedBytes = computed(() => {
  const info = props.info;
  if (!info) return 0;
  const used = layoutSegments.value.reduce((sum, seg) => sum + seg.lenBytes, 0);
  return Math.max(info.size_bytes - used, 0);
});

async function scrollToPartition(id: number) {
  const imageId = props.info.id;
  highlightedPartition.value = id;
  await nextTick();
  if (disposed || imageId !== props.info.id || highlightedPartition.value !== id) return;
  const row = sectionEl.value?.querySelector<HTMLElement>(`[id="part-row-${id}"]`);
  row?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  row?.focus({ preventScroll: true });
}

// ── 卷影副本：并入所在 NTFS 分区行，点开在行下列出副本 ───────────────────────
const {
  loadError: shadowLoadError,
  openError: shadowOpenError,
  opening: shadowOpening,
  load: loadShadowCopies,
  open: openShadow,
  volumeOf: shadowVolumeOf,
} = useShadowCopies(() => (
  props.info && canUnlockContainer.value && props.info.source_format !== 'shadow_copy' ? props.info.id : null
));
const expandedShadows = ref<Set<number>>(new Set());

function toggleShadow(partitionId: number) {
  const next = new Set(expandedShadows.value);
  if (!next.delete(partitionId)) next.add(partitionId);
  expandedShadows.value = next;
}

async function openShadowCopy(volume: ShadowVolumeInfo, copy: ShadowCopyInfo) {
  const path = await openShadow(volume, copy);
  if (path) emit('open-image', path);
}

interface PartitionRow {
  partition: DiskPartition;
  color: string;
  shadow: ShadowVolumeInfo | undefined;
}

const partitionRows = computed<PartitionRow[]>(() =>
  (props.info?.partitions ?? []).map((partition, index) => ({
    partition,
    color: partColor(index),
    shadow: shadowVolumeOf(partition.id),
  })),
);

const { SYNTHETIC_MOUNT_KEY, syntheticRoot, busyPartitions, busyShares, pickerOpenFor, availableDrives, pickerLoading, isMountable, canShowMount, onBitlockerUnlocked, mountViewFor, mountFor, toggleShare, beginMount, finishMountAt, toggleMount, mountAt } = useEvidenceMounts({
  info: () => props.info, mountTarget: () => mountTarget.value, mountFolderRoot: () => mountFolderRoot.value,
  caseDirectory: () => props.caseDirectory, onUnlocked: () => { void loadShadowCopies(); },
});

</script>
<style scoped>
/* ── 卷与分区 ── */
.volume-card { container: volume-card / inline-size; }
.head-action { margin-left: auto; }

.disk-map { padding: 0 16px 12px; }
.layout-bar {
  position: relative;
  height: 28px;
  overflow: hidden;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  background: repeating-linear-gradient(135deg,
    var(--surface-tertiary) 0 6px,
    color-mix(in srgb, var(--text-secondary) 16%, var(--surface-tertiary)) 6px 7px);
  --ui-stagger-base: 160ms;
  --ui-stagger-step: 45ms;
}
.layout-seg {
  position: absolute;
  top: 0;
  display: flex;
  align-items: center;
  height: 100%;
  min-width: 3px;
  padding: 0 0 0 6px;
  overflow: hidden;
  border: 0;
  border-right: 2px solid var(--surface-secondary);
  background: var(--seg-color);
  cursor: pointer;
  opacity: 0.85;
  transition: opacity var(--transition-fast), box-shadow var(--transition-fast);
}
.layout-seg:hover, .layout-seg.hover, .layout-seg:focus-visible { opacity: 1; }
.layout-seg.active { opacity: 1; box-shadow: inset 0 0 0 2px var(--text-primary); }
.layout-seg:focus-visible { outline-offset: -3px; }
.seg-label {
  display: block;
  max-width: calc(100% - 8px);
  padding: 1px 6px;
  overflow: hidden;
  border-radius: var(--radius-sm);
  background: color-mix(in srgb, var(--surface-secondary) 88%, transparent);
  color: var(--text-primary);
  font-size: 11px;
  font-weight: 600;
  line-height: 16px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.seg-label small { margin-left: 6px; color: var(--text-secondary); font-size: 11px; font-weight: 500; }
.disk-map-note { display: flex; align-items: center; justify-content: flex-end; gap: 6px; margin: 6px 0 0; color: var(--text-secondary); font-size: 11px; }
.unalloc-swatch {
  width: 10px;
  height: 10px;
  border: 1px solid var(--border-color);
  border-radius: 3px;
  background: repeating-linear-gradient(135deg,
    var(--surface-tertiary) 0 2px,
    color-mix(in srgb, var(--text-secondary) 40%, var(--surface-tertiary)) 2px 3px);
}

/* 分区表：表头是一条安静的浅色带，与应用其他表格一致；选中（定位）行是浅底加
   1px 主题色描边，悬停与磁盘图色块互相同步。 */
.partition-table { font-size: 12px; }
.part-head,
.partition-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 88px 120px 96px 200px;
  align-items: center;
  column-gap: 12px;
  padding: 0 16px;
}
.part-head {
  min-height: 32px;
  border-block: 1px solid var(--border-color);
  background: color-mix(in srgb, var(--surface-tertiary) 60%, var(--surface-secondary));
  color: var(--text-secondary);
  font-weight: 500;
}
.part-body { padding-bottom: 6px; --ui-stagger-base: 200ms; --ui-stagger-step: 40ms; }
.part-body > * { border-bottom: 1px solid color-mix(in srgb, var(--border-color) 70%, transparent); }
.part-body > :last-child { border-bottom: 0; }
.partition-row {
  min-height: 40px;
  padding-block: 6px;
  outline: none;
  scroll-margin-block: 16px;
  transition: background-color var(--transition-fast);
}
.partition-row:hover, .partition-row.hover { background: var(--surface-tertiary); }
.partition-row.active {
  background: var(--sidebar-active-bg);
  outline: 1px solid color-mix(in srgb, var(--primary-color) 70%, transparent);
  outline-offset: -1px;
}
.partition-row:focus-visible { outline: 1px solid var(--primary-color); outline-offset: -1px; }
.cell { min-width: 0; }
.col-start, .col-size { text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }
.cell-label { display: none; }
.col-name { display: flex; align-items: center; gap: 8px; }
.part-swatch { flex-shrink: 0; width: 10px; height: 10px; border-radius: 3px; background: var(--part-color); }
.part-label { min-width: 0; overflow: hidden; font-weight: 500; text-overflow: ellipsis; white-space: nowrap; }
.part-index { flex-shrink: 0; color: var(--text-secondary); font-size: 11px; }
.partition-row.quiet :is(.part-label, .col-start, .col-size) { color: var(--text-secondary); }
.partition-row .col-actions { display: flex; flex-wrap: wrap; align-items: center; justify-content: flex-end; gap: 6px; }

.shadow-none { color: var(--text-secondary); font-size: 11px; white-space: nowrap; cursor: help; }
.shadow-toggle.failed { color: var(--sev-warn-fg); }
.shadow-chevron { transition: transform var(--transition-fast); }
.shadow-toggle[aria-expanded='true'] .shadow-chevron { transform: rotate(180deg); }
/* 展开的卷影副本列表接在所属行下，缩进到名称文字；点开时立即入场，不排队。 */
.shadow-row {
  padding: 4px 16px 8px 34px;
  background: color-mix(in srgb, var(--surface-tertiary) 40%, var(--surface-secondary));
  --ui-stagger-base: 0ms;
  --ui-stagger-index: 0;
}
.shadow-alert { align-items: center; margin: 0 16px 16px; }
.shadow-alert .ui-btn { margin-left: auto; }

.volume-empty { padding: 28px 20px 32px; }
.synthetic-root-mount { padding: 0 16px 16px; }
.synthetic-root-row { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; min-height: 40px; }
.synthetic-root-title { font-size: 13px; font-weight: 600; }
.synthetic-root-row .mount-wrap { margin-left: auto; }
.synthetic-root-hint { margin: 4px 0 0; color: var(--text-secondary); font-size: 12px; line-height: 1.6; }

/* Motion: the page parts rise one after another (ui-stagger on .overview),
   partition rows cascade and the disk map fills in from the left. */
:root[data-motion='full'] .layout-bar > .layout-seg { animation-name: ui-grow-x; animation-duration: var(--motion-slow); transform-origin: left center; }

/* 窄卡：分区行改为两行（名称 / 大小，文件系统、起始扇区 / 操作），表头收起。 */
@container volume-card (max-width: 600px) {
  .part-head { display: none; }
  .partition-row {
    grid-template-columns: auto auto minmax(0, 1fr) auto;
    grid-template-areas: 'name name name size' 'fs start . actions';
    row-gap: 6px;
    padding-block: 8px;
  }
  .partition-row .col-name { grid-area: name; }
  .partition-row .col-fs { grid-area: fs; }
  .partition-row .col-start { grid-area: start; color: var(--text-secondary); font-size: 11px; text-align: left; }
  .partition-row .col-size { grid-area: size; }
  .partition-row .col-actions { grid-area: actions; }
  .cell-label { display: inline; margin-right: 4px; font-family: var(--font-ui); }
  .shadow-row { padding-left: 16px; }
}
</style>
