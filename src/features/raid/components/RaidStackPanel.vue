<template>
  <section v-if="visible" class="ui-card panel-card raid-card">
    <header class="ui-card-header panel-header">
      <h3 class="ui-card-title"><Boxes :size="15" />{{ tr("RAID 检测") }}</h3>
      <span v-if="arrayCount > 0" class="ui-count accent">{{ arrayCount }} {{ tr("阵列") }}</span>
      <div v-if="info.source_format !== 'assembled'" class="header-actions">
        <button
          class="ui-btn sm"
          :title="tr('手动指定成员盘、块大小和数据偏移')"
          @click="showManual = true"
        >
          <Wrench :size="13" />
          <span>{{ tr("手动重组") }}</span>
        </button>
        <button
          class="ui-icon-btn sm bordered"
          :disabled="loading"
          :title="tr('重新检测（扫描所有已加载镜像）')"
          :aria-label="tr('重新检测')"
          @click="() => detect()"
        >
          <Loader2 v-if="loading" :size="13" class="spin" />
          <RefreshCw v-else :size="13" />
        </button>
      </div>
    </header>

    <div class="panel-content">
      <div v-if="loading && !stack" class="status-row">
        <Loader2 :size="13" class="spin" />
        <span>{{ tr("正在扫描已加载镜像中的 LVM / mdadm 结构…") }}</span>
      </div>

      <div v-else-if="error" class="ui-alert danger">
        <AlertTriangle :size="15" />
        <span>{{ tr("检测失败：") }}{{ error }}</span>
      </div>

      <template v-else-if="stack">
        <!-- mdadm arrays this image participates in -->
        <RaidArrayCard v-for="arr in relevantArrays" :key="arr.set_uuid" :arr="arr" :image-id="info.id" :image-paths="imagePaths"
          :diagnostics="relevantArrays.length === 1 ? stack.diagnostics : undefined"
          :opening="openingId === arr.assembled_image_id" :exporting="!!arr.assembled_image_id && !!jobFor(arr.assembled_image_id)"
          :diagram-open="diagramOverrides[arr.set_uuid] ?? !arr.assembled_image_id" @diagram-toggle="open => diagramOverrides[arr.set_uuid] = open"
          @open="openAssembled(arr)" @export="exportAssembled(arr)" />

        <!-- LVM volume groups on this image, one line each (the layer RAID
             members may sit on — slot rows above already name the LVs) -->
        <RaidVolumeGroupCard v-for="vg in relevantVgs" :key="vg.vg_uuid" :vg="vg" />

        <div v-if="!hasFindings" class="status-row">
          <span>{{ tr("未检测到与该镜像相关的 LVM 卷组或 mdadm RAID 阵列。") }}</span>
        </div>

        <details v-disclosure-motion v-if="stack.diagnostics.length > 0 && relevantArrays.length !== 1" class="ui-disclosure diagnostics">
          <summary><ChevronRight :size="14" class="ui-chevron" />{{ tr("诊断信息") }} <span class="ui-count">{{ stack.diagnostics.length }}</span></summary>
          <ul>
            <li v-for="(d, i) in stack.diagnostics" :key="i" class="mono">{{ d }}</li>
          </ul>
        </details>
      </template>
    </div>

    <Transition name="ui-modal">
      <RaidManualBuilder
        v-if="showManual"
        @close="showManual = false"
        @open-image="path => emit('open-image', path)"
      />
    </Transition>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import { computed, onScopeDispose, ref, watch } from 'vue';
import {
  AlertTriangle,
  Boxes,
  ChevronRight,
  Loader2,
  RefreshCw,
  Wrench,
} from 'lucide-vue-next';
import { imageInfoById } from '../../../lib/ipc/image';
import { type RaidArray } from '../../../lib/ipc/raid';
import { useRaidStack } from '../composables/useRaidStack';
import { useImageExport } from '../../evidence/api';
import { notifyError } from '../../../composables/useToast';
import type { ImageInfo } from '../../../lib/ipc/image';
import RaidManualBuilder from './RaidManualBuilder.vue';
import RaidArrayCard from './RaidArrayCard.vue';
import RaidVolumeGroupCard from './RaidVolumeGroupCard.vue';
import { exportBaseName, relevantRaidArrays, relevantRaidVolumeGroups } from '../model/stackView';

const props = defineProps<{ info: ImageInfo }>();
const emit = defineEmits<{ 'open-image': [path: string] }>();

// Detection state is session-wide (the scan covers every loaded image) and
// shared across panel instances — see useRaidStack. Rendering, however, is
// per-image: only arrays/VGs this image participates in are shown, so a
// plain single-disk image never grows a card describing other tabs' RAID.
const { stack, loading, error, detect } = useRaidStack();
const { startImageExport, jobFor } = useImageExport();
const openingId = ref<string | null>(null);
const showManual = ref(false);
let generation = 0;
let disposed = false;
onScopeDispose(() => { disposed = true; generation += 1; });

// Per-array stripe-diagram open state. Seeded lazily by status: already-assembled
// arrays start collapsed (shorter resting card), degraded/unassembled ones start
// open. An examiner's manual toggle is remembered so a later reactive re-render
// (opening an image, a re-detect) never snaps their choice back to the default —
// a plain `:open="..."` binding would, since it's controlled.
const diagramOverrides = ref<Record<string, boolean>>({});

const relevantVgs = computed(() => relevantRaidVolumeGroups(stack.value, props.info.id));
const relevantArrays = computed(() => relevantRaidArrays(stack.value, props.info.id));
const imagePaths = computed(() => new Map(stack.value?.images.map(image => [image.image_id, image.path])));

const arrayCount = computed(() => relevantArrays.value.length);

const hasFindings = computed(() => relevantArrays.value.length > 0 || relevantVgs.value.length > 0);

// Heuristic for "this image might be a RAID member": an mdadm/LVM partition
// or an mdadm/LVM2 whole-disk scheme. Drives panel visibility when nothing
// relevant was found — a plain laptop image shouldn't grow an empty RAID
// card, and detect errors only surface on images that could be members.
const hasRaidHints = computed(() => {
  const info = props.info;
  if (info.table_scheme === 'mdadm' || info.table_scheme === 'lvm2') return true;
  return info.partitions.some(p => p.kind === 'mdadm' || p.kind === 'lvm');
});

// A disk with no detected partition table is a plausible headerless RAID
// member (raw striped/parity data, like a hardware-RAID or NAS member) — show
// the panel so the manual builder is reachable even before/without a
// successful auto-detect.
const couldBeRawMember = computed(() => props.info.partitions.length === 0);

const visible = computed(
  () =>
    hasFindings.value ||
    couldBeRawMember.value ||
    (hasRaidHints.value && (loading.value || error.value !== null || stack.value !== null)),
);

// Auto-detect when an image with RAID hints (mdadm/LVM partitions or
// whole-disk scheme) shows, and once per session even without hints so the
// cross-image case still surfaces (the shared stack makes remounts free —
// no re-fire on every dashboard round-trip). The manual refresh button
// re-runs unconditionally.
watch(
  () => props.info.id,
  () => {
    generation += 1;
    openingId.value = null;
    if (hasRaidHints.value || stack.value === null) void detect();
  },
  { immediate: true, flush: 'sync' },
);

async function openAssembled(arr: RaidArray) {
  const imageId = arr.assembled_image_id;
  if (!imageId || openingId.value) return;
  const captured = generation;
  openingId.value = imageId;
  try {
    // RaidStack carries only the synthetic image's id — resolve its
    // `assembled://<label>` path, which the tab loader opens by reuse.
    const summary = await imageInfoById({ imageId });
    if (!disposed && captured === generation) emit('open-image', summary.path);
  } catch (e) {
    if (!disposed && captured === generation) notifyError(tr("打开重组镜像失败"), String(e));
  } finally {
    if (!disposed && captured === generation) openingId.value = null;
  }
}

async function exportAssembled(arr: RaidArray) {
  const imageId = arr.assembled_image_id;
  if (!imageId || jobFor(imageId)) return;
  await startImageExport({ imageId, defaultFileName: `${exportBaseName(arr)}.dd` });
}

</script>

<style scoped>
.raid-card { container: raid-card / inline-size; }

.panel-header { padding: 12px 16px; }

.header-actions {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 8px;
}

.panel-content {
  padding: 4px 16px 16px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.status-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--text-secondary);
}

.diagnostics {
  font-size: 11px;
  color: var(--text-secondary);
}

.diagnostics summary {
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  font-size: 12px;
}

.diagnostics summary:hover { color: var(--primary-color); }
.diagnostics summary:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }

.diagnostics ul {
  margin: 8px 0 0;
  padding-left: 16px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  word-break: break-all;
}

</style>
