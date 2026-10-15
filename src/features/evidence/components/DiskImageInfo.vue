<template>
  <div class="disk-image-info">
    <div v-if="!info" class="ui-empty">
      <span class="ui-empty-icon"><HardDrive :size="26" /></span>
      <p class="ui-empty-title">{{ tr("未加载镜像") }}</p>
    </div>
    <div v-else class="overview ui-stagger">
      <!-- 检材头部：哪份检材（文件名、格式、完整路径、容量），右侧是平台识别结论与原始镜像导出。
           不做卡片；页面名由侧栏给出。 -->
      <EvidenceHeader :info="info" @copy="copyText" />
      <div v-if="info.index_only" class="ui-alert info" role="status">{{ tr("离线索引 · 原镜像不可用。容量与格式来自案件记录；目录和属性可在文件列表查看，恢复原路径的镜像后重新打开即可读取正文。") }}</div>
      <IosBackupCard v-if="info.ios_backup" :key="info.id" :info="info" @open-image="emit('open-image', $event)" />

      <EvidenceVolumes v-if="!info.index_only && !info.ios_backup" :key="info.id" :info="info" :can-unlock-container="supportsBlockOperations" :case-directory="activeCase?.dir"
        @unlock="openUnlock" @open-image="emit('open-image', $event)" @copy="copyText" />

      <!-- RAID 检测（与该镜像无关时自隐） -->
      <RaidStackPanel v-if="supportsBlockOperations" :info="info" @open-image="emit('open-image', $event)" />

      <div v-if="!info.index_only" class="overview-grid">
        <EvidenceIntegrityCard :info="info" @copy="copyText" />

        <EvidenceAcquisitionCard :info="info" @copy="copyText" />
      </div>

      <EncryptedVolumeUnlockDialog
        v-if="supportsBlockOperations"
        :key="info.id"
        :open="unlockOpen"
        :info="info"
        :partition-id="unlockPartitionId"
        @close="unlockOpen = false"
      />
    </div>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, ref, watch } from 'vue';
import { HardDrive } from 'lucide-vue-next';
import type { ImageInfo } from '../../../lib/ipc/image';
import { notifySuccess, notifyError } from '../../../composables/useToast';
import { useCaseContext } from '../../cases/api';
import EncryptedVolumeUnlockDialog from './EncryptedVolumeUnlockDialog.vue';
import RaidStackPanel from '../../raid/components/RaidStackPanel.vue';
import EvidenceHeader from './EvidenceHeader.vue';
import IosBackupCard from './IosBackupCard.vue';
import EvidenceVolumes from './EvidenceVolumes.vue';
import EvidenceIntegrityCard from './EvidenceIntegrityCard.vue';
import EvidenceAcquisitionCard from './EvidenceAcquisitionCard.vue';

const props = defineProps<{ info: ImageInfo | null }>();
const emit = defineEmits<{ 'open-image': [path: string] }>();

const { activeCase } = useCaseContext();
const LOGICAL_FORMATS = new Set(['folder', 'tar', 'zip', 'sevenz', 'ad1', 'docker', 'android_backup']);
const supportsBlockOperations = computed(() => !!props.info && !props.info.index_only && !props.info.ios_backup && !LOGICAL_FORMATS.has(props.info.source_format));

// ── LUKS / VeraCrypt：卷卡标题或 LUKS 分区行打开解锁弹窗 ──────────────────────
const unlockOpen = ref(false);
const unlockPartitionId = ref<number | null>(null);

function openUnlock(partitionId: number | null) {
  unlockPartitionId.value = partitionId;
  unlockOpen.value = true;
}

async function copyText(label: string, value: string) {
  try {
    await navigator.clipboard.writeText(value);
    notifySuccess(tr("已复制"), label);
  } catch (e) {
    notifyError(tr("复制失败"), String(e));
  }
}

watch(
  () => props.info?.id,
  () => {
    unlockOpen.value = false;
  },
  { immediate: true },
);

</script>

<style scoped>
.disk-image-info {
  height: 100%;
  min-width: 0;
  overflow-y: auto;
  padding: 20px 24px 28px;
  color: var(--text-primary);
  background: var(--surface-primary);
  font-family: var(--font-ui);
  container: image-info / inline-size;
}

.overview {
  display: flex;
  flex-direction: column;
  gap: 16px;
  max-width: 1200px;
  margin-inline: auto;
  --ui-stagger-step: 60ms;
}

/* ── 完整性校验与详细信息 ── */
.overview-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 360px), 1fr)); align-items: start; gap: 16px; }
</style>
