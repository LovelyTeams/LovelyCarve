<template>
  <section class="task-center" :aria-label="tr('任务列表')">
    <div class="task-toolbar">
      <label class="ui-search task-search"><Search :size="14" /><input v-model="search" :aria-label="tr('搜索任务')" :placeholder="tr('搜索任务或文件')" /><button v-if="search" class="ui-icon-btn sm" :aria-label="tr('清除任务搜索')" @click="search = ''"><X :size="13" /></button></label>
      <button v-if="internalCount" class="ui-btn sm ghost" :aria-pressed="showInternal" :title="showInternal ? tr('隐藏调度器内部任务') : tr('显示调度器内部任务')" @click="showInternal = !showInternal"><component :is="showInternal ? Eye : EyeOff" :size="13" />{{ tr("内部") }} {{ internalCount }}</button>
      <button class="ui-icon-btn" :disabled="loading" :aria-label="tr('刷新任务')" :title="tr('刷新')" @click="refresh"><Loader2 v-if="loading" :size="15" class="spin" /><RefreshCw v-else :size="15" /></button>
    </div>
    <div class="task-scroll" :aria-busy="loading">
      <div v-if="error" class="ui-empty state-box error"><span class="ui-empty-icon"><CircleAlert :size="22" /></span><p class="ui-empty-title">{{ tr("读取任务失败") }}</p><p class="ui-empty-desc">{{ error }}</p><div class="ui-empty-actions"><button class="ui-btn sm" @click="refresh">{{ tr("重试") }}</button></div></div>
      <div v-else-if="loading && !visibleTasks.length" class="ui-empty state-box"><span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span><p class="ui-empty-desc">{{ tr("正在读取任务…") }}</p></div>
      <div v-else-if="!visibleTasks.length" class="ui-empty state-box">
        <span class="ui-empty-icon accent"><ListTodo :size="24" /></span>
        <p class="ui-empty-title">{{ scopedTasks.length ? tr("没有匹配的任务") : tr("当前检材还没有任务") }}</p>
        <p class="ui-empty-desc">{{ scopedTasks.length ? tr("换个关键词再试。") : tr("在快速分析中选择功能，进度与结果会显示在这里。") }}</p>
        <div class="ui-empty-actions">
          <button v-if="scopedTasks.length" class="ui-btn sm" @click="resetFilters">{{ tr("清除搜索") }}</button>
          <button v-else class="ui-btn sm primary" @click="emit('quickAnalysis')"><Zap :size="13" />{{ tr("快速分析") }}</button>
        </div>
      </div>
      <template v-else>
        <!-- One card per section, laid out like the image-info cards: icon title and count,
             then rows split by hairlines. The section and the row's badge carry the status. -->
        <section v-for="section in sections" :key="section.key" class="ui-card task-card" :class="section.key" :aria-label="tr('{0} {1} 项', { 0: tr(section.label), 1: section.tasks.length })">
          <header class="card-head">
            <h3 class="ui-card-title"><component :is="SECTION_ICON[section.key]" :size="15" />{{ tr(section.label) }}</h3>
            <span class="ui-count" :class="{ accent: section.key === 'active' }">{{ section.tasks.length }}</span>
            <button type="button" class="ui-icon-btn sm card-toggle" :aria-expanded="!collapsed.has(section.key)" :aria-label="`${collapsed.has(section.key) ? tr('展开') : tr('收起')}${tr(section.label)}`" :title="collapsed.has(section.key) ? tr('展开') : tr('收起')" @click="toggleSection(section.key)"><ChevronDown :size="14" /></button>
          </header>
          <UiTransition><div v-if="!collapsed.has(section.key)" class="task-list">
            <article v-for="row in section.rows" :key="row.entry.id" class="task-row" :class="{ expanded: expandedIds.has(row.entry.id) }" :style="{ '--tone': TASK_STATUS_TONE[row.entry.status] }" :aria-label="row.name">
              <div class="task-main">
                <button class="task-open" :aria-expanded="expandedIds.has(row.entry.id)" :aria-controls="detailId(row.entry)" :aria-label="tr('{0} {1} 详情', { 0: expandedIds.has(row.entry.id) ? tr('收起') : tr('展开'), 1: row.name })" @click="toggleDetail(row.entry.id)">
                  <span class="task-line">
                    <strong class="task-name" :title="row.name">{{ row.name }}</strong>
                    <span v-if="row.badge" class="ui-badge sm tone task-badge"><Loader2 v-if="row.entry.status === 'running' && !row.entry.cancelling" :size="11" class="spin" />{{ row.badge }}</span>
                  </span>
                  <span v-if="!(row.entry.error && expandedIds.has(row.entry.id))" class="task-line task-sub"><span class="task-summary" :title="row.summary">{{ row.summary }}</span><span class="task-meta mono">{{ row.meta }}</span></span>
                  <span v-if="row.showsProgress" class="ui-progress task-progress" :class="{ indeterminate: row.indeterminate }" role="progressbar" :aria-label="tr('{0}进度', { 0: row.name })" :aria-valuenow="row.indeterminate ? undefined : row.percent" :aria-valuetext="row.indeterminate ? tr('处理中') : undefined" aria-valuemin="0" aria-valuemax="100"><span :style="row.indeterminate ? undefined : { width: `${row.percent}%` }" /></span>
                </button>
                <button v-if="row.action" class="ui-btn sm task-action" :disabled="pendingIds.has(row.entry.id)" :title="actionLabel(row.action, row.target)" :aria-label="`${actionLabel(row.action, row.target)} ${row.name}`" @click="runTaskAction(row.entry, row.action)"><Loader2 v-if="pendingIds.has(row.entry.id)" :size="13" class="spin" /><component v-else :is="ACTION_ICON[row.action]" :size="13" />{{ ACTION_TEXT[row.action] }}</button>
              </div>
              <UiTransition><div v-if="expandedIds.has(row.entry.id)" :id="detailId(row.entry)" class="task-detail">
                <div v-if="row.entry.error" class="ui-alert danger detail-error"><CircleAlert :size="14" /><span>{{ row.entry.error }}</span></div>
                <dl class="detail-list">
                  <div v-for="field in row.details" :key="field.label" class="detail-row"><dt>{{ field.label }}</dt><dd :class="{ mono: field.mono }">{{ field.value }}</dd></div>
                </dl>
                <div class="detail-actions">
                  <button v-if="row.removable" class="ui-btn sm ghost danger-soft detail-remove" :disabled="pendingIds.has(row.entry.id)" @click="runTaskAction(row.entry, 'remove')"><Trash2 :size="13" />{{ tr("删除任务") }}</button>
                  <button v-if="row.action !== 'cancel' && offers(row.entry, 'cancel')" class="ui-btn sm" :disabled="pendingIds.has(row.entry.id)" @click="runTaskAction(row.entry, 'cancel')"><X :size="13" />{{ tr("取消任务") }}</button>
                  <button v-if="row.entry.source === 'plugin' && row.action !== 'restart' && offers(row.entry, 'restart')" class="ui-btn sm" :disabled="pendingIds.has(row.entry.id)" @click="runTaskAction(row.entry, 'restart')"><RotateCcw :size="13" />{{ tr("重新分析") }}</button>
                  <button v-if="row.target && row.action !== 'open'" class="ui-btn sm" @click="openResult(row.entry)"><ArrowUpRight :size="13" />{{ tr(TASK_RESULT_LABEL[row.target]) }}</button>
                </div>
                <details v-disclosure-motion class="card-disclosure technical-details"><summary><ChevronDown :size="14" />{{ tr("技术信息") }}</summary><dl class="detail-list"><div v-for="field in row.technical" :key="field.label" class="detail-row"><dt>{{ field.label }}</dt><dd :class="{ mono: field.mono }">{{ field.value }}</dd></div></dl></details>
              </div></UiTransition>
            </article>
            <button v-if="section.hidden" type="button" class="card-more" @click="showAllFinished = true"><ChevronDown :size="14" />{{ tr("显示其余") }} {{ section.hidden }} {{ tr("项") }}</button>
          </div></UiTransition>
        </section>
        <section v-if="notices.length" class="ui-card task-card notices-card" :aria-label="tr('最近通知 {0} 条', { 0: notices.length })">
          <header class="card-head">
            <h3 class="ui-card-title"><Bell :size="15" />{{ tr("最近通知") }}</h3>
            <span class="ui-count">{{ notices.length }}</span>
            <button type="button" class="ui-icon-btn sm card-toggle" :aria-expanded="noticesOpen" :aria-label="noticesOpen ? tr('收起最近通知') : tr('展开最近通知')" :title="noticesOpen ? tr('收起') : tr('展开')" @click="noticesOpen = !noticesOpen"><ChevronDown :size="14" /></button>
          </header>
          <UiTransition><dl v-if="noticesOpen" class="detail-list notice-list"><div v-for="(notice, index) in notices" :key="index" class="detail-row"><dt>{{ NOTICE_LABEL[notice.kind] }}</dt><dd>{{ notice.message }}</dd></div></dl></UiTransition>
        </section>
      </template>
    </div>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';
import { joinList } from '../../../i18n/translate';

import UiTransition from '../../../components/ui/UiTransition.vue';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import { computed, onBeforeUnmount, onMounted, ref, watch, type Component } from 'vue';
import {
  Activity,
  ArrowUpRight,
  Bell,
  ChevronDown,
  CircleAlert,
  Eye,
  EyeOff,
  History,
  ListTodo,
  Loader2,
  Pause,
  Play,
  RefreshCw,
  RotateCcw,
  Search,
  Trash2,
  TriangleAlert,
  X,
  Zap,
} from 'lucide-vue-next';
import { useUnifiedTaskCenter } from '../composables/useUnifiedTaskCenter';
import { useTaskCenter } from '../composables/useTaskCenter';
import { usePluginCatalog } from '../composables/usePluginCatalog';
import { pluginTextKey } from '../../../lib/models/pluginText';
import { confirm } from '../../../composables/useConfirm';
import {
  TASK_RESULT_LABEL,
  TASK_STATUS_TONE,
  formatTaskClock,
  formatTaskDateTime,
  formatTaskDuration,
  groupTaskSections,
  type TaskResultPanel,
  type TaskSectionKey,
} from '../model/taskCenterView';
import {
  builtinMessage,
  builtinSubject,
  builtinTaskTitle,
  taskEntryElapsedSecs,
  taskEntryIndeterminate,
  taskEntryPercent,
  taskEntryResultPanel,
} from '../model/taskEntryView';
import type { SchedNoticeKind, SchedTaskRecord, SchedTaskStatus } from '../../../lib/ipc/tasks';
import type { PluginTaskEntry, TaskAction, TaskEntry } from '../../../lib/ipc/taskRuntime';
import { TASK_STATUS_LABEL } from '../model/taskStatus';

const props = defineProps<{ imageId: string | null }>();
/** `openResult` carries the plugin run so the analysis page can open its results. */
const emit = defineEmits<{ quickAnalysis: []; openResult: [panel: TaskResultPanel, analyzer: string | null] }>();

/** Finished history beyond this stays folded until asked for (searching shows every match). */
const FINISHED_PREVIEW = 6;

const search = ref('');
const showInternal = ref(false);
const showAllFinished = ref(false);
const noticesOpen = ref(false);
const collapsed = ref(new Set<TaskSectionKey>());
const pendingIds = ref(new Set<string>());
const expandedIds = ref(new Set<string>());
let disposed = false;
onBeforeUnmount(() => { disposed = true; });

const { tasks, notices, loading, error, refresh, now, cancel, pause, resume, start, restart, remove } =
  useUnifiedTaskCenter(() => props.imageId);
const { functions } = useTaskCenter();
const catalog = usePluginCatalog();

/** Plugin analyzer names in the interface language; built-in titles by their kind. */
function taskName(entry: TaskEntry): string {
  return entry.source === 'plugin'
    ? catalog.text(entry.job.info.pluginId, pluginTextKey.analyzer(entry.job.info.analyzer), entry.job.displayName)
    : builtinTaskTitle(entry.kind, tr);
}

/** The options a plugin task runs with, by their declared labels. */
function optionSummary(entry: PluginTaskEntry): string {
  const { job } = entry;
  const declared = functions.value.find(fn => fn.function === job.name)?.options ?? [];
  return Object.entries(job.params.options).map(([id, value]) => {
    const option = declared.find(item => item.id === id);
    const label = option ? catalog.text(job.info.pluginId, pluginTextKey.option(job.info.analyzer, id), option.label) : id;
    const choice = (item: unknown) => {
      const known = option?.choices.find(candidate => candidate.id === item);
      return known ? catalog.text(job.info.pluginId, pluginTextKey.choice(job.info.analyzer, id, known.id), known.label) : String(item);
    };
    const shown = Array.isArray(value) ? (value.length ? joinList(value.map(choice), tr) : tr("全部")) : typeof value === 'boolean' ? (value ? tr("是") : tr("否")) : choice(value);
    return `${label}: ${shown}`;
  }).join('; ');
}

function detailId(entry: TaskEntry): string {
  return `task-detail-${entry.id.replace(/[^\w-]/g, '_')}`;
}
function toggleDetail(id: string) {
  if (expandedIds.value.has(id)) expandedIds.value.delete(id);
  else expandedIds.value.add(id);
}
function toggleSection(key: TaskSectionKey) {
  if (collapsed.value.has(key)) collapsed.value.delete(key);
  else collapsed.value.add(key);
}
/** Called after a quick-analysis submission: new work must be in view. */
function resetFilters() {
  search.value = '';
  collapsed.value.delete('active');
}
watch(() => props.imageId, () => { search.value = ''; showAllFinished.value = false; expandedIds.value.clear(); });
defineExpose({ resetFilters });

onMounted(() => {
  void refresh();
});

const internalCount = computed(() => tasks.value.filter(entry => entry.source === 'plugin' && entry.job.internal).length);
const scopedTasks = computed(() => tasks.value.filter(entry => showInternal.value || !(entry.source === 'plugin' && entry.job.internal)));
function searchText(entry: TaskEntry): string {
  return entry.source === 'plugin'
    ? `${taskName(entry)} ${entry.job.displayName} ${entry.job.params.fileName ?? ''} ${entry.job.params.targetLabel ?? ''}`
    : `${taskName(entry)} ${builtinSubject(entry, tr)}`;
}
const visibleTasks = computed(() => {
  const keyword = search.value.trim().toLowerCase();
  if (!keyword) return scopedTasks.value;
  return scopedTasks.value.filter(entry => searchText(entry).toLowerCase().includes(keyword));
});

const SECTION_ICON: Record<TaskSectionKey, Component> = { active: Activity, attention: TriangleAlert, finished: History };

/** `pause` / `resume` are carving controls; `start` runs a plugin task again from the beginning. */
type RowAction = 'pause' | 'resume' | 'start' | 'restart' | 'cancel' | 'open';
const ACTION_LABEL: Record<Exclude<RowAction, 'open'>, string> = { get pause() { return tr("暂停"); }, get resume() { return tr("继续"); }, get start() { return tr("开始"); }, get restart() { return tr("重新分析"); }, get cancel() { return tr("取消"); } };
/** Button text; `open` is spelled out in the tooltip and accessible name. */
const ACTION_TEXT: Record<RowAction, string> = { get pause() { return ACTION_LABEL.pause; }, get resume() { return ACTION_LABEL.resume; }, get start() { return ACTION_LABEL.start; }, get restart() { return ACTION_LABEL.restart; }, get cancel() { return ACTION_LABEL.cancel; }, get open() { return tr("查看"); } };
const ACTION_ICON: Record<RowAction, Component> = { pause: Pause, resume: Play, start: Play, restart: RotateCcw, cancel: X, open: ArrowUpRight };
function actionLabel(action: RowAction, target: TaskResultPanel | null): string {
  if (action === 'start') return tr("从头重新分析");
  return action === 'open' && target ? tr(TASK_RESULT_LABEL[target]) : ACTION_LABEL[action as Exclude<RowAction, 'open'>];
}

function offers(entry: TaskEntry, action: TaskAction): boolean {
  return !entry.cancelling && entry.actions.includes(action);
}

/** One direct control per row: run state first, then the way to the results, then recovery. */
function primaryAction(entry: TaskEntry, target: TaskResultPanel | null): RowAction | null {
  if (entry.cancelling) return null;
  if (entry.source === 'builtin') {
    if (offers(entry, 'pause')) return 'pause';
    if (offers(entry, 'resume')) return 'resume';
    if (offers(entry, 'cancel')) return 'cancel';
    return entry.status === 'success' && target ? 'open' : null;
  }
  if (offers(entry, 'start')) return 'start';
  if (entry.status === 'success' && target) return 'open';
  if (offers(entry, 'restart')) return 'restart';
  return offers(entry, 'cancel') ? 'cancel' : null;
}

/** What a plugin run published: records, timeline points, warnings and unreadable source bytes. */
function resultSummary(job: SchedTaskRecord): string {
  const result = job.result;
  if (!result) return '';
  const parts = [tr("{0} 条记录", { 0: result.recordCount.toLocaleString() })];
  if (result.timelineCount) parts.push(tr("{0} 个时间点", { 0: result.timelineCount.toLocaleString() }));
  if (result.warningCount) parts.push(tr("{0} 条告警", { 0: result.warningCount.toLocaleString() }));
  if (result.syntheticBytes) parts.push(tr("{0} 字节读不出、以零填充", { 0: result.syntheticBytes.toLocaleString() }));
  return parts.join(' · ');
}

const STATUS_HINT: Record<SchedTaskStatus, string> = {
  get created() { return tr("等待派发"); }, get queued() { return tr("等待可用执行器"); }, get running() { return tr("正在分析检材…"); },
  get success() { return tr("分析已完成"); }, get failed() { return tr("分析失败"); }, get cancel() { return tr("任务已取消"); },
  get pause() { return tr("旧版本暂停的任务，开始后从头分析"); }, get offline() { return tr("执行器离线，可从头重新分析"); },
};

/** The native task's status line: its message, or a stopping note. */
function builtinLine(entry: TaskEntry): string {
  if (entry.source !== 'builtin') return '';
  return entry.cancelling ? tr("正在取消，请稍候…") : builtinMessage(entry, tr);
}

function taskSummary(entry: TaskEntry): string {
  if (entry.error) return entry.error;
  if (entry.source === 'builtin') {
    const line = [builtinSubject(entry, tr), builtinLine(entry)].filter(Boolean).join(' · ');
    if (line) return line;
    return entry.status === 'pause' ? tr("进度已保留，可继续") : tr(TASK_STATUS_LABEL[entry.status]);
  }
  const { job } = entry;
  if (entry.status === 'success' && job.result) return resultSummary(job);
  const target = job.params.fileName ?? job.params.targetLabel;
  if (target) return target;
  if (job.result?.summary) return job.result.summary;
  return STATUS_HINT[entry.status];
}

/** Right-hand note: how long live work has run, when queued work was created, when the rest started. */
function taskMeta(entry: TaskEntry, elapsed: number | null): string {
  if (entry.status === 'running' || entry.status === 'pause') {
    const remain = entry.source === 'plugin' && entry.status === 'running' && entry.job.remainTimeSecs
      ? tr(" · 剩余 {0}", { 0: formatTaskDuration(entry.job.remainTimeSecs) }) : '';
    return tr("已用 {0}{1}", { 0: formatTaskDuration(elapsed), 1: remain });
  }
  if (entry.status === 'created' || entry.status === 'queued') return tr("{0} 创建", { 0: formatTaskClock(entry.createdAtMs) });
  return formatTaskClock(entry.startedAtMs ?? entry.createdAtMs);
}

/** Status badge; a finished success needs none, the section already says so. */
function badgeLabel(entry: TaskEntry, indeterminate: boolean, percent: number): string | null {
  if (entry.cancelling) return tr("取消中");
  if (entry.status === 'running') return indeterminate ? tr("处理中") : tr("执行中 {0}%", { 0: percent });
  if (entry.status === 'success') return null;
  return tr(TASK_STATUS_LABEL[entry.status]);
}

interface Field { label: string; value: string; mono?: boolean }

function detailFields(entry: TaskEntry, elapsed: number | null, percent: number, indeterminate: boolean): Field[] {
  const fields: Field[] = [];
  if (entry.source === 'plugin') {
    const { params } = entry.job;
    if (params.fileName) fields.push({ label: tr("文件"), value: params.fileName });
    else if (params.targetLabel) fields.push({ label: tr("系统"), value: params.targetLabel });
    const options = optionSummary(entry);
    if (options) fields.push({ label: tr("选项"), value: options });
  } else {
    const subject = builtinSubject(entry, tr);
    if (subject) fields.push({ label: tr("对象"), value: subject });
  }
  fields.push({ label: entry.startedAtMs !== null ? tr("开始") : tr("创建"), value: formatTaskDateTime(entry.startedAtMs ?? entry.createdAtMs), mono: true });
  fields.push({ label: tr("耗时"), value: formatTaskDuration(elapsed), mono: true });
  if (entry.status !== 'success' && !indeterminate && percent > 0) fields.push({ label: tr("进度"), value: `${percent}%`, mono: true });
  const result = entry.source === 'plugin' ? resultSummary(entry.job) : builtinLine(entry);
  if (result) fields.push({ label: tr("结果"), value: result });
  return fields;
}

function technicalFields(entry: TaskEntry): Field[] {
  const fields: Field[] = [];
  if (entry.source === 'plugin') {
    const { job } = entry;
    fields.push({ label: tr("任务"), value: `${job.id} · ${job.name}`, mono: true });
    fields.push({ label: tr("检材 ID"), value: job.info.imageId, mono: true });
    if (job.params.systemId) fields.push({ label: tr("系统 ID"), value: job.params.systemId, mono: true });
    if (job.result?.runId) fields.push({ label: tr("运行 ID"), value: job.result.runId, mono: true });
    fields.push({ label: tr("触发"), value: entry.automatic ? tr("自动") : tr("手动") });
    if (job.dependence) fields.push({ label: tr("依赖"), value: job.dependence, mono: true });
    if (job.info.parentTid !== null) fields.push({ label: tr("父任务"), value: `#${job.info.parentTid}`, mono: true });
    return fields;
  }
  fields.push({ label: tr("任务"), value: `${entry.id} · ${entry.kind}`, mono: true });
  if (entry.imageId) fields.push({ label: tr("检材 ID"), value: entry.imageId, mono: true });
  fields.push({ label: tr("触发"), value: entry.automatic ? tr("自动") : tr("手动") });
  return fields;
}

/** Everything a row shows, computed once per render. */
function rowView(entry: TaskEntry) {
  const target = taskEntryResultPanel(entry);
  const indeterminate = taskEntryIndeterminate(entry);
  const percent = taskEntryPercent(entry);
  const elapsed = taskEntryElapsedSecs(entry, now.value);
  return {
    entry,
    name: taskName(entry),
    summary: taskSummary(entry),
    meta: taskMeta(entry, elapsed),
    badge: badgeLabel(entry, indeterminate, percent),
    showsProgress: entry.status === 'running' || entry.status === 'pause',
    indeterminate,
    percent,
    target,
    action: primaryAction(entry, target),
    removable: entry.source === 'plugin' && offers(entry, 'remove'),
    details: detailFields(entry, elapsed, percent, indeterminate),
    technical: technicalFields(entry),
  };
}

const sections = computed(() => groupTaskSections(visibleTasks.value).map(section => {
  const limit = section.key === 'finished' && !showAllFinished.value && !search.value.trim() ? FINISHED_PREVIEW : section.tasks.length;
  return {
    ...section,
    hidden: Math.max(0, section.tasks.length - limit),
    rows: section.tasks.slice(0, limit).map(rowView),
  };
}));

function openResult(entry: TaskEntry) {
  const target = taskEntryResultPanel(entry);
  if (target) emit('openResult', target, entry.source === 'plugin' ? entry.job.result?.runId ?? entry.job.name : null);
}

async function runTaskAction(entry: TaskEntry, action: RowAction | 'remove') {
  if (action === 'open') {
    openResult(entry);
    return;
  }
  const id = entry.id;
  if (pendingIds.value.has(id)) return;
  pendingIds.value.add(id);
  try {
    if (action === 'remove' || action === 'restart') {
      const ok = await confirm({ title: action === 'remove' ? tr("删除任务") : tr("重新分析"),
        message: action === 'remove' ? tr("将删除该任务及其分析结果，是否继续？") : tr("重新分析成功后，新结果会替换之前的结果，是否继续？"),
        confirmText: action === 'remove' ? tr("删除") : tr("重新分析"), danger: true });
      if (!ok || disposed) return;
    }
    await ({ cancel, pause, resume, start, restart, remove })[action](id);
  } finally { pendingIds.value.delete(id); }
}

/** 调度通知的展示标签。 */
const NOTICE_LABEL: Record<SchedNoticeKind, string> = {
  get taskOffline() { return tr("任务离线"); },
  get taskTimeout() { return tr("任务超时"); },
  get allTasksFinished() { return tr("全部完成"); },
  get postActionFailed() { return tr("收尾失败"); },
};
</script>

<style scoped>
.task-center { flex: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; color: var(--text-primary); }
.task-toolbar { display: flex; align-items: center; gap: 6px; flex-shrink: 0; padding: 10px 12px; border-bottom: 1px solid var(--border-color); background: var(--surface-secondary); }
.task-search { flex: 1; }
.task-scroll { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; gap: 12px; padding: 12px 8px 14px 12px; background: var(--surface-primary); scrollbar-gutter: stable; }

/* Section card: the image-info card head (icon title, count, trailing control). */
.task-card { flex-shrink: 0; overflow: hidden; }
.card-head { display: flex; align-items: center; gap: 8px; min-height: 44px; padding: 10px 12px 0 16px; }
.card-toggle { margin-left: auto; }
.card-toggle:where([aria-expanded]) { background: transparent; color: var(--text-secondary); }
.card-toggle:hover:not(:disabled) { background: var(--hover-bg); color: var(--text-primary); }
.card-toggle svg { transition: transform var(--transition-fast); }
.card-toggle[aria-expanded='false'] svg { transform: rotate(-90deg); }

/* Rows: hairlines between them, text on the card head's 16 px gutter. */
.task-list { display: flex; flex-direction: column; padding: 6px 0 8px; }
.task-row { border-top: 1px solid color-mix(in srgb, var(--border-color) 70%, transparent); transition: background-color var(--transition-fast); }
.task-row:first-child { border-top: 0; }
.task-row:hover, .task-row.expanded { background: color-mix(in srgb, var(--surface-tertiary) 60%, transparent); }
.task-main { display: flex; align-items: center; gap: 10px; padding: 10px 12px 10px 16px; }
.task-open { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; padding: 0; border: 0; border-radius: var(--radius-sm); background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
.task-open:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 3px; }
.task-line { display: flex; align-items: center; gap: 8px; min-width: 0; }
.task-name { min-width: 0; overflow: hidden; font-size: 13px; font-weight: 600; line-height: 20px; text-overflow: ellipsis; white-space: nowrap; }
.task-badge { --badge-tone: var(--tone); font-family: var(--font-mono); font-variant-numeric: tabular-nums; }
.task-sub { gap: 12px; color: var(--text-secondary); font-size: 12px; line-height: 18px; }
.task-summary { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.attention .task-summary { color: var(--sev-critical-fg); }
.task-meta { flex-shrink: 0; font-size: 11px; font-variant-numeric: tabular-nums; }
.task-progress { margin-top: 5px; }
.task-progress > span { background: var(--tone); }
.task-action { min-width: 64px; }

/* Expanded details: field rows like the acquisition record, actions on the right. */
.task-detail { padding: 0 12px 12px 16px;color: var(--text-secondary); font-size: 12px; overflow-wrap: anywhere; }
.detail-error { margin-bottom: 6px; white-space: pre-wrap; }
.detail-list { margin: 0; }
.detail-row { display: grid; grid-template-columns: 56px minmax(0, 1fr); align-items: baseline; gap: 12px; padding: 7px 0; border-bottom: 1px solid color-mix(in srgb, var(--border-color) 70%, transparent); }
.detail-row:last-child { border-bottom: 0; }
.detail-row dt { color: var(--text-secondary); font-size: 11px; }
.detail-row dd { min-width: 0; margin: 0; color: var(--text-primary); font-size: 12px; line-height: 1.6; }
.detail-row dd.mono { font-size: 11px; }
.detail-actions { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 6px; margin-top: 8px; }
.detail-actions:empty { display: none; }
.detail-remove { margin-right: auto; }

.card-disclosure > summary { display: flex; align-items: center; gap: 6px; padding: 10px 0 2px; color: var(--text-secondary); font-size: 12px; list-style: none; cursor: pointer; }
.card-disclosure > summary::-webkit-details-marker { display: none; }
.card-disclosure > summary:hover { color: var(--primary-color); }
.card-disclosure > summary svg { transition: transform var(--transition-fast); }
.card-disclosure:not([open]) > summary svg { transform: rotate(-90deg); }
.card-disclosure > summary:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 3px; }
.technical-details .detail-list { margin-top: 4px; }

.card-more { display: flex; align-items: center; gap: 6px; width: calc(100% - 32px); margin: 2px 16px 0; padding: 10px 0 2px; border: 0; border-top: 1px solid color-mix(in srgb, var(--border-color) 70%, transparent); background: transparent; color: var(--text-secondary); font: 12px var(--font-ui); cursor: pointer; }
.card-more:hover { color: var(--primary-color); }
.card-more svg { transform: rotate(-90deg); }
.card-more:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }

.notice-list { padding: 4px 16px 10px; }

.state-box { min-height: 220px; padding: 40px 20px; overflow-wrap: anywhere; }
.state-box.error .ui-empty-icon { background: var(--sev-critical-bg); color: var(--sev-critical-fg); }
@media (max-width: 400px) { .task-meta { display: none; } .task-action { min-width: 0; } }
</style>
