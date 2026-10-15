<template>
  <DetailDock :label="tr('文件详情')" :close-label="tr('关闭文件详情')" :title="artifact.name" :content-key="artifact.id" @close="$emit('close')">
    <FileDetailLayout :title="artifact.name">
      <template #icon><FileNodeIcon :id="artifact.id" kind="file" :name="artifact.name" :size="20" /></template>
      <template #badges><span class="ui-badge">{{ tr("雕刻结果") }}</span><span class="ui-badge">{{ artifact.format_id.toUpperCase() }}</span><span v-if="artifact.read_fault" class="ui-badge warn">{{ tr("读取异常") }}</span></template>
      <template #actions><button class="ui-btn sm" :disabled="recovering" @click="$emit('recover', artifact.id)"><Loader2 v-if="recovering" :size="14" class="spin" /><Download v-else :size="14" />{{ recovering ? tr("恢复中…") : tr("恢复此文件") }}</button></template>
      <FileDetailSection :title="tr('基本信息')">
        <FileDetailFields :fields="[
          { label: tr('格式'), value: artifact.format_id.toUpperCase(), mono: true },
          { label: tr('大小'), value: formatBytes(artifact.length), mono: true },
          { label: tr('起始偏移'), value: offset, mono: true, copyable: true },
          { label: tr('所在区域'), value: artifact.region },
          { label: tr('置信度'), value: `${artifact.confidence}%`, mono: true },
        ]" />
      </FileDetailSection>
      <FileDetailSection :title="tr('文件预览')">
        <div class="image-preview" :aria-busy="loading">
          <Loader2 v-if="loading" :size="22" class="spin" :aria-label="tr('正在加载预览')" />
          <img v-else-if="previewUrl" :src="previewUrl" :alt="artifact.name" />
          <span v-else>{{ previewMessage }}</span>
        </div>
      </FileDetailSection>
      <FileDetailSection :title="tr('复核提示')" wide>
        <p class="detail-note">{{ tr("雕刻结果无原始路径与时间戳，请通过偏移和导出哈希复核。") }}</p>
        <p v-if="artifact.read_fault" class="ui-alert warn detail-alert">{{ tr("包含读取失败后补零的数据，请复核完整性。") }}</p>
      </FileDetailSection>
    </FileDetailLayout>
  </DetailDock>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { Download, Loader2 } from 'lucide-vue-next';
import DetailDock from '../../../components/layout/DetailDock.vue';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import FileDetailLayout from '../../../components/layout/FileDetailLayout.vue';
import FileDetailSection from '../../../components/layout/FileDetailSection.vue';
import FileDetailFields from '../../../components/layout/FileDetailFields.vue';
import type { CarveArtifact } from '../../../lib/ipc/carve';
import { formatBytes } from '../../../utils/format';

defineProps<{ artifact: CarveArtifact; offset: string; previewUrl: string | null; previewMessage: string; loading: boolean; recovering: boolean }>();
defineEmits<{ recover: [id: string]; close: [] }>();
</script>

<style scoped src="../../../styles/analysis-detail.css"></style>
<style scoped>
.image-preview { display: flex; align-items: center; justify-content: center; min-height: 140px; padding: 12px; border-radius: var(--radius-md); background: var(--surface-primary); color: var(--text-secondary); font-size: 12px; line-height: 1.8; text-align: center; }
.image-preview img { max-width: 100%; max-height: 320px; object-fit: contain; }
</style>
