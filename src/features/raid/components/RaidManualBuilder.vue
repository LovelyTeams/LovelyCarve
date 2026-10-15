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
          {{ tr("自动检测啃不动的阵列（未知偏移 / 缺盘 / 任意盘序）可在此手动指定几何参数，按列序选择成员盘后预览重组结果。") }}
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
            <h4 class="col-title">{{ tr("列顺序（col0 → colN）") }}</h4>
            <ul v-if="order.length" class="order-list">
              <li v-for="(id, idx) in order" :key="id" class="order-row">
                <span class="slot mono">col{{ idx }}</span>
                <span class="image-name" :title="pathOf(id)">{{ baseName(pathOf(id)) }}</span>
                <span v-if="level === 5 && missingSlot === idx" class="ui-badge sm warn">{{ tr("掉线·重建") }}</span>
                <span class="order-actions">
                  <button class="ui-icon-btn sm" :title="tr('上移')" :aria-label="tr('上移')" :disabled="idx === 0" @click="move(idx, -1)">
                    <ChevronUp :size="14" />
                  </button>
                  <button class="ui-icon-btn sm" :title="tr('下移')" :aria-label="tr('下移')" :disabled="idx === order.length - 1" @click="move(idx, 1)">
                    <ChevronDown :size="14" />
                  </button>
                  <button class="ui-icon-btn sm danger" :title="tr('移除')" :aria-label="tr('移除')" @click="toggleMember(id)">
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
          <label v-if="level === 5" class="param">
            <span>{{ tr("校验布局") }}</span>
            <UiSelect v-model="layout" class="sm mono" :options="LAYOUT_OPTIONS" />
          </label>
          <label class="param">
            <span>{{ tr("块大小 (KiB)") }}</span>
            <input v-model.number="chunkKiB" class="ui-input sm mono" type="number" min="1" step="1" />
          </label>
          <label class="param">
            <span>{{ tr("数据偏移 (扇区)") }}</span>
            <input v-model.number="dataOffsetSectors" class="ui-input sm mono" type="number" min="0" step="1" />
          </label>
          <label v-if="level === 5" class="param">
            <span>{{ tr("缺失列（降级）") }}</span>
            <UiSelect v-model="missingSlot" class="sm mono" :options="missingSlotOptions" />
          </label>
        </div>

        <!-- Live stripe-layout preview (pure geometry, no backend round-trip) -->
        <div v-if="order.length >= 2" class="diagram-wrap">
          <h4 class="col-title">{{ tr("条带布局预览") }}</h4>
          <RaidLayoutDiagram
            :level="level"
            :layout="layout"
            :num-disks="order.length"
            :disk-labels="diagramLabels"
            :missing-slot="level === 5 ? missingSlot : null"
            :chunk-label="`${chunkKiB}K`"
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
          <p v-else class="empty mono">{{ tr("该几何未产生可识别的分区/文件系统，请调整参数后重试。") }}</p>
        </div>
      </div>

      <footer class="ui-modal-footer">
        <button class="ui-btn ghost start" @click="emit('close')">{{ tr("取消") }}</button>
        <button class="ui-btn" :class="{ primary: !canOpen }" :disabled="order.length < 2 || busy" @click="preview">
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
import { computed, onMounted, ref } from 'vue';
import {
  AlertTriangle,
  CheckSquare,
  ChevronDown,
  ChevronUp,
  ExternalLink,
  Loader2,
  Square,
  Wrench,
  X,
} from 'lucide-vue-next';
import { imageInfoById, listLoadedImages, type LoadedImage } from '../../../lib/ipc/image';
import { raidManualAssemble, type ManualRaidResult } from '../../../lib/ipc/raid';
import { notifyError } from '../../../composables/useToast';
import RaidLayoutDiagram from './RaidLayoutDiagram.vue';
import UiSelect, { type UiSelectOption } from '../../../components/ui/UiSelect.vue';

const emit = defineEmits<{ close: []; 'open-image': [path: string] }>();

const LEVEL_OPTIONS: UiSelectOption<number>[] = [{ value: 5, label: 'RAID5' }, { value: 0, label: 'RAID0' }];
const LAYOUT_OPTIONS: UiSelectOption<number>[] = [
  { value: 0, get label() { return tr("左异步"); } }, { value: 1, get label() { return tr("右异步"); } }, { value: 2, get label() { return tr("左同步"); } }, { value: 3, get label() { return tr("右同步"); } },
];

const candidates = ref<LoadedImage[]>([]);
const order = ref<string[]>([]);
const level = ref(5);
const layout = ref(1); // right-asymmetric — the common hardware-RAID5 default
const chunkKiB = ref(32);
const dataOffsetSectors = ref(2048); // 1 MiB
const missingSlot = ref<number | null>(null);
const missingSlotOptions = computed<UiSelectOption<number | null>[]>(() => [
  { value: null, get label() { return tr("无（全部在线）"); } },
  ...order.value.map((_, index) => ({ value: index, label: `col${index}` })),
]);

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

const FS_KINDS = new Set(['ext4', 'btrfs', 'xfs', 'ntfs', 'refs', 'fat12', 'fat16', 'fat32', 'exfat']);
function isBrowsable(kind: string): boolean {
  return FS_KINDS.has(kind);
}

const canOpen = computed(
  () => result.value != null && result.value.partitions.some(p => isBrowsable(p.kind)),
);

/** Column labels for the live diagram, in the current column order. */
const diagramLabels = computed(() => order.value.map(id => baseName(pathOf(id))));

function pathOf(id: string): string {
  return candidates.value.find(i => i.id === id)?.path ?? id;
}

function baseName(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || path;
}

function toggleMember(id: string) {
  const i = order.value.indexOf(id);
  if (i >= 0) {
    order.value.splice(i, 1);
    if (missingSlot.value != null && missingSlot.value >= order.value.length) missingSlot.value = null;
  } else {
    order.value.push(id);
  }
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
  if (order.value.length < 2 || busy.value) return;
  busy.value = true;
  error.value = null;
  result.value = null;
  try {
    result.value = await raidManualAssemble({
      member_image_ids: [...order.value],
      level: level.value,
      layout: layout.value,
      chunk_sectors: Math.max(1, Math.round(chunkKiB.value * 2)),
      data_offset_sectors: Math.max(0, Math.round(dataOffsetSectors.value)),
      missing_slot: level.value === 5 ? missingSlot.value : null,
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
