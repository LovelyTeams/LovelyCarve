<template>
  <div class="ui-modal-overlay manual-overlay" @click.self="emit('close')">
    <div class="ui-modal manual-modal" role="dialog" aria-modal="true" aria-labelledby="raid-manual-title">
      <header class="ui-modal-header">
        <span class="ui-modal-icon"><Wrench :size="18" /></span>
        <div class="ui-modal-heading">
          <h2 id="raid-manual-title" class="ui-modal-title">{{ tr("手动 RAID 重组") }}</h2>
        </div>
        <button class="ui-icon-btn" :title="tr('关闭')" :aria-label="tr('关闭')" @click="emit('close')">
          <X :size="16" />
        </button>
      </header>

      <div class="ui-modal-body manual-body">
        <p class="hint">
          {{ tr("自动检测无法识别的阵列（未知偏移、缺盘或任意盘序）可在此手动指定几何参数，按列序选择成员盘后预览重组结果。") }}
        </p>

        <!-- Member selection + column order -->
        <div class="cols">
          <section class="col">
            <h4 class="col-title">{{ tr("可选镜像") }}</h4>
            <ul v-if="candidates.length" class="image-list">
              <li
                v-for="img in candidates"
                :key="img.id"
                class="image-row"
                :class="{ used: order.includes(img.id) }"
                @click="toggleMember(img.id)"
              >
                <component :is="order.includes(img.id) ? CheckSquare : Square" :size="14" />
                <span class="image-name" :title="img.path">{{ baseName(img.path) }}</span>
                <span class="image-size mono">{{ formatBytes(img.size_bytes) }}</span>
              </li>
            </ul>
            <p v-else class="empty">{{ tr("无已加载的真实镜像") }}</p>
          </section>

          <section class="col">
            <div class="col-head">
              <h4 class="col-title">{{ tr("列顺序（col0 → colN）") }}</h4>
              <button v-if="tolerance > 0" class="ui-btn sm ghost" :title="tr('为没有镜像的成员盘占一列，按校验或镜像重建')" @click="addAbsentColumn">
                <Plus :size="13" />{{ tr("缺盘列") }}
              </button>
            </div>
            <ul v-if="order.length" class="order-list">
              <li v-for="(key, idx) in order" :key="key" class="order-row">
                <span class="slot mono">col{{ idx }}</span>
                <span v-if="isAbsent(key)" class="image-name absent">{{ tr("（缺盘）") }}</span>
                <span v-else class="image-name" :title="pathOf(key)">{{ baseName(pathOf(key)) }}</span>
                <span v-if="missingKeys.includes(key) || isAbsent(key)" class="ui-badge sm warn">{{ rebuildBadge }}</span>
                <span class="order-actions">
                  <button v-if="tolerance > 0 && !isAbsent(key)" class="ui-icon-btn sm"
                    :class="{ active: missingKeys.includes(key) }"
                    :title="missingKeys.includes(key) ? tr('改为读取该列') : tr('不读取该列，按校验或镜像重建')"
                    :aria-label="missingKeys.includes(key) ? tr('改为读取该列') : tr('不读取该列，按校验或镜像重建')"
                    :aria-pressed="missingKeys.includes(key)" @click="toggleMissing(key)">
                    <CircleSlash :size="14" />
                  </button>
                  <button class="ui-icon-btn sm" :title="tr('上移')" :aria-label="tr('上移')" :disabled="idx === 0" @click="move(idx, -1)">
                    <ChevronUp :size="14" />
                  </button>
                  <button class="ui-icon-btn sm" :title="tr('下移')" :aria-label="tr('下移')" :disabled="idx === order.length - 1" @click="move(idx, 1)">
                    <ChevronDown :size="14" />
                  </button>
                  <button class="ui-icon-btn sm danger" :title="tr('移除')" :aria-label="tr('移除')" @click="removeColumn(key)">
                    <X :size="14" />
                  </button>
                </span>
              </li>
            </ul>
            <p v-else class="empty">{{ tr("点击左侧镜像，按列序加入") }}</p>
          </section>
        </div>

        <!-- Geometry parameters -->
        <div class="params">
          <label class="param">
            <span>{{ tr("级别") }}</span>
            <UiSelect v-model="level" class="sm mono" :options="LEVEL_OPTIONS" />
          </label>
          <label v-if="level === 5 || level === 6" class="param">
            <span>{{ tr("校验布局") }}</span>
            <UiSelect v-model="parityLayout" class="sm mono" :options="parityLayoutOptions" />
          </label>
          <template v-if="level === 10">
            <label class="param">
              <span>{{ tr("副本布局") }}</span>
              <UiSelect v-model="copyMode" class="sm mono" :options="COPY_MODE_OPTIONS" />
            </label>
            <label class="param">
              <span>{{ tr("副本数") }}</span>
              <input v-model.number="copies" class="ui-input sm mono" type="number" min="2" step="1" />
            </label>
          </template>
          <label v-if="level !== 1" class="param">
            <span>{{ level === -1 ? tr("成员取整（KiB，0 表示不取整）") : tr("块大小（KiB）") }}</span>
            <input v-model.number="chunkKiB" class="ui-input sm mono" type="number" :min="level === -1 ? 0 : 1" step="1"
              :aria-invalid="chunkInvalid || undefined" :aria-describedby="chunkInvalid ? 'raid-manual-problem' : undefined" />
          </label>
          <label class="param">
            <span>{{ tr("数据偏移（扇区）") }}</span>
            <input v-model.number="dataOffsetSectors" class="ui-input sm mono" type="number" min="0" step="1" />
          </label>
        </div>
        <p v-if="geometryProblem" id="raid-manual-problem" class="ui-field-error">{{ geometryProblem }}</p>

        <!-- Live stripe-layout preview (pure geometry, no backend round-trip) -->
        <div v-if="order.length >= 2 && showDiagram" class="diagram-wrap">
          <h4 class="col-title">{{ tr("条带布局预览") }}</h4>
          <RaidLayoutDiagram
            :level="level"
            :layout="layout"
            :num-disks="order.length"
            :disk-labels="diagramLabels"
            :missing-slots="missingSlots"
            :chunk-label="level === 1 ? null : `${chunkKiB}K`"
          />
        </div>

        <!-- Preview -->
        <div v-if="error" class="ui-alert danger">
          <AlertTriangle :size="15" />
          <span>{{ error }}</span>
        </div>
        <div v-else-if="result" class="preview">
          <div class="preview-head mono">
            {{ tr("重组容量") }} {{ formatBytes(result.size_bytes) }} ·
            <template v-if="result.partitions.length">{{ result.partitions.length }} {{ tr("分区") }}</template>
            <template v-else>{{ tr("未识别出分区表") }}</template>
          </div>
          <ul v-if="result.partitions.length" class="part-list">
            <li v-for="(p, i) in result.partitions" :key="i" class="part-row mono">
              <span class="part-kind" :class="{ fs: isBrowsable(p.kind) }">{{ p.kind }}</span>
              <span>LBA {{ p.start_lba }} · {{ formatBytes(p.length_lba * 512) }}</span>
              <span v-if="p.label" class="part-label">{{ p.label }}</span>
            </li>
          </ul>
          <p v-else class="empty mono">{{ tr("当前几何参数下未识别出分区或文件系统，请调整参数后重试。") }}</p>
        </div>
      </div>

      <footer class="ui-modal-footer">
        <button class="ui-btn ghost start" @click="emit('close')">{{ tr("取消") }}</button>
        <button class="ui-btn" :class="{ primary: !canOpen }" :disabled="!canPreview || busy"
          :aria-describedby="geometryProblem ? 'raid-manual-problem' : undefined" @click="preview">
          <Loader2 v-if="busy" :size="14" class="spin" />
          <span>{{ busy ? tr("重组中…") : tr("重组并预览") }}</span>
        </button>
        <button v-if="canOpen" class="ui-btn primary" :disabled="opening" @click="open">
          <Loader2 v-if="opening" :size="14" class="spin" />
          <ExternalLink v-else :size="14" />
          <span>{{ tr("打开重组镜像") }}</span>
        </button>
      </footer>
    </div>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { formatBinaryBytes as formatBytes } from '../../../utils/format';
import { computed, onMounted, ref, watch } from 'vue';
import {
  AlertTriangle,
  CheckSquare,
  ChevronDown,
  ChevronUp,
  CircleSlash,
  ExternalLink,
  Loader2,
  Plus,
  Square,
  Wrench,
  X,
} from 'lucide-vue-next';
import { imageInfoById, listLoadedImages, type LoadedImage } from '../../../lib/ipc/image';
import { raidManualAssemble, type ManualRaidResult } from '../../../lib/ipc/raid';
import { notifyError } from '../../../composables/useToast';
import RaidLayoutDiagram from './RaidLayoutDiagram.vue';
import UiSelect, { type UiSelectOption } from '../../../components/ui/UiSelect.vue';
import { parityLayoutName, RAID5_LAYOUTS, RAID6_LAYOUTS } from '../model/stackView';
import { encodeRaid10Layout, hasStripeLayout } from '../model/stripeLayout';

const emit = defineEmits<{ close: []; 'open-image': [path: string] }>();

const LEVEL_OPTIONS: UiSelectOption<number>[] = [
  { value: 5, label: 'RAID5' },
  { value: 6, label: 'RAID6' },
  { value: 0, label: 'RAID0' },
  { value: 1, label: 'RAID1' },
  { value: 10, label: 'RAID10' },
  { value: 4, label: 'RAID4' },
  { value: -1, get label() { return tr("线性（JBOD）"); } },
];
type CopyMode = 'near' | 'far' | 'offset';
const COPY_MODE_OPTIONS: UiSelectOption<CopyMode>[] = [
  { value: 'near', get label() { return tr("近端（相邻盘）"); } },
  { value: 'far', get label() { return tr("远端（盘内后半区）"); } },
  { value: 'offset', get label() { return tr("偏移（下一行）"); } },
];
const ABSENT_PREFIX = 'absent:';

const candidates = ref<LoadedImage[]>([]);
/** Column keys in order: loaded image ids, or `absent:<n>` for a column whose disk is unavailable. */
const order = ref<string[]>([]);
/** Present columns to rebuild instead of read. */
const missingKeys = ref<string[]>([]);
const level = ref(5);
const parityLayout = ref(1); // right-asymmetric — the common hardware-RAID5 default
const copyMode = ref<CopyMode>('near');
const copies = ref(2);
const chunkKiB = ref(32);
const dataOffsetSectors = ref(2048); // 1 MiB
let absentCounter = 0;

const busy = ref(false);
const opening = ref(false);
const error = ref<string | null>(null);
const result = ref<ManualRaidResult | null>(null);

onMounted(async () => {
  try {
    const all = await listLoadedImages();
    // Real images only — synthetic assembled devices can't be members.
    candidates.value = all.filter(img => !img.path.startsWith('assembled://'));
  } catch (e) {
    error.value = tr("加载镜像列表失败：{0}", { 0: e });
  }
});

// md's default rotation for each parity level.
watch(level, next => {
  if (next === 6 && !RAID6_LAYOUTS.includes(parityLayout.value)) parityLayout.value = 2;
  if (next === 5 && !RAID5_LAYOUTS.includes(parityLayout.value)) parityLayout.value = 1;
  if (next === -1 && chunkKiB.value < 0) chunkKiB.value = 0;
  if (tolerance.value === 0) {
    order.value = order.value.filter(key => !isAbsent(key));
    missingKeys.value = [];
  }
  result.value = null;
});
watch([parityLayout, copyMode, copies, chunkKiB, dataOffsetSectors], () => { result.value = null; });

const parityLayoutOptions = computed<UiSelectOption<number>[]>(() =>
  (level.value === 6 ? RAID6_LAYOUTS : RAID5_LAYOUTS).map(value => ({
    value,
    get label() { return parityLayoutName(value, tr); },
  })));

/** md `layout` field for the chosen level. */
const layout = computed(() => {
  if (level.value === 5 || level.value === 6) return parityLayout.value;
  if (level.value === 10) {
    const count = Math.max(2, Math.round(copies.value));
    return encodeRaid10Layout(copyMode.value === 'near'
      ? { near: count, far: 1, offset: false }
      : { near: 1, far: count, offset: copyMode.value === 'offset' });
  }
  return 0;
});

/** Columns the level can lose and still read every chunk. */
const tolerance = computed(() => {
  switch (level.value) {
    case 1: return Math.max(0, order.value.length - 1);
    case 4:
    case 5: return 1;
    case 6: return 2;
    case 10: return Math.max(0, Math.round(copies.value) - 1);
    default: return 0;
  }
});

const missingSlots = computed(() => order.value
  .map((key, index) => (isAbsent(key) || missingKeys.value.includes(key) ? index : -1))
  .filter(index => index >= 0));

const minColumns = computed(() => (level.value === 6 ? 4 : level.value === 10 ? Math.max(2, Math.round(copies.value)) : 2));

const chunkInvalid = computed(() => level.value !== 1 && level.value !== -1 && !(chunkKiB.value > 0));

const geometryProblem = computed<string | null>(() => {
  if (chunkInvalid.value) return tr("块大小必须大于 0");
  // While the first columns are being picked the disabled button says enough.
  if (order.value.length < 2) return null;
  if (order.value.length < minColumns.value) return tr("该级别至少需要 {0} 列", { 0: minColumns.value });
  if (missingSlots.value.length > tolerance.value) return tr("该级别最多缺失 {0} 列", { 0: tolerance.value });
  if (missingSlots.value.length === order.value.length) return tr("至少需要一列可读的成员盘");
  return null;
});

const canPreview = computed(() => order.value.length >= 2 && geometryProblem.value == null);

const showDiagram = computed(() => hasStripeLayout(level.value));

const FS_KINDS = new Set(['ext4', 'btrfs', 'xfs', 'ntfs', 'refs', 'fat12', 'fat16', 'fat32', 'exfat']);
function isBrowsable(kind: string): boolean {
  return FS_KINDS.has(kind);
}

const canOpen = computed(
  () => result.value != null && result.value.partitions.some(p => isBrowsable(p.kind)),
);

const rebuildBadge = computed(() => (level.value === 1 || level.value === 10 ? tr("缺失 · 镜像读取") : tr("缺失 · 校验重建")));

/** Column labels for the live diagram, in the current column order. */
const diagramLabels = computed(() => order.value.map(key => (isAbsent(key) ? tr("（缺盘）") : baseName(pathOf(key)))));

function isAbsent(key: string): boolean {
  return key.startsWith(ABSENT_PREFIX);
}

function pathOf(id: string): string {
  return candidates.value.find(i => i.id === id)?.path ?? id;
}

function baseName(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || path;
}

function toggleMember(id: string) {
  if (order.value.includes(id)) removeColumn(id);
  else order.value.push(id);
  result.value = null;
}

function addAbsentColumn() {
  absentCounter += 1;
  order.value.push(`${ABSENT_PREFIX}${absentCounter}`);
  result.value = null;
}

function removeColumn(key: string) {
  order.value = order.value.filter(item => item !== key);
  missingKeys.value = missingKeys.value.filter(item => item !== key);
  result.value = null;
}

function toggleMissing(key: string) {
  missingKeys.value = missingKeys.value.includes(key)
    ? missingKeys.value.filter(item => item !== key)
    : [...missingKeys.value, key];
  result.value = null;
}

function move(idx: number, delta: number) {
  const j = idx + delta;
  if (j < 0 || j >= order.value.length) return;
  const arr = order.value;
  [arr[idx], arr[j]] = [arr[j], arr[idx]];
  result.value = null;
}

async function preview() {
  if (!canPreview.value || busy.value) return;
  busy.value = true;
  error.value = null;
  result.value = null;
  try {
    result.value = await raidManualAssemble({
      member_image_ids: order.value.map(key => (isAbsent(key) ? '' : key)),
      level: level.value,
      layout: layout.value,
      chunk_sectors: level.value === 1 ? 0 : Math.max(0, Math.round(chunkKiB.value * 2)),
      data_offset_sectors: Math.max(0, Math.round(dataOffsetSectors.value)),
      missing_slots: order.value
        .map((key, index) => (missingKeys.value.includes(key) ? index : -1))
        .filter(index => index >= 0),
    });
  } catch (e) {
    error.value = tr("重组失败：{0}", { 0: e });
  } finally {
    busy.value = false;
  }
}

async function open() {
  if (!result.value || opening.value) return;
  opening.value = true;
  try {
    const summary = await imageInfoById({ imageId: result.value.image_id });
    emit('open-image', summary.path);
    emit('close');
  } catch (e) {
    notifyError(tr("打开重组镜像失败"), String(e));
  } finally {
    opening.value = false;
  }
}

</script>

<style scoped>
.manual-overlay {
  z-index: 1000;
}

.manual-modal {
  width: min(760px, 100%);
  max-height: 90vh;
}

.manual-body {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.hint {
  margin: 0;
  font-size: 12px;
  color: var(--text-secondary);
  line-height: 1.6;
}

.cols {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
}

.col-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 8px;
}

.col-head .col-title {
  margin: 0;
}

.col-title {
  margin: 0 0 8px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-primary);
}

.image-list,
.order-list {
  list-style: none;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  padding: 4px;
  min-height: 120px;
  max-height: 200px;
  overflow-y: auto;
  background: var(--surface-primary);
}

.image-row,
.order-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 30px;
  padding: 3px 6px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  color: var(--text-primary);
}

.image-row {
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast);
}

.image-row:hover {
  background: var(--hover-bg);
}

.image-row.used {
  color: var(--primary-color);
  background: var(--sidebar-active-bg);
}

.image-row svg {
  flex-shrink: 0;
}

.image-name {
  flex: 1;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}

.image-name.absent {
  color: var(--text-secondary);
}

.image-size {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-secondary);
}

.slot {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-secondary);
  width: 36px;
}

.order-actions {
  margin-left: auto;
  display: inline-flex;
  gap: 2px;
}

.order-actions .ui-icon-btn.active {
  color: var(--sev-warn-fg);
}

.empty {
  margin: 0;
  font-size: 12px;
  color: var(--text-secondary);
  padding: 10px 6px;
}

.params {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
}

.param {
  display: flex;
  flex-direction: column;
  gap: 5px;
  font-size: 12px;
  font-weight: 500;
  color: var(--text-secondary);
}

.param .ui-select,
.param .ui-input {
  min-width: 130px;
}

.preview {
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  padding: 10px 12px;
  background: var(--surface-primary);
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.preview-head {
  font-size: 12px;
  color: var(--text-primary);
}

.part-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.part-row {
  display: flex;
  align-items: baseline;
  gap: 10px;
  font-size: 11px;
  color: var(--text-secondary);
}

.part-kind {
  flex-shrink: 0;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--text-secondary);
}

.part-kind.fs {
  color: var(--sev-success-fg);
}

.part-label {
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
