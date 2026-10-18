<template>
  <DetailDock :label="tr('文件详情')" :close-label="tr('关闭命中详情')" :title="hit.file_name" :content-key="hit.hit_seq" @close="$emit('close')">
    <FileDetailLayout :title="hit.file_name">
      <template #icon><FileNodeIcon :id="hit.node_id" :kind="hit.source_kind === 'file' ? 'file' : 'unallocated'" :name="hit.file_name" :size="20" /></template>
      <template #badges><span class="ui-badge">{{ tr("搜索命中") }}</span><span class="ui-badge">{{ sourceKindLabel(hit.source_kind, tr) }}</span></template>
      <template #actions><button class="ui-btn sm" :disabled="locating" @click="$emit('open', hit)"><Loader2 v-if="locating" :size="14" class="spin" /><ArrowUpRight v-else :size="14" />{{ locating ? tr("定位中…") : tr("定位文件") }}</button></template>
      <FileDetailSection :title="tr('基本信息')">
        <FileDetailFields :fields="[
          { label: tr('路径'), value: hit.file_path, copyable: true },
          { label: tr('大小'), value: formatSearchFileSize(hit.file_size), mono: true },
        ]" />
      </FileDetailSection>
      <FileDetailSection :title="tr('命中信息')">
        <FileDetailFields :fields="[
          { label: tr('匹配方式'), value: hit.pattern_kind === 'regex' ? tr('正则表达式') : tr('关键词') },
          { label: tr('搜索词'), value: hit.pattern, copyable: true },
          { label: tr('偏移'), value: formatHitOffset(hit), mono: true, copyable: true },
          { label: tr('编码'), value: hit.encoding || tr('未知'), mono: true },
        ]" />
      </FileDetailSection>
      <FileDetailSection :title="tr('命中上下文')" wide>
        <pre class="detail-block"><template v-for="(segment, index) in highlightSearchPattern(hit.context_full || hit.context_preview, hit.pattern)" :key="index"><mark v-if="segment.matched">{{ segment.text }}</mark><template v-else>{{ segment.text }}</template></template></pre>
      </FileDetailSection>
      <FileDetailSection v-if="hashFields.length" :title="tr('文件哈希')" wide>
        <FileDetailFields :fields="hashFields" />
      </FileDetailSection>
    </FileDetailLayout>
  </DetailDock>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { ArrowUpRight, Loader2 } from 'lucide-vue-next';
import DetailDock from '../../../components/layout/DetailDock.vue';
import FileNodeIcon from '../../../components/display/FileNodeIcon.vue';
import FileDetailLayout from '../../../components/layout/FileDetailLayout.vue';
import FileDetailSection from '../../../components/layout/FileDetailSection.vue';
import FileDetailFields from '../../../components/layout/FileDetailFields.vue';
import type { GlobalSearchHit } from '../../../lib/ipc/search';
import { formatHitOffset, formatSearchFileSize, highlightSearchPattern, sourceKindLabel } from '../model/searchResults';

const props = defineProps<{ hit: GlobalSearchHit; locating?: boolean }>();
defineEmits<{ open: [hit: GlobalSearchHit]; close: [] }>();
const hashFields = computed(() => [
  { label: 'MD5', value: props.hit.md5, mono: true, copyable: true },
  { label: 'SHA256', value: props.hit.sha256, mono: true, copyable: true },
].filter(field => field.value));
</script>

<style scoped src="../../../styles/analysis-detail.css"></style>
