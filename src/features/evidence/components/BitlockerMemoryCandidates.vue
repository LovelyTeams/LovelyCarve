<template>
  <div class="memory-scan">
    <div class="memory-file">
      <button class="ui-btn ghost sm" type="button" :disabled="!active || !!scan" @click="pickMemoryFile">
        <FileSearch :size="13" />{{ memoryPath ? tr("更换内存镜像") : tr("选择内存镜像") }}
      </button>
      <span v-if="memoryPath" class="memory-path mono" :title="memoryPath">{{ memoryPath }}</span>
      <button v-if="!scan" class="ui-btn sm" type="button" :disabled="!active || !memoryPath || partitionId === undefined" @click="startScan">
        {{ candidates ? tr("重新扫描") : tr("开始扫描") }}
      </button>
    </div>
    <div v-if="scan" class="scan-progress" role="status">
      <div class="ui-progress" :class="{ indeterminate: !scan.bytesTotal }"><span :style="scan.bytesTotal ? { width: `${percent}%` } : undefined"></span></div>
      <span v-if="scan.cancelling" class="scan-text">{{ tr("取消中…") }}</span>
      <span v-else-if="scan.bytesTotal" class="scan-text mono">{{ `${percent.toFixed(1)}%` }}<span>{{ `${formatBytes(scan.bytesDone)} / ${formatBytes(scan.bytesTotal)}` }}</span></span>
      <span v-else class="scan-text">{{ tr("准备中…") }}</span>
      <button class="ui-btn ghost sm" type="button" :disabled="scan.cancelling" @click="cancelScan">{{ tr("取消扫描") }}</button>
    </div>
    <template v-else-if="candidates">
      <p v-if="!candidates.length" class="ui-field-hint" role="status">{{ tr("未找到 VMK 候选") }}</p>
      <div v-else class="candidate-list" role="radiogroup" :aria-label="tr('VMK 候选')">
        <label v-for="candidate in candidates" :key="`${candidate.offset}:${candidate.vmk_hex}`" class="ui-choice candidate">
          <input v-model="selected" type="radio" :name="groupName" :value="candidate.vmk_hex" :disabled="!active" />
          <span class="ui-choice-text">
            <span class="ui-choice-title mono">{{ candidate.recovery_key ?? `VMK ${candidate.vmk_hex.slice(0, 16)}…` }}</span>
            <span class="ui-choice-desc mono">0x{{ candidate.offset.toString(16) }}</span>
          </span>
          <span v-if="candidate.already_tried" class="ui-badge sm">{{ tr("已尝试") }}</span>
        </label>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, onScopeDispose, ref, useId, watch } from 'vue';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { FileSearch } from 'lucide-vue-next';
import { tr } from '../../../composables/useLocale';
import { formatBytes } from '../../../utils/format';
import type { BitlockerVmkCandidate, UnlockCredential } from '../../../lib/ipc/encrypted';
import type { ImageId } from '../../../lib/ipc/ids';
import { cancelBitlockerMemoryScan, startBitlockerMemoryScan, useBitlockerScanJob } from '../composables/useBitlockerMemoryScan';

/** Find VMKs in a memory image and pick one to unlock with; the credential is
 *  the chosen candidate's VMK. */
const props = defineProps<{ imageId: ImageId; partitionId: number | undefined; active: boolean }>();
const credential = defineModel<UnlockCredential | null>('credential', { required: true });

const memoryPath = ref('');
const candidates = ref<BitlockerVmkCandidate[] | null>(null);
const selected = ref('');
const groupName = useId();
const scan = useBitlockerScanJob(() => props.imageId, () => props.partitionId);
const percent = computed(() => scan.value?.bytesTotal ? Math.min(100, scan.value.bytesDone / scan.value.bytesTotal * 100) : 0);
watch(selected, vmk => { credential.value = vmk ? { kind: 'vmk', keyHex: vmk } : null; }, { immediate: true });

// Results and file pickers belong to one partition of one image.
let context = 0;
onScopeDispose(() => { context++; });
watch(() => [props.imageId, props.partitionId], () => {
  context++;
  memoryPath.value = '';
  candidates.value = null;
  selected.value = '';
}, { flush: 'sync' });
watch(() => props.active, active => { if (!active) selected.value = ''; });

async function pickMemoryFile() {
  const captured = context;
  const picked = await openDialog({
    multiple: false,
    title: tr("选择内存镜像"),
    filters: [
      { name: tr("内存镜像"), extensions: ['raw', 'mem', 'bin', 'dmp', 'vmem'] },
      { name: tr("所有文件"), extensions: ['*'] },
    ],
  });
  if (captured !== context || !props.active || typeof picked !== 'string') return;
  memoryPath.value = picked;
  candidates.value = null;
  selected.value = '';
}

async function startScan() {
  if (!memoryPath.value || props.partitionId === undefined || scan.value) return;
  const captured = context;
  selected.value = '';
  const result = await startBitlockerMemoryScan(props.imageId, props.partitionId, memoryPath.value);
  if (!result || captured !== context) return;
  candidates.value = result.candidates;
  // Prefer a VMK that derives this volume's recovery key.
  selected.value = result.candidates.find(candidate => candidate.recovery_key)?.vmk_hex ?? '';
}

function cancelScan() {
  if (props.partitionId !== undefined) void cancelBitlockerMemoryScan(props.imageId, props.partitionId);
}

/** Mark the chosen candidate as tried once an unlock with it is submitted. */
function markSubmitted() {
  const candidate = candidates.value?.find(item => item.vmk_hex === selected.value);
  if (candidate) candidate.already_tried = true;
  selected.value = '';
}
defineExpose({ markSubmitted });
</script>

<style scoped>
.memory-scan { display: grid; gap: 10px; min-width: 0; }
.memory-file { display: flex; align-items: center; gap: 8px; min-width: 0; }
.memory-path { flex: 1; min-width: 0; overflow: hidden; color: var(--text-secondary); font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
.scan-progress { display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: center; gap: 6px 10px; }
.scan-progress .ui-progress { grid-column: 1 / -1; }
.scan-text { display: flex; gap: 10px; color: var(--text-secondary); font-size: 11px; }
.candidate-list { display: grid; gap: 6px; max-height: 240px; overflow-y: auto; padding: 2px; }
.candidate { gap: 10px; padding: 10px 12px; }
.candidate .ui-choice-title { overflow-wrap: anywhere; }
.ui-field-hint { margin: 0; }
</style>
