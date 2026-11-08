<template>
  <div class="bde-control">
    <span class="ui-badge sm mono" :class="{ success: !locked }" :title="statusTitle">
      <Lock v-if="locked" :size="11" />
      <LockOpen v-else :size="11" />
      {{ locked ? 'LOCKED' : 'UNLOCKED' }}
    </span>
    <button v-if="runningJob" class="ui-btn sm" type="button" aria-haspopup="dialog" :title="progressTitle" @click="emit('open')">
      {{ runningJob.cancelling ? tr("取消中") : job ? tr("解锁中") : tr("扫描中") }}
      <span class="mono">{{ progress.percentText }}</span>
    </button>
    <button v-else-if="locked" class="ui-btn sm" type="button" aria-haspopup="dialog" @click="emit('open')">
      <KeyRound :size="13" />{{ tr("解锁") }}
    </button>
    <button v-else class="ui-btn sm" type="button" :disabled="!!exportJob"
      :title="exportJob ? tr('正在导出解密后完整镜像') : tr('导出完整 RAW 镜像，并将此 BitLocker 分区替换为解密后内容')"
      @click="exportDecryptedImage">
      <Download :size="13" />{{ exportJob ? tr("导出中") : tr("导出解密镜像") }}
    </button>
  </div>
</template>

<script setup lang="ts">
import { computed, onScopeDispose, ref, watch } from 'vue';
import { Download, KeyRound, Lock, LockOpen } from 'lucide-vue-next';
import { tr } from '../../../composables/useLocale';
import { bitlockerStatus } from '../../../lib/ipc/bitlocker';
import { useBitlockerJob, useBitlockerScanJob } from '../composables/useBitlockerUnlocks';
import { useImageExport } from '../composables/useImageExport';
import { bitlockerProgress } from '../model/bitlocker';

const props = defineProps<{ imageId: string; partitionId: number }>();
const emit = defineEmits<{ open: []; unlocked: [partitionId: number] }>();
const locked = ref(true);
const encryption = ref<string | null>(null);
const job = useBitlockerJob(() => props.imageId, () => props.partitionId);
const scanJob = useBitlockerScanJob(() => props.imageId, () => props.partitionId);
const runningJob = computed(() => job.value ?? scanJob.value);
const progress = computed(() => bitlockerProgress(runningJob.value, tr));
const progressTitle = computed(() => runningJob.value?.cancelling ? tr("正在取消…") : job.value
  ? tr("点击查看解锁进度") : tr("点击查看扫描进度"));
const { startBitlockerDecryptedImageExport, jobFor } = useImageExport();
const exportJob = computed(() => jobFor(props.imageId));
const statusTitle = computed(() => locked.value ? tr("BitLocker 卷未解锁") : encryption.value
  ? tr("BitLocker 已解锁 · {0}", { 0: encryption.value })
  : tr("BitLocker 已解锁"));

let statusRequest = 0;
onScopeDispose(() => { statusRequest++; });

async function refreshStatus() {
  const request = ++statusRequest;
  const { imageId, partitionId } = props;
  try {
    const status = await bitlockerStatus({ imageId, partitionId });
    if (request !== statusRequest) return;
    const wasLocked = locked.value;
    locked.value = status.locked;
    encryption.value = status.encryption_method;
    if (wasLocked && !status.locked) emit('unlocked', partitionId);
  } catch {
    // Keep the last known lock state if the status query fails.
  }
}

watch(() => [props.imageId, props.partitionId], () => {
  locked.value = true;
  encryption.value = null;
  void refreshStatus();
}, { immediate: true, flush: 'sync' });

watch(job, (next, previous) => {
  if (previous && !next) void refreshStatus();
});

async function exportDecryptedImage() {
  if (locked.value || exportJob.value) return;
  await startBitlockerDecryptedImageExport({
    imageId: props.imageId,
    partitionId: props.partitionId,
    defaultFileName: `bitlocker-part${props.partitionId}-decrypted-full.dd`,
  });
}
</script>

<style scoped>
.bde-control { display: inline-flex; flex-wrap: wrap; align-items: center; justify-content: flex-end; gap: 6px; }
</style>
