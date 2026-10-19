<template>
  <div class="run-audit">
    <FileDetailLayout :title="nodeLabel(node)">
      <template #icon><ClipboardList :size="20" /></template>
      <template #badges>
        <span class="ui-badge" :class="audit.state === 'published' ? 'success' : 'warn'">{{ audit.state === 'published' ? tr("已发布") : tr("已被替换") }}</span>
        <span v-if="audit.warningCount" class="ui-badge warn">{{ tr("{0} 条告警", { 0: audit.warningCount.toLocaleString() }) }}</span>
        <span v-if="audit.syntheticBytes" class="ui-badge danger">{{ tr("有读不出的源字节") }}</span>
      </template>
      <FileDetailSection :title="tr('运行')" wide>
        <FileDetailFields :fields="[
          { label: tr('目标'), value: targetText },
          { label: tr('插件'), value: `${audit.pluginId} v${audit.pluginVersion}`, mono: true },
          { label: tr('摘要'), value: audit.summary || '—' },
          { label: tr('开始'), value: audit.startedAt, mono: true },
          { label: tr('结束'), value: audit.finishedAt ?? '—', mono: true },
          { label: tr('发布'), value: audit.publishedAt ?? '—', mono: true },
        ]" />
      </FileDetailSection>
      <FileDetailSection :title="tr('产出')" wide>
        <FileDetailFields :fields="[
          { label: tr('记录'), value: tr('{0} 条', { 0: audit.recordCount.toLocaleString() }) },
          ...audit.recordTypes.map(type => ({ label: recordTypeLabel(type.recordType, type.label), value: type.count.toLocaleString() })),
          { label: tr('输入文件'), value: audit.inputCount.toLocaleString() },
        ]" />
      </FileDetailSection>
      <FileDetailSection v-if="audit.warnings.length" :title="tr('告警')" wide>
        <p v-if="audit.warnings.length < audit.warningCount" class="audit-note">{{ tr("只列出前 {0} 条告警。", { 0: audit.warnings.length.toLocaleString() }) }}</p>
        <ul class="audit-list">
          <li v-for="(warning, index) in audit.warnings" :key="index">
            <span class="ui-badge sm warn mono">{{ warning.code }}</span>
            <span class="audit-text">{{ warning.message }}</span>
            <button v-if="warningTarget(warning)" class="ui-btn link sm" :disabled="locating" :title="warning.path ?? ''" @click="emit('openFile', warningTarget(warning)!)"><LocateFixed :size="13" />{{ warning.path || tr("定位") }}</button>
            <span v-else-if="warning.path" class="audit-path mono">{{ warning.path }}</span>
          </li>
        </ul>
      </FileDetailSection>
      <FileDetailSection v-if="audit.inputs.length" :title="tr('输入文件')" collapsible wide>
        <ul class="audit-list">
          <li v-for="input in audit.inputs" :key="input.path">
            <button v-if="inputTarget(input)" class="ui-btn link sm" :disabled="locating" :title="input.path" @click="emit('openFile', inputTarget(input)!)"><LocateFixed :size="13" /><span class="mono">{{ input.path }}</span></button>
            <span v-else class="audit-path mono">{{ input.path }}</span>
            <span class="audit-size">{{ formatBytes(input.size) }}</span>
          </li>
        </ul>
      </FileDetailSection>
      <FileDetailSection v-if="optionFields.length || profileFields.length" :title="tr('选项与画像事实')" collapsible wide>
        <FileDetailFields :fields="[...optionFields, ...profileFields]" />
      </FileDetailSection>
      <FileDetailSection v-if="audit.syntheticBytes || audit.badRegions.length" :title="tr('读不出的源字节')" wide>
        <p class="audit-note">{{ tr("这些源字节读取失败，分析时以零填充；基于它们的记录不能视为原始内容。") }}</p>
        <FileDetailFields :fields="[
          { label: tr('合成字节'), value: audit.syntheticBytes.toLocaleString(), mono: true, warning: true },
          ...audit.badRegions.map((region, index) => ({ label: tr('区域 {0}', { 0: index + 1 }), value: `0x${region.offset.toString(16)} + ${region.len.toLocaleString()}`, mono: true })),
        ]" />
      </FileDetailSection>
      <FileDetailSection :title="tr('技术信息')" collapsible wide>
        <FileDetailFields :fields="[
          { label: tr('运行 ID'), value: audit.runId, mono: true, copyable: true },
          { label: tr('分析器'), value: `${audit.pluginId}/${audit.analyzer}`, mono: true, copyable: true },
          { label: tr('结构哈希'), value: audit.schemaHash, mono: true, copyable: true },
        ]" />
      </FileDetailSection>
    </FileDetailLayout>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

/** A run's audit: its target, what it was given and produced, its warnings
 *  and the source bytes it could only read as zeroes. */
import { computed } from 'vue';
import { ClipboardList, LocateFixed } from 'lucide-vue-next';
import FileDetailLayout from '../../../components/layout/FileDetailLayout.vue';
import FileDetailSection from '../../../components/layout/FileDetailSection.vue';
import FileDetailFields, { type FileDetailField } from '../../../components/layout/FileDetailFields.vue';
import type { AnalysisResultNode, AnalysisRunAudit, AnalysisRunInput, AnalysisRunWarning } from '../../../lib/ipc/analysisResults';
import type { FileLocationTarget } from '../../../lib/models/fileLocation';
import { pluginTextKey } from '../../../lib/models/pluginText';
import { formatBytes } from '../../../utils/format';
import { usePluginCatalog } from '../../tasks/api';
import { useAnalysisLabels } from '../composables/useAnalysisLabels';

const props = defineProps<{ audit: AnalysisRunAudit; node: AnalysisResultNode; locating?: boolean }>();
const emit = defineEmits<{ openFile: [target: FileLocationTarget] }>();
const catalog = usePluginCatalog();
const { nodeLabel } = useAnalysisLabels();

const targetText = computed(() => {
  const target = props.audit.target;
  if (target.kind === 'evidence') return tr("整个检材");
  return target.label || target.id;
});

function recordTypeLabel(recordType: string, label: string): string {
  return catalog.text(props.audit.pluginId, pluginTextKey.record(recordType), label);
}

function valueText(value: unknown): string {
  if (Array.isArray(value)) return value.length ? value.map(item => String(item)).join(', ') : tr("全部");
  if (typeof value === 'boolean') return value ? tr("是") : tr("否");
  return typeof value === 'string' ? value : JSON.stringify(value);
}

const optionFields = computed<FileDetailField[]>(() => Object.entries(props.audit.options).map(([id, value]) => ({
  label: catalog.text(props.audit.pluginId, pluginTextKey.option(props.audit.analyzer, id), id),
  value: valueText(value),
})));

const profileFields = computed<FileDetailField[]>(() => Object.entries(props.audit.profile).map(([fact, value]) => ({
  label: tr("事实 {0}", { 0: fact }),
  value: valueText(value),
  mono: true,
})));

function inputTarget(input: AnalysisRunInput): FileLocationTarget | null {
  if (input.root && input.rootPath) return { imageId: props.node.imageId, rootId: input.root, path: input.rootPath };
  return input.nodeId ? { imageId: props.node.imageId, nodeId: input.nodeId } : null;
}

function warningTarget(warning: AnalysisRunWarning): FileLocationTarget | null {
  return warning.nodeId ? { imageId: props.node.imageId, nodeId: warning.nodeId } : null;
}
</script>

<style scoped>
.run-audit { flex: 1; min-height: 0; overflow-y: auto; padding: 16px; background: var(--surface-primary); }
.audit-note { margin: 0 0 8px; color: var(--text-secondary); font-size: 12px; line-height: 1.6; }
.audit-list { display: flex; flex-direction: column; gap: 6px; margin: 0; padding: 0; list-style: none; }
.audit-list li { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; min-width: 0; font-size: 12px; }
.audit-list .ui-btn.link { max-width: 100%; height: auto; text-align: left; }
.audit-list .ui-btn.link span, .audit-path { overflow-wrap: anywhere; white-space: normal; }
.audit-text { flex: 1; min-width: 160px; overflow-wrap: anywhere; }
.audit-path { color: var(--text-secondary); font-size: 11px; }
.audit-size { color: var(--text-secondary); font-size: 11px; font-variant-numeric: tabular-nums; }
</style>
