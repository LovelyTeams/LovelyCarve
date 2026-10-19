<template>
  <div class="run-overview">
    <div class="overview ui-stagger">
      <!-- 运行头部：分析器、目标与插件版本；与镜像信息页的检材头部同一结构，不做卡片。 -->
      <header class="run-head">
        <span class="run-icon" aria-hidden="true"><ClipboardList :size="20" /></span>
        <div class="run-heading">
          <div class="run-title">
            <h2>{{ nodeLabel(node) }}</h2>
            <span v-if="node.legacy" class="ui-badge sm">{{ tr("旧版") }}</span>
            <span v-if="audit?.state === 'superseded'" class="ui-badge sm warn">{{ tr("已被替换") }}</span>
          </div>
          <p class="run-target">{{ targetText }}</p>
          <p class="run-meta">
            <span v-if="audit" class="mono">{{ audit.pluginId }} {{ audit.pluginVersion }}</span>
            <span v-if="completedLabel">{{ tr("完成于（本地）") }} <strong class="mono">{{ completedLabel }}</strong></span>
          </p>
          <p v-if="audit?.summary" class="run-summary">{{ audit.summary }}</p>
        </div>
      </header>

      <div v-if="audit && audit.syntheticBytes > 0" class="ui-alert danger" role="note"><ShieldAlert :size="14" aria-hidden="true" /><span>{{ tr("{0} 字节源数据读取失败，分析时以零填充；依据这些补零字节得出的记录不能视为原始内容。", { 0: audit.syntheticBytes.toLocaleString() }) }}</span></div>
      <div v-if="node.legacy" class="ui-alert info" role="note"><Info :size="14" aria-hidden="true" /><span>{{ tr("旧版本的分析结果没有运行信息。") }}</span></div>
      <div v-else-if="error" class="ui-alert danger audit-error" role="alert"><CircleAlert :size="14" aria-hidden="true" /><span>{{ tr("运行信息读取失败：{0}", { 0: error }) }}</span><button class="ui-btn sm" type="button" @click="emit('retry')">{{ tr("重试") }}</button></div>

      <dl class="ui-card run-stats">
        <div class="stat"><dt>{{ tr("记录数") }}</dt><dd>{{ recordTotal.toLocaleString() }}</dd></div>
        <div class="stat"><dt>{{ tr("记录类型") }}</dt><dd>{{ outputs.length.toLocaleString() }}</dd></div>
        <div v-if="audit" class="stat"><dt>{{ tr("输入文件") }}</dt><dd>{{ audit.inputCount.toLocaleString() }}</dd></div>
        <div v-if="!node.legacy" class="stat" :class="{ warn: warningTotal > 0 }"><dt>{{ tr("告警") }}</dt><dd>{{ warningTotal.toLocaleString() }}</dd></div>
        <div v-if="elapsed" class="stat"><dt>{{ tr("耗时") }}</dt><dd>{{ elapsed }}</dd></div>
      </dl>

      <div class="overview-grid">
        <div class="overview-column">
          <section class="ui-card" :aria-label="tr('产出')">
            <header class="ui-card-header"><h3 class="ui-card-title"><Table2 :size="15" aria-hidden="true" />{{ tr("产出") }}</h3><span class="ui-count">{{ outputs.length.toLocaleString() }}</span></header>
            <ul v-if="outputs.length" class="output-list">
              <li v-for="item in outputs" :key="item.node.nodeId">
                <button class="output-row" :class="{ empty: item.count === 0 }" type="button" :title="tr('查看{0}', { 0: item.label })" @click="emit('openResult', item.node)">
                  <span class="output-name">{{ item.label }}</span>
                  <span class="output-bar" aria-hidden="true"><span v-if="item.count" :style="{ width: `${item.share}%` }"></span></span>
                  <span class="output-count mono">{{ item.count.toLocaleString() }}</span>
                  <ChevronRight :size="14" class="output-arrow" aria-hidden="true" />
                </button>
              </li>
            </ul>
            <p v-else class="card-note">{{ tr("这次运行没有产出记录。") }}</p>
          </section>

          <section v-if="audit?.warnings.length" class="ui-card" :aria-label="tr('告警')">
            <header class="ui-card-header"><h3 class="ui-card-title"><TriangleAlert :size="15" aria-hidden="true" />{{ tr("告警") }}</h3><span class="ui-count">{{ audit.warningCount.toLocaleString() }}</span></header>
            <p v-if="audit.warnings.length < audit.warningCount" class="card-note">{{ tr("只列出前 {0} 条告警。", { 0: audit.warnings.length.toLocaleString() }) }}</p>
            <ul class="detail-list">
              <li v-for="(warning, index) in shownWarnings" :key="index" class="warning-row">
                <span class="ui-badge sm warn mono">{{ warning.code }}</span>
                <span class="warning-text">{{ warning.message }}</span>
                <button v-if="warningTarget(warning)" class="ui-btn link sm location" :disabled="locating" :title="warning.path || tr('定位')" @click="emit('openFile', warningTarget(warning)!)"><LocateFixed :size="13" aria-hidden="true" /><span class="mono">{{ warning.path || tr("定位") }}</span></button>
                <span v-else-if="warning.path" class="location-text mono">{{ warning.path }}</span>
              </li>
            </ul>
            <button v-if="audit.warnings.length > LIST_PREVIEW" class="ui-btn link sm list-toggle" type="button" @click="allWarnings = !allWarnings">{{ allWarnings ? tr("收起") : tr("显示全部（{0}）", { 0: audit.warnings.length.toLocaleString() }) }}</button>
          </section>

          <section v-if="audit?.inputs.length" class="ui-card" :aria-label="tr('输入文件')">
            <header class="ui-card-header"><h3 class="ui-card-title"><FileInput :size="15" aria-hidden="true" />{{ tr("输入文件") }}</h3><span class="ui-count">{{ audit.inputCount.toLocaleString() }}</span></header>
            <p v-if="audit.inputs.length < audit.inputCount" class="card-note">{{ tr("只列出前 {0} 个输入文件。", { 0: audit.inputs.length.toLocaleString() }) }}</p>
            <ul class="detail-list">
              <li v-for="input in shownInputs" :key="input.path" class="input-row">
                <button v-if="inputTarget(input)" class="ui-btn link sm location" :disabled="locating" :title="input.path" @click="emit('openFile', inputTarget(input)!)"><LocateFixed :size="13" aria-hidden="true" /><span class="mono">{{ input.path }}</span></button>
                <span v-else class="location-text mono" :title="input.path">{{ input.path }}</span>
                <span class="input-size mono">{{ formatBytes(input.size) }}</span>
              </li>
            </ul>
            <button v-if="audit.inputs.length > LIST_PREVIEW" class="ui-btn link sm list-toggle" type="button" @click="allInputs = !allInputs">{{ allInputs ? tr("收起") : tr("显示全部（{0}）", { 0: audit.inputs.length.toLocaleString() }) }}</button>
          </section>
        </div>

        <div class="overview-column">
          <section v-if="!node.legacy && loading" class="ui-card loading-card" role="status"><Loader2 :size="14" class="spin" aria-hidden="true" />{{ tr("正在读取运行信息…") }}</section>
          <template v-if="audit">
            <section class="ui-card" :aria-label="tr('运行参数')">
              <header class="ui-card-header"><h3 class="ui-card-title"><SlidersHorizontal :size="15" aria-hidden="true" />{{ tr("运行参数") }}</h3></header>
              <div class="card-body">
                <div>
                  <h4 class="group-title">{{ tr("选项") }}</h4>
                  <FileDetailFields v-if="optionFields.length" :fields="optionFields" />
                  <p v-else class="group-note">{{ tr("使用默认选项。") }}</p>
                </div>
                <div v-if="profileFields.length">
                  <h4 class="group-title">{{ tr("画像事实") }}</h4>
                  <FileDetailFields :fields="profileFields" />
                </div>
              </div>
            </section>
            <section v-if="audit.syntheticBytes || audit.badRegions.length" class="ui-card" :aria-label="tr('读不出的源字节')">
              <header class="ui-card-header"><h3 class="ui-card-title"><ShieldAlert :size="15" aria-hidden="true" />{{ tr("读不出的源字节") }}</h3></header>
              <div class="card-body">
                <FileDetailFields :fields="[
                  { label: tr('补零字节'), value: audit.syntheticBytes.toLocaleString(), mono: true, warning: true },
                  ...audit.badRegions.map((region, index) => ({ label: tr('区域 {0}', { 0: index + 1 }), value: `0x${region.offset.toString(16)} + ${region.len.toLocaleString()}`, mono: true })),
                ]" />
              </div>
            </section>
            <section class="ui-card" :aria-label="tr('技术信息')">
              <header class="ui-card-header"><h3 class="ui-card-title"><Fingerprint :size="15" aria-hidden="true" />{{ tr("技术信息") }}</h3></header>
              <div class="card-body">
                <FileDetailFields :fields="[
                  { label: tr('运行 ID'), value: audit.runId, mono: true, copyable: true },
                  { label: tr('分析器'), value: `${audit.pluginId}/${audit.analyzer}`, mono: true, copyable: true },
                  { label: tr('结构哈希'), value: audit.schemaHash, mono: true, copyable: true },
                  { label: tr('开始'), value: timestampText(audit.startedAt), mono: true },
                  { label: tr('结束'), value: audit.finishedAt ? timestampText(audit.finishedAt) : '—', mono: true },
                  { label: tr('发布'), value: audit.publishedAt ? timestampText(audit.publishedAt) : '—', mono: true },
                ]" />
              </div>
            </section>
          </template>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { tr, useLocale } from '../../../composables/useLocale';

/** One analyzer run: its target and plugin, what it produced (each record
 *  type opens from here), its warnings and inputs, the options and profile
 *  facts it ran with, and the source bytes it could only read as zeroes.
 *  The tree node alone carries the outputs, so results of an earlier
 *  version, which have no audit, still get their overview. */
import { computed, ref, watch } from 'vue';
import { ChevronRight, CircleAlert, ClipboardList, FileInput, Fingerprint, Info, Loader2, LocateFixed, ShieldAlert, SlidersHorizontal, Table2, TriangleAlert } from 'lucide-vue-next';
import FileDetailFields, { type FileDetailField } from '../../../components/layout/FileDetailFields.vue';
import type { AnalysisResultNode, AnalysisRunAudit, AnalysisRunInput, AnalysisRunWarning } from '../../../lib/ipc/analysisResults';
import type { FileLocationTarget } from '../../../lib/models/fileLocation';
import { pluginTextKey } from '../../../lib/models/pluginText';
import { formatBytes } from '../../../utils/format';
import { usePluginCatalog } from '../../tasks/api';
import { useAnalysisLabels } from '../composables/useAnalysisLabels';
import { durationText, timestampText } from '../model/analysisValue';

const props = defineProps<{ node: AnalysisResultNode; audit: AnalysisRunAudit | null; loading?: boolean; error?: string | null; locating?: boolean }>();
const emit = defineEmits<{ openFile: [target: FileLocationTarget]; openResult: [node: AnalysisResultNode]; retry: [] }>();
const catalog = usePluginCatalog();
const { nodeLabel } = useAnalysisLabels();
const { locale } = useLocale();

/** Long warning and input lists open on their first rows. */
const LIST_PREVIEW = 8;
const allWarnings = ref(false);
const allInputs = ref(false);
watch(() => props.node.nodeId, () => { allWarnings.value = false; allInputs.value = false; });
const shownWarnings = computed(() => allWarnings.value ? props.audit?.warnings ?? [] : props.audit?.warnings.slice(0, LIST_PREVIEW) ?? []);
const shownInputs = computed(() => allInputs.value ? props.audit?.inputs ?? [] : props.audit?.inputs.slice(0, LIST_PREVIEW) ?? []);

const targetText = computed(() => {
  if (props.audit?.target.kind === 'evidence') return tr("整个检材");
  return props.node.targetLabel || props.audit?.target.label || tr("整个检材");
});

/** Record types in the plugin's order; the bar compares each with the largest. */
const outputs = computed(() => {
  const results = props.node.children.filter(child => child.kind === 'result');
  const largest = Math.max(1, ...results.map(child => child.childCount));
  return results.map(child => ({
    node: child,
    label: nodeLabel(child),
    count: child.childCount,
    share: child.childCount ? Math.max(1.5, child.childCount / largest * 100) : 0,
  }));
});
const recordTotal = computed(() => props.audit?.recordCount ?? outputs.value.reduce((sum, item) => sum + item.count, 0));
const warningTotal = computed(() => props.audit?.warningCount ?? props.node.warningCount);

function localTime(value: string | null | undefined): string {
  if (!value) return '';
  const time = new Date(value);
  return Number.isNaN(time.getTime()) ? '' : time.toLocaleString(locale.value, { hour12: false });
}
const completedLabel = computed(() => localTime(props.audit?.publishedAt ?? props.audit?.finishedAt ?? props.node.completedAt));
const elapsed = computed(() => {
  const audit = props.audit;
  if (!audit?.finishedAt) return '';
  const seconds = (Date.parse(audit.finishedAt) - Date.parse(audit.startedAt)) / 1000;
  return Number.isFinite(seconds) && seconds >= 0 ? durationText(Math.round(seconds * 10) / 10) : '';
});

function optionText(value: unknown): string {
  if (Array.isArray(value)) return value.length ? value.map(item => String(item)).join(', ') : tr("全部");
  if (typeof value === 'boolean') return value ? tr("是") : tr("否");
  return typeof value === 'string' ? value : JSON.stringify(value);
}
const optionFields = computed<FileDetailField[]>(() => Object.entries(props.audit?.options ?? {}).map(([id, value]) => ({
  key: id,
  label: catalog.text(props.node.pluginId, pluginTextKey.option(props.node.analyzer, id), id),
  value: optionText(value),
})));
const profileFields = computed<FileDetailField[]>(() => Object.entries(props.audit?.profile ?? {}).map(([fact, value]) => ({
  key: fact,
  label: fact,
  value: optionText(value),
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
.run-overview { flex: 1; min-width: 0; min-height: 0; overflow-y: auto; padding: 20px 24px 28px; background: var(--surface-primary); container: run-overview / inline-size; }
.overview { display: flex; flex-direction: column; gap: 16px; max-width: 1200px; margin-inline: auto; --ui-stagger-step: 50ms; }

.run-head { display: grid; grid-template-columns: auto minmax(0, 1fr); align-items: start; gap: 14px; padding: 4px 4px 0; }
.run-icon { display: grid; place-items: center; width: 40px; height: 40px; border-radius: var(--radius-md); background: var(--sev-info-bg); color: var(--primary-color); }
.run-heading { min-width: 0; }
.run-title { display: flex; align-items: center; gap: 8px; min-width: 0; }
.run-title h2 { min-width: 0; margin: 0; overflow: hidden; font-size: 16px; font-weight: 600; line-height: 1.4; text-overflow: ellipsis; white-space: nowrap; }
.run-target { margin: 2px 0 0; color: var(--text-primary); font-size: 13px; line-height: 1.5; overflow-wrap: anywhere; }
.run-meta { display: flex; flex-wrap: wrap; gap: 4px 16px; margin: 4px 0 0; color: var(--text-secondary); font-size: 11px; line-height: 1.6; }
.run-meta strong { color: var(--text-primary); font-weight: 500; }
.run-summary { margin: 8px 0 0; color: var(--text-primary); font-size: 12px; line-height: 1.6; overflow-wrap: anywhere; }
.audit-error { align-items: center; }
.audit-error > span { flex: 1; min-width: 0; overflow-wrap: anywhere; }

/* 关键数字：一条卡片里并列，格之间用细分隔线。 */
.run-stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); margin: 0; overflow: hidden; }
.stat { display: flex; flex-direction: column; gap: 4px; min-width: 0; padding: 12px 16px; box-shadow: -1px 0 0 var(--border-color), 0 -1px 0 var(--border-color); }
.stat dt { color: var(--text-secondary); font-size: 11px; }
.stat dd { margin: 0; color: var(--text-primary); font-size: 20px; font-weight: 600; line-height: 1.2; letter-spacing: -0.01em; font-variant-numeric: tabular-nums; }
.stat.warn dd { color: var(--sev-warn-fg); }

.overview-grid { display: grid; grid-template-columns: minmax(0, 3fr) minmax(0, 2fr); align-items: start; gap: 16px; }
.overview-column { display: flex; flex-direction: column; gap: 16px; min-width: 0; }
.ui-card-header .ui-count { margin-left: auto; }
.card-body { display: flex; flex-direction: column; gap: 14px; padding: 0 16px 16px; }
.card-note, .group-note { margin: 0; color: var(--text-secondary); font-size: 11px; line-height: 1.6; }
.card-note { padding: 0 16px 8px; }
.group-title { margin: 0 0 2px; color: var(--text-secondary); font-size: 11px; font-weight: 600; }
.loading-card { display: flex; align-items: center; gap: 8px; padding: 14px 16px; color: var(--text-secondary); font-size: 12px; }

/* 产出：每种记录类型一行，条形按最多的一种折算，点击打开该记录类型。 */
.output-list, .detail-list { margin: 0; padding: 0 0 8px; list-style: none; }
.output-row { display: grid; grid-template-columns: minmax(96px, 2fr) minmax(48px, 3fr) auto 14px; align-items: center; gap: 12px; width: 100%; height: 32px; padding: 0 16px; border: 0; background: transparent; color: var(--text-primary); font: inherit; font-size: 12px; text-align: left; cursor: pointer; transition: background var(--transition-fast); }
.output-row:hover { background: var(--hover-bg); }
.output-row:focus-visible { outline: 2px solid var(--primary-color); outline-offset: -2px; }
.output-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.output-bar { height: 6px; overflow: hidden; border-radius: 999px; background: var(--surface-tertiary); }
.output-bar > span { display: block; height: 100%; border-radius: inherit; background: color-mix(in srgb, var(--primary-color) 55%, transparent); transform-origin: left center; }
:root[data-motion='full'] .output-bar > span { animation: ui-grow-x var(--motion-slow) var(--ease-out) backwards; }
.output-count { min-width: 40px; color: var(--text-primary); font-size: 12px; text-align: right; font-variant-numeric: tabular-nums; }
.output-arrow { color: var(--text-secondary); opacity: 0; transition: opacity var(--transition-fast); }
.output-row:is(:hover, :focus-visible) .output-arrow { opacity: 1; color: var(--primary-color); }
.output-row.empty .output-name, .output-row.empty .output-count { color: var(--text-secondary); }

.warning-row, .input-row { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 8px; min-width: 0; padding: 8px 16px; font-size: 12px; }
.warning-row + .warning-row, .input-row + .input-row { border-top: 1px solid color-mix(in srgb, var(--border-color) 70%, transparent); }
.warning-text { flex: 1 1 200px; min-width: 0; line-height: 1.6; overflow-wrap: anywhere; }
.location { max-width: 100%; min-width: 0; height: auto; min-height: 24px; padding: 0 4px; text-align: left; }
.location > span { min-width: 0; overflow-wrap: anywhere; white-space: normal; }
.location-text { min-width: 0; color: var(--text-secondary); font-size: 11px; overflow-wrap: anywhere; }
.input-row .location, .input-row .location-text { flex: 1; }
.input-size { flex-shrink: 0; margin-left: auto; color: var(--text-secondary); font-size: 11px; font-variant-numeric: tabular-nums; }
.list-toggle { margin: 0 12px 12px; }

@container run-overview (max-width: 900px) {
  .overview-grid { grid-template-columns: minmax(0, 1fr); }
}
@container run-overview (max-width: 560px) {
  .run-overview { padding: 16px 12px 24px; }
  .output-row { grid-template-columns: minmax(0, 1fr) auto 14px; }
  .output-bar { display: none; }
}
</style>
