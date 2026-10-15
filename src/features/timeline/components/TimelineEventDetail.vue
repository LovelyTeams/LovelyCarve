<template>
  <DetailDock :label="tr('事件详情')" :close-label="tr('关闭事件详情')" :title="time" :content-key="row.id" @close="$emit('close')">
    <FileDetailLayout :title="time" mono-title>
      <template #icon><component :is="icon" :size="20" /></template>
      <template #badges><span class="ui-badge tone" :style="{ '--badge-tone': tone }">{{ flagLabel }}</span><span v-if="row.deleted" class="ui-badge warn">{{ tr("已删除") }}</span></template>
      <template v-if="row.data_type === 1" #actions><button class="ui-btn sm" :disabled="locating" @click="$emit('open', row)"><Loader2 v-if="locating" :size="14" class="spin" /><ArrowUpRight v-else :size="14" />{{ locating ? tr("定位中…") : tr("定位文件") }}</button></template>
      <FileDetailSection :title="tr('基本信息')">
        <FileDetailFields :fields="[
          { label: tr('事件类型'), value: dataTypeLabel(row.data_type, tr) },
          { label: tr('时间语义'), value: row.tl_flag },
          { label: tr('来源'), value: row.from_app || tr('文件系统') },
        ]" />
      </FileDetailSection>
      <FileDetailSection :title="tr('事件内容')" wide>
        <p class="detail-block prose">{{ row.content || tr("（无摘要）") }}</p>
        <p v-if="row.data_type === 3" class="detail-note detail-alert">{{ tr("日志来源：$Extend\\$UsnJrnl:$J。显示事件发生时的文件名与目录编号。") }}</p>
      </FileDetailSection>
      <FileDetailSection :key="row.id" :title="tr('来源标识')" collapsible wide>
        <FileDetailFields :fields="[
          { label: tr('检材'), value: row.eid, mono: true, copyable: true },
          { label: tr('记录'), value: row.oid, mono: true, copyable: true },
        ]" />
      </FileDetailSection>
    </FileDetailLayout>
  </DetailDock>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, type Component } from 'vue';
import { ArrowUpRight, Loader2 } from 'lucide-vue-next';
import DetailDock from '../../../components/layout/DetailDock.vue';
import FileDetailLayout from '../../../components/layout/FileDetailLayout.vue';
import FileDetailSection from '../../../components/layout/FileDetailSection.vue';
import FileDetailFields from '../../../components/layout/FileDetailFields.vue';
import type { TimelineRow } from '../../../lib/ipc/timeline';
import { dataTypeLabel } from '../model/timelineView';

const props = defineProps<{
  row: TimelineRow;
  /** Event time already formatted in the panel's display time zone. */
  time: string;
  /** Event-kind colour for the badge (the same tone as the table row). */
  tone: string;
  icon: Component;
  locating?: boolean;
}>();
defineEmits<{ open: [row: TimelineRow]; close: [] }>();
const flagLabel = computed(() => props.row.tl_flag || dataTypeLabel(props.row.data_type, tr));
</script>

<style scoped src="../../../styles/analysis-detail.css"></style>
