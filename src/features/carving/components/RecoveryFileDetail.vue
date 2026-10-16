<template>
  <DetailDock :label="tr('文件详情')" :close-label="tr('关闭文件详情')" :title="entry.name" :content-key="entry.key" @close="$emit('close')">
    <FileDetailLayout :title="entry.name">
      <template #icon><FileNodeIcon :id="entry.key" kind="file" :name="entry.name" :size="20" /></template>
      <template #badges>
        <span class="ui-badge" :class="{ danger: node.deleted }">{{ node.deleted ? tr("已删除") : tr("现存文件") }}</span>
        <span v-if="candidate.source_node" class="ui-badge info">{{ tr("回收站还原") }}</span>
        <span v-if="volume?.reconstruction" class="ui-badge warn">{{ tr("MFT 重建") }}</span>
        <span v-else-if="volume?.lost" class="ui-badge warn">{{ tr("遗失分区") }}</span>
        <span v-if="!candidate.readable" class="ui-badge warn">{{ tr("无可读映射") }}</span>
      </template>
      <template #actions><button class="ui-btn sm" :disabled="recovering || !candidate.readable" @click="$emit('recover', entry)"><Loader2 v-if="recovering" :size="14" class="spin" /><Download v-else :size="14" />{{ recovering ? tr("恢复中…") : tr("恢复此文件") }}</button></template>
      <FileDetailSection :title="tr('基本信息')">
        <FileDetailFields :fields="[
          { label: tr('原路径'), value: candidate.path, mono: true, copyable: true },
          { label: tr('大小'), value: formatBytes(node.size), mono: true },
          { label: tr('所在卷'), value: volumeLabel },
          { label: tr('节点 ID'), value: node.id, mono: true, copyable: true },
          { label: tr('记录号'), value: node.inode, mono: true },
          ...(candidate.initialized_size !== null ? [{ label: tr('有效数据'), value: formatBytes(candidate.initialized_size), mono: true }] : []),
          ...(candidate.source_path ? [{ label: tr('回收站路径'), value: candidate.source_path, mono: true, copyable: true }] : []),
        ]" />
      </FileDetailSection>
      <FileDetailSection :title="tr('时间')">
        <FileDetailFields :fields="[
          { label: tr('修改时间'), value: formatTimestamp(node.timestamps.modified), mono: true },
          { label: tr('访问时间'), value: formatTimestamp(node.timestamps.accessed), mono: true },
          { label: tr('创建时间'), value: formatTimestamp(node.timestamps.created || node.timestamps.birth), mono: true },
          { label: tr('MFT 变更'), value: formatTimestamp(node.timestamps.mft_changed), mono: true },
        ]" />
      </FileDetailSection>
      <FileDetailSection :title="tr('复核提示')" wide>
        <p class="detail-note">{{ node.deleted ? tr("删除文件的簇可能已被覆盖。可读映射只表示可以尝试读取，请用导出哈希和内容复核。") : tr("现存文件按文件系统记录读取。") }}</p>
        <p v-if="!candidate.readable" class="ui-alert warn detail-alert">{{ tr("该记录没有可读的数据映射，无法恢复内容。可尝试签名雕刻查找残留数据。") }}</p>
      </FileDetailSection>
    </FileDetailLayout>
  </DetailDock>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';
import { computed } from 'vue';
import { Download, Loader2 } from 'lucide-vue-next';
import DetailDock from '../../../components/layout/DetailDock.vue';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import FileDetailLayout from '../../../components/layout/FileDetailLayout.vue';
import FileDetailSection from '../../../components/layout/FileDetailSection.vue';
import FileDetailFields from '../../../components/layout/FileDetailFields.vue';
import type { RecoveryCandidate, RecoveryVolume } from '../../../lib/ipc/carve';
import { formatBytes } from '../../../utils/format';
import { formatTimestamp } from '../../../utils/time';
import type { RecoveryEntry } from '../model/recoveryTree';

const props = defineProps<{ entry: RecoveryEntry & { candidate: RecoveryCandidate }; volume: RecoveryVolume | null; volumeName: string; recovering: boolean }>();
defineEmits<{ recover: [entry: RecoveryEntry]; close: [] }>();
const candidate = computed(() => props.entry.candidate);
const node = computed(() => props.entry.candidate.node);
const volumeLabel = computed(() => props.volume
  ? `${props.volumeName} · ${props.volume.partition.kind.toUpperCase()} · LBA ${props.volume.partition.start_lba}`
  : props.volumeName);
</script>

<style scoped src="../../../styles/analysis-detail.css"></style>
