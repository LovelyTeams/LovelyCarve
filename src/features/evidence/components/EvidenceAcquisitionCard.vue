<template>
<section class="ui-card" data-section="details" aria-labelledby="details-title">
  <header class="ui-card-header">
    <h3 id="details-title" class="ui-card-title"><Info :size="15" />{{ tr("详细信息") }}</h3>
  </header>
  <div class="card-body">
    <dl class="fact-list">
      <div v-for="row in detailRows" :key="row.label" class="fact-row">
        <dt>{{ row.label }}</dt>
        <dd>
          <span :class="{ mono: row.mono }">{{ row.value }}</span>
          <button v-if="row.copyable" class="ui-icon-btn sm" :title="tr('复制{0}', { 0: row.label })" :aria-label="tr('复制{0}', { 0: row.label })" @click="emit('copy', row.label, row.value)"><Copy :size="12" /></button>
        </dd>
      </div>
    </dl>
    <div v-if="ewfRows.length > 0" class="acquisition">
      <h4 class="group-title">{{ tr("采集记录") }}</h4>
      <dl v-if="acquisitionHighlights.length > 0" class="fact-list">
        <div v-for="row in acquisitionHighlights" :key="row.label" class="fact-row">
          <dt>{{ row.label }}</dt><dd><span :class="{ mono: row.mono }">{{ row.value }}</span></dd>
        </div>
      </dl>
      <details v-disclosure-motion v-if="acquisitionDetails.length > 0" :key="info.id" class="ui-disclosure acquisition-details">
        <summary>
          <ChevronRight :size="14" class="ui-chevron" />{{ acquisitionHighlights.length > 0 ? tr("更多采集信息") : tr("查看采集信息") }}
          <span class="ui-count">{{ acquisitionDetails.length }}</span>
        </summary>
        <dl class="fact-list">
          <div v-for="row in acquisitionDetails" :key="row.label" class="fact-row">
            <dt>{{ row.label }}</dt><dd><span :class="{ mono: row.mono }">{{ row.value }}</span></dd>
          </div>
        </dl>
      </details>
    </div>
  </div>
</section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { ChevronRight, Copy, Info } from 'lucide-vue-next';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import type { ImageInfo } from '../../../lib/ipc/image';
import { ACQUISITION_HIGHLIGHTS, acquisitionRows, evidenceFacts } from '../model/evidenceOverview';
const props = defineProps<{ info: ImageInfo }>();
const emit = defineEmits<{ copy: [label: string, value: string] }>();
const detailRows = computed(() => evidenceFacts(props.info, tr));
const ewfRows = computed(() => acquisitionRows(props.info));
const acquisitionHighlights = computed(() => ewfRows.value.filter(row => ACQUISITION_HIGHLIGHTS.has(row.label))
  .map(row => ({ ...row, label: tr(row.label) })));
const acquisitionDetails = computed(() => ewfRows.value.filter(row => !ACQUISITION_HIGHLIGHTS.has(row.label))
  .map(row => ({ ...row, label: tr(row.label) })));
</script>
<style scoped src="../styles/evidence-facts.css"></style>
