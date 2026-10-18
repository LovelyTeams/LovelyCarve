<template>
  <div class="hash-panel">
    <div class="hash-controls">
      <div class="hash-algos" role="group" :aria-label="tr('哈希算法')">
        <label
          v-for="algo in ALL_HASH_ALGOS"
          :key="algo"
          class="algo-chip mono"
          :class="{ selected: shownSelection.has(algo), disabled: !!job }"
        >
          <input
            type="checkbox"
            :checked="shownSelection.has(algo)"
            :disabled="!!job"
            @change="toggleAlgo(algo)"
          />
          <span>{{ algo.toUpperCase() }}</span>
        </label>
      </div>
      <button
        v-if="!job"
        class="ui-btn sm hash-btn"
        :disabled="selected.length === 0"
        @click="start"
      >
        <Fingerprint :size="13" />
        <span>{{ hasAnyResult ? tr("重新计算") : tr("计算哈希") }}</span>
      </button>
      <button
        v-else
        class="ui-btn sm danger-soft hash-btn"
        :disabled="job.cancelling"
        @click="cancel"
      >
        <span>{{ job.cancelling ? tr("取消中…") : tr("取消") }}</span>
      </button>
    </div>

    <div v-if="job" class="hash-progress">
      <div class="hash-progress-meta">
        <span class="hash-progress-label">{{ job.cancelling ? tr("正在取消") : tr("正在计算") }} <span class="mono hash-selection">{{ algorithmSummary }}</span></span>
        <span class="mono hash-progress-pct">{{ percentText }}</span>
      </div>
      <div class="ui-progress">
        <span :style="{ width: percent + '%' }"></span>
      </div>
      <div class="hash-progress-bytes mono">{{ bytesText }}</div>
    </div>

    <dl v-if="hasAnyResult" class="hash-results">
      <div v-for="algo in completedAlgos" :key="algo" class="hash-row">
        <dt class="hash-name mono">{{ algo.toUpperCase() }}</dt>
        <dd>
          <span class="hash-value mono">{{ result?.[algo] }}</span>
          <button
            class="ui-icon-btn sm hash-copy"
            :title="copiedAlgo === algo ? tr('已复制') : tr('复制 {0}', { 0: algo.toUpperCase() })"
            :aria-label="tr('复制 {0}', { 0: algo.toUpperCase() })"
            @click="copy(algo)"
          >
            <Check v-if="copiedAlgo === algo" :size="12" />
            <Copy v-else :size="12" />
          </button>
        </dd>
      </div>
    </dl>
    <div v-if="result?.read_faults_detected" class="ui-alert warn hash-fault-warning">
      <AlertTriangle :size="15" />
      <span>
        {{ tr("哈希包含") }} {{ formatBytes(result.synthetic_bytes) }} {{ tr("补零字节，涉及") }}
        {{ result.bad_regions.length }} {{ tr("个坏区；结果不能视为无错误的源盘哈希。") }}
      </span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { formatBinaryBytes as formatBytes } from '../../../utils/format';
import { computed, ref, watch } from 'vue';
import { AlertTriangle, Fingerprint, Copy, Check } from 'lucide-vue-next';
import {
  ALL_HASH_ALGOS,
  cancelImageHash,
  clearImageHashResult,
  startImageHash,
  useImageHashJob,
  useImageHashResult,
  type HashAlgo,
} from '../composables/useImageHash';
import { notifyError } from '../../../composables/useToast';

const props = defineProps<{
  imageId: string;
  sizeBytes: number;
}>();

const selected = ref<HashAlgo[]>([...ALL_HASH_ALGOS]);

const job = useImageHashJob(() => props.imageId);
const result = useImageHashResult(() => props.imageId);
// A running job shows the algorithms it was started with, which may differ
// from this panel's selection when it began elsewhere.
const shownSelection = computed(() => new Set(job.value?.algorithms ?? selected.value));
const algorithmSummary = computed(() => [...shownSelection.value].map(algo => algo.toUpperCase()).join(' · '));

const copiedAlgo = ref<HashAlgo | null>(null);
let copiedTimer: ReturnType<typeof setTimeout> | null = null;

const hasAnyResult = computed(() => {
  const r = result.value;
  return !!r && !!(r.md5 || r.sha1 || r.sha256 || r.sm3);
});

const completedAlgos = computed(() => ALL_HASH_ALGOS.filter(algo => !!result.value?.[algo]));

const percent = computed(() => {
  const j = job.value;
  if (!j || j.bytesTotal === 0) return 0;
  return Math.min(100, (j.bytesDone / j.bytesTotal) * 100);
});
const percentText = computed(() => `${percent.value.toFixed(1)}%`);


const bytesText = computed(() => {
  const j = job.value;
  if (!j) return '';
  if (j.bytesTotal === 0) return tr("准备中…");
  return `${formatBytes(j.bytesDone)} / ${formatBytes(j.bytesTotal)}`;
});

function toggleAlgo(algo: HashAlgo) {
  if (job.value) return;
  const set = new Set(selected.value);
  if (set.has(algo)) set.delete(algo);
  else set.add(algo);
  selected.value = ALL_HASH_ALGOS.filter(a => set.has(a));
}

async function start() {
  if (selected.value.length === 0) {
    notifyError(tr("哈希计算失败"), tr("请至少选择一种算法"));
    return;
  }
  await startImageHash(props.imageId, selected.value);
}

async function cancel() {
  await cancelImageHash(props.imageId);
}

async function copy(algo: HashAlgo) {
  const value = result.value?.[algo];
  if (!value) return;
  try {
    await navigator.clipboard.writeText(value);
    copiedAlgo.value = algo;
    if (copiedTimer) clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => {
      copiedAlgo.value = null;
      copiedTimer = null;
    }, 1500);
  } catch (err) {
    notifyError(tr("复制失败"), String(err));
  }
}

// When the image swaps (different tab / unload), clear the stale result
// so the new image starts from a blank slate. The in-flight job map is
// keyed by imageId so it isn't affected.
watch(
  () => props.imageId,
  (_next, prev) => {
    if (prev && prev !== props.imageId) {
      clearImageHashResult(prev);
    }
  },
);
</script>

<style scoped>
.hash-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

/* 算法开关与计算按钮同一行，窄时按钮换到下一行。 */
.hash-controls {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
}

.hash-algos {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-right: auto;
}

.algo-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 26px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  background: var(--surface-secondary);
  color: var(--text-secondary);
  font-size: 11px;
  cursor: pointer;
  user-select: none;
  transition: background var(--transition-fast), color var(--transition-fast), border-color var(--transition-fast);
}

.algo-chip:hover {
  border-color: color-mix(in srgb, var(--primary-color) 45%, var(--border-color));
}

.algo-chip:focus-within {
  outline: 2px solid var(--primary-color);
  outline-offset: 1px;
}

.algo-chip input {
  width: 12px;
  height: 12px;
  margin: 0;
  accent-color: var(--primary-color);
  cursor: inherit;
}

.algo-chip.selected {
  border-color: color-mix(in srgb, var(--primary-color) 60%, var(--border-color));
  background: var(--sidebar-active-bg);
  color: var(--primary-color);
}

.algo-chip.disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.hash-progress {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.hash-progress-meta {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  color: var(--text-secondary);
  font-size: 12px;
}

.hash-progress-pct {
  color: var(--text-primary);
  font-size: 12px;
}

.hash-progress-bytes {
  color: var(--text-secondary);
  font-size: 11px;
}

/* 结果直接列出：算法、完整摘要（可换行）、复制。 */
.hash-results {
  margin: 0;
}

.hash-row {
  display: grid;
  grid-template-columns: 56px minmax(0, 1fr);
  align-items: baseline;
  gap: 12px;
  padding: 8px 0;
  border-top: 1px solid color-mix(in srgb, var(--border-color) 70%, transparent);
}

.hash-name {
  color: var(--text-secondary);
  font-size: 11px;
  letter-spacing: 0.06em;
}

.hash-row dd {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  min-width: 0;
  margin: 0;
}

.hash-value {
  flex: 1;
  min-width: 0;
  color: var(--text-primary);
  font-size: 12px;
  line-height: 1.6;
  word-break: break-all;
}

.hash-copy {
  flex-shrink: 0;
}
</style>
