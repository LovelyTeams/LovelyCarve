<template>
  <div class="property-list" :aria-label="nodeLabel(node)">
    <!-- 插件声明了属性角色：一张属性表，左列名称（下方是英文键名），右列值与来源。 -->
    <dl v-if="layout" class="property-sheet">
      <div v-for="record in records" :key="record.recordId" class="property-row" :data-record-id="record.recordId" :class="{ active: selectedRecordId === record.recordId }">
        <dt>
          <button class="property-label" type="button" :title="tr('查看记录详情')" @click="emit('openRecord', record.recordId)">{{ layout.label ? text(record, layout.label) : columnLabel(node, layout.value) }}</button>
          <small v-if="layout.secondary" class="property-key mono" :title="columnLabel(node, layout.secondary)">{{ text(record, layout.secondary) }}</small>
        </dt>
        <dd>
          <button class="property-value" :class="{ mono: monoKinds.has(layout.value.kind), empty: isEmptyValue(record.values[layout.value.id]) }" type="button" :title="tr('查看记录详情')" @click="emit('openRecord', record.recordId)"><span v-if="record.severity" class="ui-badge sm" :class="severityTone(record.severity)">{{ severityLabel(record.severity, tr) }}</span><span>{{ text(record, layout.value) }}</span></button>
          <span class="property-source"><button class="ui-btn link sm source-link" :disabled="locating || !sources(record).length" :title="sourceTitle(record)" @click="locate(record)"><LocateFixed :size="13" aria-hidden="true" /><span class="mono">{{ sources(record)[0]?.label || tr('无来源') }}</span></button><button v-if="sources(record).length > 1" class="ui-btn ghost sm source-more mono" type="button" :title="tr('查看全部来源（{0}）', { 0: sources(record).length })" :aria-label="tr('查看全部来源（{0}）', { 0: sources(record).length })" @click="emit('openRecord', record.recordId)">+{{ (sources(record).length - 1).toLocaleString() }}</button></span>
        </dd>
      </div>
    </dl>
    <!-- 没有属性角色：每条记录一张卡片，字段按可用宽度排成多列。 -->
    <div v-else class="record-sheets">
      <article v-for="record in records" :key="record.recordId" class="record-sheet" :data-record-id="record.recordId" :class="{ active: selectedRecordId === record.recordId }">
        <header class="record-sheet-head">
          <button class="property-label" type="button" :title="tr('查看记录详情')" @click="emit('openRecord', record.recordId)">{{ record.title }}</button>
          <span v-if="record.severity" class="ui-badge sm" :class="severityTone(record.severity)">{{ severityLabel(record.severity, tr) }}</span>
          <span class="property-source"><button class="ui-btn link sm source-link" :disabled="locating || !sources(record).length" :title="sourceTitle(record)" @click="locate(record)"><LocateFixed :size="13" aria-hidden="true" /><span class="mono">{{ sources(record)[0]?.label || tr('无来源') }}</span></button><button v-if="sources(record).length > 1" class="ui-btn ghost sm source-more mono" type="button" :title="tr('查看全部来源（{0}）', { 0: sources(record).length })" :aria-label="tr('查看全部来源（{0}）', { 0: sources(record).length })" @click="emit('openRecord', record.recordId)">+{{ (sources(record).length - 1).toLocaleString() }}</button></span>
        </header>
        <dl class="record-fields"><div v-for="column in visibleColumns" :key="column.id"><dt :title="columnLabel(node, column)">{{ columnLabel(node, column) }}</dt><dd :class="{ mono: monoKinds.has(column.kind), empty: isEmptyValue(record.values[column.id]) }">{{ text(record, column) }}</dd></div></dl>
      </article>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { LocateFixed } from 'lucide-vue-next';
import { tr } from '../../../composables/useLocale';
import type { AnalysisRecord, AnalysisResultNode, RecordColumn, RecordPropertyLayout, RecordFieldKind } from '../../../lib/ipc/analysisResults';
import type { FileLocationTarget } from '../../../lib/models/fileLocation';
import { useAnalysisLabels } from '../composables/useAnalysisLabels';
import { resolvePropertyLayout } from '../model/analysisPresentation';
import { analysisSources } from '../model/analysisSource';
import { isEmptyValue, recordValueText, severityLabel, severityTone } from '../model/analysisValue';

const props = defineProps<{ node: AnalysisResultNode; records: AnalysisRecord[]; columns: RecordColumn[]; properties: RecordPropertyLayout | null; selectedRecordId: string | null; locating?: boolean }>();
const emit = defineEmits<{ openRecord: [id: string]; openFile: [target: FileLocationTarget] }>();
const { nodeLabel, columnLabel, choiceLabel } = useAnalysisLabels();
const layout = computed(() => resolvePropertyLayout(props.properties, props.columns));
const visibleColumns = computed(() => props.columns.filter(column => !column.hidden));
const monoKinds = new Set<RecordFieldKind>(['int', 'float', 'size', 'duration', 'hash', 'ip', 'path', 'timestamp']);
const sources = (record: AnalysisRecord) => analysisSources(record, props.node.imageId);
const sourceTitle = (record: AnalysisRecord) => {
  const source = sources(record)[0];
  return source ? [tr('定位源文件'), source.label, source.note].filter(Boolean).join(' · ') : tr('此记录未提供源文件位置');
};
const text = (record: AnalysisRecord, column: RecordColumn) => recordValueText(record.values[column.id], column.kind, { detailed: true, choice: id => choiceLabel(props.node, column, id), tr });
function locate(record: AnalysisRecord) {
  const source = sources(record)[0];
  if (!props.locating && source) emit('openFile', source.target);
}
</script>

<style scoped>
.property-list { min-height: 100%; padding: 20px 24px 32px; background: var(--surface-primary); }
.property-sheet, .record-sheets { max-width: 1040px; margin: 0 auto; }

/* 属性表：一张卡片，行间细分隔线；选中行与表格一致（浅底 + 1px 主题色描边）。 */
.property-sheet { overflow: hidden; border: 1px solid var(--border-color); border-radius: var(--radius-lg); background: var(--surface-secondary); box-shadow: var(--card-shadow); }
.property-row { display: grid; grid-template-columns: 200px minmax(0, 1fr); gap: 24px; padding: 10px 16px; transition: background var(--transition-fast); }
.property-row + .property-row { border-top: 1px solid color-mix(in srgb, var(--border-color) 70%, transparent); }
.property-row:hover { background: var(--hover-bg); }
.property-row.active, .record-sheet.active { background: var(--sidebar-active-bg); outline: 1px solid color-mix(in srgb, var(--primary-color) 70%, transparent); outline-offset: -1px; }
.property-row dt { display: flex; flex-direction: column; align-items: flex-start; gap: 2px; min-width: 0; }
.property-row dd { display: flex; align-items: flex-start; justify-content: space-between; gap: 24px; min-width: 0; margin: 0; }
.property-label, .property-value { border: 0; padding: 0; background: transparent; color: var(--text-primary); font: inherit; text-align: left; cursor: pointer; overflow-wrap: anywhere; }
.property-label { color: var(--text-secondary); font-size: 12px; font-weight: 500; line-height: 20px; }
.property-value { display: flex; align-items: baseline; gap: 6px; min-width: 0; font-size: 13px; line-height: 20px; }
.property-value.mono { font-family: var(--font-mono); font-size: 12px; }
.property-value.empty { color: var(--text-secondary); }
.property-value .ui-badge { flex-shrink: 0; align-self: center; }
.property-value > span:last-child { min-width: 0; white-space: pre-wrap; }
.property-label:hover, .property-value:hover { color: var(--primary-color); }
.property-label:focus-visible, .property-value:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; border-radius: var(--radius-sm); }
.property-key { color: var(--text-secondary); font-size: 11px; line-height: 16px; overflow-wrap: anywhere; }
.property-source { display: inline-flex; align-items: center; flex-shrink: 0; max-width: 240px; min-width: 0; }
.source-link { --ui-btn-h: 24px; flex-shrink: 1; min-width: 0; max-width: 100%; color: var(--text-secondary); }
.property-row:is(:hover, :focus-within, .active) .source-link:not(:disabled), .record-sheet:is(:hover, :focus-within, .active) .source-link:not(:disabled) { color: var(--primary-color); }
.source-link > span { overflow: hidden; text-overflow: ellipsis; }
.source-more { --ui-btn-h: 24px; padding: 0 4px; font-size: 11px; }

/* 记录卡片：标题行，下面字段按宽度自动分列；值为空时退为次要色。 */
.record-sheets { display: flex; flex-direction: column; gap: 12px; }
.record-sheet { border: 1px solid var(--border-color); border-radius: var(--radius-lg); background: var(--surface-secondary); box-shadow: var(--card-shadow); }
.record-sheet-head { display: flex; align-items: center; gap: 8px; min-height: 44px; padding: 8px 12px 8px 16px; border-bottom: 1px solid color-mix(in srgb, var(--border-color) 70%, transparent); }
.record-sheet-head .property-label { min-width: 0; color: var(--text-primary); font-size: 13px; font-weight: 600; }
.record-sheet-head .property-label:hover { color: var(--primary-color); }
.record-sheet-head .property-source { max-width: 50%; margin-left: auto; }
.record-fields { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 12px 24px; margin: 0; padding: 12px 16px 16px; }
.record-fields > div { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.record-fields dt { overflow: hidden; color: var(--text-secondary); font-size: 11px; line-height: 16px; text-overflow: ellipsis; white-space: nowrap; }
.record-fields dd { margin: 0; color: var(--text-primary); font-size: 12px; line-height: 18px; overflow-wrap: anywhere; white-space: pre-wrap; }
.record-fields dd.mono { font-family: var(--font-mono); }
.record-fields dd.empty { color: var(--text-secondary); }

@container analysis-records (max-width: 900px) {
  .property-list { padding: 12px; }
  .property-row { grid-template-columns: 160px minmax(0, 1fr); gap: 16px; }
  .property-row dd { flex-direction: column; gap: 4px; }
}
@container analysis-records (max-width: 480px) {
  .property-row { grid-template-columns: minmax(0, 1fr); gap: 4px; }
  .record-sheet-head { flex-wrap: wrap; }
}
</style>
