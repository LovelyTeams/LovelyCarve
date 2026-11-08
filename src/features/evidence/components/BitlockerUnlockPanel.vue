<template>
  <div v-if="active" class="bitlocker-panel">
    <BitlockerJobProgress v-if="job" :label="job.cancelling ? tr('取消中…') : unlockLabel" :percent="unlockProgress.percent" :percent-text="unlockProgress.percentText" :bytes-text="unlockProgress.bytesText"
      :cancelling="job.cancelling" :note="tr('解锁任务会在后台继续，关闭面板不会中断。')" :cancel-label="tr('取消解锁')" @cancel="cancelBitlockerUnlock(imageId, partitionId)" />
    <BitlockerJobProgress v-else-if="scanJob" :label="scanJob.cancelling ? tr('取消中…') : tr('扫描内存镜像')" :percent="scanProgress.percent" :percent-text="scanProgress.percentText" :bytes-text="scanProgress.bytesText"
      :cancelling="scanJob.cancelling" :note="tr('扫描会在后台继续，关闭面板不会中断。')" :cancel-label="tr('取消扫描')" @cancel="cancelBitlockerMemoryScan(imageId, partitionId)" />
    <p v-else-if="unlocked" class="ui-alert success" role="status">{{ tr("BitLocker 已解锁") }}</p>
    <template v-else>
      <div class="ui-seg" role="group" :aria-label="tr('解锁凭据')">
        <button v-for="tab in tabs" :key="tab.id" class="ui-seg-item" :class="{ active: mode === tab.id }" type="button"
          :aria-pressed="mode === tab.id" @click="mode = tab.id">{{ tab.label }}</button>
      </div>
      <BitlockerCredentialForm v-if="mode === 'password' || mode === 'recovery'" :mode="mode" v-model:password-input="passwordInput" v-model:recovery-input="recoveryInput"
        :elements="credentialElements" :recovery-error="recoveryError" :recovery-note-id="recoveryNoteId" :valid-groups="recoveryCheck.validGroups"
        :can-submit-password="canSubmitPassword" :can-submit-recovery="canSubmitRecovery" @password="submitPassword" @recovery="submitRecovery" @blur="onRecoveryBlur" @close="emit('close')" />
      <BitlockerKeyForm v-else-if="mode === 'key'" :key="`${imageId}:${partitionId}`" @submit="submitKey" @close="emit('close')" />
      <BitlockerMemoryCandidates v-else :memory-path="memoryPath" :candidates="candidates"
        @pick="pickMemoryFile" @scan="startScan" @reset="candidates = null" @close="emit('close')" @unlock="unlockWithCandidate" />
      <p v-if="metadata" class="ui-field-hint">{{ tr("元数据 v") }}{{ metadata.version }} · {{ metadata.bytes_per_sector }} {{ tr("字节扇区") }}{{ metadata.used_space_only ? tr(" · 仅加密已用空间") : '' }} {{ tr("· 保护器") }} {{ joinList(metadata.protector_types.map(protectorLabel), tr) || tr("无") }}</p>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, onScopeDispose, ref, watch } from 'vue';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { tr } from '../../../composables/useLocale';
import { joinList } from '../../../i18n/translate';
import { useBitlockerCredentials } from '../composables/useBitlockerCredentials';
import { dismissBitlockerReminder } from '../composables/useFileSession';
import {
  cancelBitlockerMemoryScan,
  cancelBitlockerUnlock,
  startBitlockerMemoryScan,
  startBitlockerPasswordUnlock,
  startBitlockerRecoveryUnlock,
  startBitlockerVmkUnlock,
  startBitlockerKeyUnlock,
  useBitlockerJob,
  useBitlockerScanJob,
} from '../composables/useBitlockerUnlocks';
import { bitlockerMetadata, type BitlockerMetadata } from '../../../lib/ipc/bitlocker';
import { bitlockerProgress, type BitlockerInputMode, type BitlockerVmkCandidate, type BitlockerKeyCredential } from '../model/bitlocker';
import BitlockerCredentialForm from './BitlockerCredentialForm.vue';
import BitlockerKeyForm from './BitlockerKeyForm.vue';
import BitlockerMemoryCandidates from './BitlockerMemoryCandidates.vue';
import BitlockerJobProgress from './BitlockerJobProgress.vue';

const props = defineProps<{ imageId: string; partitionId: number; active: boolean; unlocked: boolean }>();
const emit = defineEmits<{ close: [] }>();

const tabs: { id: BitlockerInputMode; label: string }[] = [
  { id: 'password', get label() { return tr("密码"); } },
  { id: 'recovery', get label() { return tr("恢复密钥"); } },
  { id: 'memory', get label() { return tr("内存镜像"); } },
  { id: 'key', get label() { return tr("其他密钥"); } },
];

const metadata = ref<BitlockerMetadata | null>(null);
const mode = ref<BitlockerInputMode>('recovery');
const { passwordInput, recoveryInput, passwordEl, recoveryEl, credentialElements, canSubmitPassword, recoveryCheck, recoveryTouched, recoveryNoteId, recoveryError, canSubmitRecovery, onRecoveryBlur, clearCredentials } = useBitlockerCredentials();
const memoryPath = ref('');
const candidates = ref<BitlockerVmkCandidate[] | null>(null);
const job = useBitlockerJob(() => props.imageId, () => props.partitionId);
const scanJob = useBitlockerScanJob(() => props.imageId, () => props.partitionId);
const unlockProgress = computed(() => bitlockerProgress(job.value, tr));
const scanProgress = computed(() => bitlockerProgress(scanJob.value, tr));

const unlockLabel = computed(() => {
  switch (job.value?.mode) {
    case 'password': return tr("密码解锁中");
    case 'recovery': return tr("恢复密钥解锁中");
    case 'vmk': return tr("VMK 解锁中");
    case 'key': return tr("密钥解锁中");
    default: return tr("解锁中");
  }
});

// A scan may publish its candidates after the modal closes. File pickers and
// automatic close must still belong to the same visit to this partition.
let context = 0;
let visit = 0;
onScopeDispose(() => { context++; visit++; });
watch(() => [props.imageId, props.partitionId], () => {
  context++;
  visit++;
  metadata.value = null;
  memoryPath.value = '';
  candidates.value = null;
  clearCredentials();
}, { flush: 'sync' });
watch(() => props.active, async active => {
  visit++;
  if (!active) {
    clearCredentials();
    return;
  }
  dismissBitlockerReminder(props.imageId);
  if (metadata.value) return;
  const captured = visit;
  try {
    const result = await bitlockerMetadata({ imageId: props.imageId, partitionId: props.partitionId });
    if (captured === visit) metadata.value = result;
  } catch {
    // Credential entry is still available when metadata cannot be read.
  }
}, { immediate: true });
watch(mode, () => {
  if (!props.active) return;
  if (mode.value === 'password') passwordEl.value?.focus();
  if (mode.value === 'recovery') recoveryEl.value?.focus();
}, { flush: 'post' });

function protectorLabel(value: number): string {
  const labels: Record<number, string> = { 0: tr("明文密钥"), 0x0200: 'BEK', 0x0800: tr("恢复密码"), 0x2000: tr("密码"), 0x0100: 'TPM' };
  return labels[value] ?? `0x${value.toString(16).padStart(4, '0')}`;
}

async function finishUnlock(result: Promise<boolean>) {
  const capturedContext = context;
  const capturedVisit = visit;
  clearCredentials();
  if (!await result || capturedContext !== context) return false;
  candidates.value = null;
  memoryPath.value = '';
  if (props.active && capturedVisit === visit) emit('close');
  return true;
}

function submitKey(credential: BitlockerKeyCredential) {
  if (!job.value) void finishUnlock(startBitlockerKeyUnlock(props.imageId, props.partitionId, credential));
}

function submitPassword() {
  if (!canSubmitPassword.value || job.value) return;
  void finishUnlock(startBitlockerPasswordUnlock(props.imageId, props.partitionId, passwordInput.value));
}

function submitRecovery() {
  recoveryTouched.value = true;
  if (!canSubmitRecovery.value || job.value) return;
  void finishUnlock(startBitlockerRecoveryUnlock(props.imageId, props.partitionId, recoveryCheck.value.normalized));
}

async function pickMemoryFile() {
  const captured = visit;
  const selected = await openDialog({
    multiple: false,
    title: tr("选择内存镜像"),
    filters: [
      { name: 'Memory dump', extensions: ['raw', 'mem', 'bin', 'dmp', 'vmem'] },
      { name: tr("所有文件"), extensions: ['*'] },
    ],
  });
  if (captured !== visit || !props.active || mode.value !== 'memory' || !selected || typeof selected !== 'string') return;
  memoryPath.value = selected;
  candidates.value = null;
}

async function startScan() {
  if (!memoryPath.value || scanJob.value) return;
  const captured = context;
  const result = await startBitlockerMemoryScan(props.imageId, props.partitionId, memoryPath.value);
  if (result && captured === context) candidates.value = result.candidates;
}

async function unlockWithCandidate(candidate: BitlockerVmkCandidate) {
  if (job.value || candidate.already_tried) return;
  const captured = context;
  const unlocked = await finishUnlock(startBitlockerVmkUnlock(props.imageId, props.partitionId, candidate.vmk_hex));
  if (!unlocked && captured === context) candidate.already_tried = true;
}
</script>

<style scoped>
.bitlocker-panel { display: flex; flex-direction: column; gap: 8px; }
.bitlocker-panel .ui-seg-item { flex: 1; }
</style>
