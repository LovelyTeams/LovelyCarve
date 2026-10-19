<template>
  <section class="quick-analysis" :aria-label="tr('快速分析')">
    <div class="qa-head">
      <div class="qa-toolbar">
        <UiSelect v-if="systems.length > 1" class="qa-system" :model-value="activeSystemId ?? ''" :options="systemOptions" :disabled="submitting" :aria-label="tr('分析的系统')" @update:model-value="onPickSystem" />
        <span v-else-if="systems.length" class="qa-system-label" :title="systems[0].label"><component :is="familyIcon(systems[0].family)" :size="13" /><span>{{ systems[0].label }}</span></span>
        <span v-else-if="systemsLoading" class="qa-system-label" role="status"><Loader2 :size="13" class="spin" /><span>{{ tr("正在识别系统…") }}</span></span>
        <button v-else-if="systemsError" class="ui-btn sm ghost qa-system-retry" type="button" :title="tr('识别系统失败：{0}', { 0: systemsError })" @click="loadSystems"><RotateCcw :size="13" />{{ tr("重新识别系统") }}</button>
        <span v-else class="qa-system-label" :title="tr('只列出整个检材的分析')"><MonitorX :size="13" /><span>{{ tr("未识别出操作系统") }}</span></span>
        <label class="ui-search qa-search"><Search :size="14" /><input v-model="search" type="text" :placeholder="tr('搜索插件或分析内容')" :aria-label="tr('搜索插件或分析内容')" /><button v-if="search" class="ui-icon-btn sm" :aria-label="tr('清除搜索')" @click="search = ''"><X :size="13" /></button></label>
      </div>
      <!-- One box for everything listed (the matches while searching); each category has its own below. -->
      <div v-if="groups.length" class="qa-selectbar">
        <label class="ui-check qa-check-all">
          <input
            type="checkbox"
            :checked="checkState(visiblePlugins) === 'all'"
            :indeterminate="checkState(visiblePlugins) === 'some'"
            :disabled="submitting || !selectable(visiblePlugins).length"
            @change="toggleMany(visiblePlugins)"
          />{{ searching ? tr("全选匹配的插件") : tr("全选") }}
        </label>
        <span class="qa-selectbar-count">{{ visiblePlugins.length }} {{ tr("个插件") }}</span>
        <button class="ui-btn sm ghost qa-expand-all" type="button" @click="toggleExpandAll">
          <component :is="allExpanded ? ChevronsDownUp : ChevronsUpDown" :size="13" />{{ allExpanded ? tr("全部收起") : tr("全部展开") }}
        </button>
      </div>
    </div>

    <div class="qa-scroll">
      <div v-if="loading && !functions.length" class="ui-empty state-box"><span class="ui-empty-icon accent"><Loader2 :size="22" class="spin" /></span><p class="ui-empty-desc">{{ tr("正在加载分析插件…") }}</p></div>
      <div v-else-if="error && !functions.length" class="ui-empty state-box error"><span class="ui-empty-icon"><CircleAlert :size="22" /></span><p class="ui-empty-title">{{ tr("读取分析插件失败") }}</p><p class="ui-empty-desc">{{ error }}</p><div class="ui-empty-actions"><button class="ui-btn sm" @click="refresh">{{ tr("重试") }}</button></div></div>
      <div v-else-if="!availablePlugins.length" class="ui-empty state-box"><span class="ui-empty-icon"><Zap :size="22" /></span><p class="ui-empty-title">{{ tr("暂无可用的分析插件") }}</p><p class="ui-empty-desc">{{ systems.length ? tr("所选系统与整个检材都没有适用的分析插件。") : tr("安装提供系统或检材分析的插件后，会显示在这里。") }}</p></div>
      <div v-else-if="!groups.length" class="ui-empty state-box"><span class="ui-empty-icon"><Search :size="22" /></span><p class="ui-empty-title">{{ tr("没有匹配的插件") }}</p><div class="ui-empty-actions"><button class="ui-btn sm" @click="search = ''">{{ tr("清除搜索") }}</button></div></div>
      <template v-else>
        <section v-for="group in groups" :key="group.key" class="qa-group" :aria-label="tr('{0} {1} 个插件', { 0: group.label, 1: group.plugins.length })">
          <div class="qa-group-head">
            <input
              type="checkbox"
              class="qa-group-check"
              :checked="checkState(group.plugins) === 'all'"
              :indeterminate="checkState(group.plugins) === 'some'"
              :disabled="submitting || !selectable(group.plugins).length"
              :aria-label="tr('全选{0}', { 0: group.label })"
              @change="toggleMany(group.plugins)"
            />
            <button
              class="qa-group-toggle"
              type="button"
              :aria-expanded="isExpanded(group.key)"
              :aria-controls="isExpanded(group.key) ? `qa-group-${group.key}` : undefined"
              @click="toggleExpanded(group.key)"
            >
              <component :is="groupIcon(group.key)" :size="14" class="qa-group-icon" />
              <span class="qa-group-label">{{ group.label }}</span>
              <span class="qa-group-count">{{ countText(group.plugins) }}</span>
              <ChevronDown :size="14" class="qa-group-chevron" />
            </button>
          </div>
          <UiTransition>
            <!-- Plugins as tiles, several per line; what each runs is in its tooltip. -->
            <ul v-if="isExpanded(group.key)" :id="`qa-group-${group.key}`" class="qa-grid">
              <li v-for="plugin in group.plugins" :key="plugin.id" class="qa-tile" :class="{ selected: selection.isSelected(plugin.id), busy: stateOf(plugin).busy, 'has-status': stateOf(plugin).badge }">
                <label class="qa-tile-choice" :title="tileTitle(plugin)">
                  <input type="checkbox" :checked="selection.isSelected(plugin.id)" :disabled="submitting || stateOf(plugin).busy" @change="selection.toggle(plugin.id)" />
                  <span class="qa-tile-name">{{ plugin.name }}</span>
                </label>
                <button
                  class="ui-icon-btn sm qa-star"
                  :class="{ saved: isFavouritePlugin(plugin.id) }"
                  type="button"
                  :disabled="submitting"
                  :title="isFavouritePlugin(plugin.id) ? tr('取消收藏') : tr('收藏')"
                  :aria-label="`${isFavouritePlugin(plugin.id) ? tr('取消收藏') : tr('收藏')} ${plugin.name}`"
                  :aria-pressed="isFavouritePlugin(plugin.id)"
                  @click="toggleFavouritePlugin(plugin.id)"
                ><Star :size="12" /></button>
                <span v-if="stateOf(plugin).badge" class="qa-tile-status" :style="{ color: stateOf(plugin).badge!.tone }" :title="tileTitle(plugin)">
                  <component :is="STATUS_ICON[stateOf(plugin).badge!.status]" :size="13" :class="{ spin: stateOf(plugin).badge!.status === 'running' }" aria-hidden="true" />
                  <span class="sr-only">{{ stateOf(plugin).badge!.text }}</span>
                </span>
              </li>
            </ul>
          </UiTransition>
        </section>
      </template>
    </div>

    <footer class="qa-foot">
      <div v-if="evidenceTasks.length" ref="moreMenuRoot" class="qa-more" @keydown.esc.stop="moreOpen = false">
        <button class="ui-icon-btn" :aria-expanded="moreOpen" aria-haspopup="menu" :aria-label="tr('更多操作')" :title="tr('更多操作')" @click="moreOpen = !moreOpen"><MoreHorizontal :size="16" /></button>
        <div v-if="moreOpen" class="ui-menu qa-more-menu" role="menu"><button class="ui-menu-item" role="menuitem" :disabled="submitting" @click="onRestartAll"><RotateCcw :size="14" />{{ tr("重新分析全部任务") }}</button></div>
      </div>
      <span class="qa-summary">
        <span v-if="selectedCount">{{ tr("已选") }} <strong :key="selectedCount" class="ui-bump">{{ selectedCount }}</strong> {{ tr("个插件") }}</span>
        <span v-else>{{ tr("勾选要运行的插件") }}</span>
        <span v-if="rerunCount" class="qa-rerun">{{ tr("其中 {0} 个已有结果，重新分析成功后替换", { 0: rerunCount }) }}</span>
      </span>
      <button v-if="selectedCount" class="ui-btn ghost" :disabled="submitting" @click="selection.clear()">{{ tr("清空") }}</button>
      <button class="ui-btn primary" :disabled="submitting || !selectedCount" @click="onConfirm"><Loader2 v-if="submitting" :size="13" class="spin" /><Play v-else :size="13" />{{ submitting ? tr("提交中…") : tr("开始分析") }}</button>
    </footer>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';
import { joinList } from '../../../i18n/translate';

/** 系统与检材级分析插件清单：按分类平铺插件，勾选插件即以默认选项运行它在所选系统（与整个检材）上的全部分析；
 *  顶部与每个分类各有一个三态全选框，格子右侧的图标是该插件在当前目标上的最近任务状态。 */
import UiSelect from '../../../components/ui/UiSelect.vue';
import UiTransition from '../../../components/ui/UiTransition.vue';
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue';
import {
  Apple, AppWindow, Bot, ChartColumn, ChevronDown, ChevronsDownUp, ChevronsUpDown, CircleAlert, CircleCheck, CirclePause,
  CircleSlash, CircleX, Clock, Database, FileSearch, FolderTree, Globe, Hourglass, Info, Laptop, Loader2, MonitorCog,
  MonitorX, MoreHorizontal, Play, Router, RotateCcw, ScrollText, Search, Shapes, Star, Terminal, Unplug, Workflow, X, Zap,
  type LucideIcon,
} from 'lucide-vue-next';

import { confirm } from '../../../composables/useConfirm';
import {
  buildPluginGroups, favouriteNames, groupByPlugin, isFavouritePlugin, preprocessFunctions,
  toggleFavouritePlugin, useQuickAnalysisSelection, useSystemMemory,
  type QuickPlugin,
} from '../composables/useQuickAnalysis';
import { useTaskCenter } from '../composables/useTaskCenter';
import { usePluginCatalog } from '../composables/usePluginCatalog';
import { TASK_STATUS_LABEL, isTerminalStatus } from '../model/taskStatus';
import { notifyInfo } from '../../../composables/useToast';
import { TASK_STATUS_TONE, taskProgressPercent } from '../model/taskCenterView';
import {
  schedSystemsList,
  type DetectedSystem, type SchedFunction, type SchedTaskRecord, type SchedTaskStatus,
} from '../../../lib/ipc/tasks';

const props = defineProps<{ imageId: string }>();

const emit = defineEmits<{ submitted: [] }>();

const { functions, tasks, loading, error, refresh, create, restart, start } = useTaskCenter();
const catalog = usePluginCatalog();
const selection = useQuickAnalysisSelection();
const systemMemory = useSystemMemory(props.imageId);

const systems = shallowRef<DetectedSystem[]>([]);
const systemsLoading = ref(false);
const systemsError = ref<string | null>(null);
const activeSystemId = ref<string | null>(null);
const search = ref('');
const submitting = ref(false);
const moreOpen = ref(false);
const moreMenuRoot = ref<HTMLElement | null>(null);
/** Categories the examiner folded; a new search unfolds everything again. */
const collapsed = ref<Set<string>>(new Set());
let disposed = false;

function onOutsidePointer(event: PointerEvent) {
  if (event.target instanceof Node && moreMenuRoot.value?.contains(event.target)) return;
  moreOpen.value = false;
}
watch(moreOpen, open => {
  if (open) document.addEventListener('pointerdown', onOutsidePointer);
  else document.removeEventListener('pointerdown', onOutsidePointer);
});
onMounted(() => {
  void refresh();
  void loadSystems();
});
onBeforeUnmount(() => {
  disposed = true;
  document.removeEventListener('pointerdown', onOutsidePointer);
});

/** The systems detected on this evidence; the remembered one, else the first, is preselected. */
async function loadSystems(): Promise<void> {
  if (systemsLoading.value) return;
  systemsLoading.value = true;
  systemsError.value = null;
  try {
    const detected = await schedSystemsList(props.imageId);
    if (disposed) return;
    systems.value = detected;
    const remembered = systemMemory.value;
    activeSystemId.value = detected.find(system => system.id === remembered)?.id ?? detected[0]?.id ?? null;
  } catch (cause) {
    if (!disposed) systemsError.value = String(cause);
  } finally {
    if (!disposed) systemsLoading.value = false;
  }
}

const activeSystem = computed(() => systems.value.find(system => system.id === activeSystemId.value) ?? null);
const systemOptions = computed(() => systems.value.map(system => ({ value: system.id, label: system.label })));
watch(activeSystemId, (id) => {
  if (id && systemMemory.value !== id) systemMemory.value = id;
});

const availableFunctions = computed(() => preprocessFunctions(functions.value, activeSystem.value));

/** 所选系统与整个检材上可运行的全部插件(不受搜索影响)。 */
const availablePlugins = computed(() => groupByPlugin(availableFunctions.value, catalog.text));
const pluginById = computed(() => new Map(availablePlugins.value.map(plugin => [plugin.id, plugin])));
const groups = computed(() => buildPluginGroups(availablePlugins.value, { favourites: favouriteNames.value, search: search.value, text: catalog.text }));
const searching = computed(() => search.value.trim() !== '');

/** 当前列出的插件(搜索时为匹配项);收藏分类里的重复项只算一次。 */
const visiblePlugins = computed(() => {
  const listed = new Map<string, QuickPlugin>();
  for (const group of groups.value) {
    for (const plugin of group.plugins) listed.set(plugin.id, plugin);
  }
  return [...listed.values()];
});

// A selection made for another system does not carry over.
watch(availablePlugins, (plugins) => {
  const available = new Set(plugins.map(plugin => plugin.id));
  const gone = selection.selectionList.value.filter(id => !available.has(id));
  if (gone.length) selection.deselectMany(gone);
});

/** A job is a function on one target: the selected system, or the whole evidence. */
function jobKey(fn: SchedFunction): string {
  return `${fn.function}#${fn.scope === 'system' ? activeSystemId.value ?? '' : ''}`;
}

/** 本检材任务(每个作业最新一条,状态画回插件格子)。 */
const latestTaskByJob = computed(() => {
  const latest = new Map<string, SchedTaskRecord>();
  for (const task of tasks.value) {
    if (task.info.imageId !== props.imageId) continue;
    const key = `${task.name}#${task.params.systemId ?? ''}`;
    const current = latest.get(key);
    if (!current || task.id > current.id) latest.set(key, task);
  }
  return latest;
});

const evidenceTasks = computed(() => tasks.value.filter(task => task.info.imageId === props.imageId));

interface PluginBadge {
  /** The status that stands for the plugin: running, then waiting, then failed, cancelled, success. */
  status: SchedTaskStatus;
  text: string;
  tone: string;
}

interface PluginState {
  /** A job's latest task has not ended: the scheduler keeps one unfinished
   *  task per job, so the plugin cannot be submitted again. */
  busy: boolean;
  /** Some job already published results that a new run replaces. */
  published: boolean;
  /** One line per function that has a task, for the tooltip. */
  taskLines: string[];
  badge: PluginBadge | null;
}

const IDLE_STATE: PluginState = { busy: false, published: false, taskLines: [], badge: null };

const STATUS_ICON: Record<SchedTaskStatus, LucideIcon> = {
  created: Clock, queued: Clock, running: Loader2, pause: CirclePause, offline: Unplug,
  success: CircleCheck, failed: CircleX, cancel: CircleSlash,
};

function statusText(task: SchedTaskRecord): string {
  if (task.status === 'running') return tr("执行中 {0}%", { 0: taskProgressPercent(task) });
  if (task.status === 'success') return tr("已完成");
  return tr(TASK_STATUS_LABEL[task.status]);
}

function badgeOf(plugin: QuickPlugin, latest: SchedTaskRecord[]): PluginBadge | null {
  if (!latest.length) return null;
  const running = latest.filter(task => task.status === 'running');
  if (running.length) {
    const percent = Math.round(running.reduce((sum, task) => sum + taskProgressPercent(task), 0) / running.length);
    return { status: 'running', text: tr("执行中 {0}%", { 0: percent }), tone: TASK_STATUS_TONE.running };
  }
  const waiting = latest.find(task => !isTerminalStatus(task.status));
  if (waiting) return { status: waiting.status, text: tr(TASK_STATUS_LABEL[waiting.status]), tone: TASK_STATUS_TONE[waiting.status] };
  if (latest.some(task => task.status === 'failed')) return { status: 'failed', text: tr("失败"), tone: TASK_STATUS_TONE.failed };
  const succeeded = latest.filter(task => task.status === 'success').length;
  if (!succeeded) return { status: 'cancel', text: tr(TASK_STATUS_LABEL.cancel), tone: TASK_STATUS_TONE.cancel };
  const total = plugin.functions.length;
  return { status: 'success', text: succeeded === total ? tr("已完成") : tr("已完成 {0}/{1}", { 0: succeeded, 1: total }), tone: TASK_STATUS_TONE.success };
}

const pluginStates = computed(() => {
  const states = new Map<string, PluginState>();
  for (const plugin of availablePlugins.value) {
    const entries = plugin.functions
      .map(fn => ({ fn, task: latestTaskByJob.value.get(jobKey(fn)) }))
      .filter((entry): entry is { fn: SchedFunction; task: SchedTaskRecord } => entry.task !== undefined);
    const latest = entries.map(entry => entry.task);
    states.set(plugin.id, {
      busy: latest.some(task => !isTerminalStatus(task.status)),
      published: latest.some(task => task.status === 'success'),
      taskLines: entries.map(({ fn, task }) => {
        const detail = task.info.error ?? task.result?.summary;
        return `#${task.id} ${fn.displayName} · ${statusText(task)}${detail ? ` · ${detail}` : ''}`;
      }),
      badge: badgeOf(plugin, latest),
    });
  }
  return states;
});

function stateOf(plugin: QuickPlugin): PluginState {
  return pluginStates.value.get(plugin.id) ?? IDLE_STATE;
}

function tileTitle(plugin: QuickPlugin): string {
  const state = stateOf(plugin);
  return [
    plugin.name,
    tr("包含：{0}", { 0: joinList(plugin.functions.map(fn => fn.displayName), tr) }),
    state.busy ? tr("已有未结束的任务，可在任务列表中查看或取消") : null,
    state.badge ? tr("最近任务：{0}", { 0: state.badge.text }) : null,
    ...state.taskLines,
  ].filter(Boolean).join('\n');
}

// A task started elsewhere (another window, a dependency) locks a plugin that is still ticked.
watch(pluginStates, (states) => {
  const busy = selection.selectionList.value.filter(id => states.get(id)?.busy);
  if (busy.length) selection.deselectMany(busy);
});

const selectedPlugins = computed(() =>
  selection.selectionList.value
    .map(id => pluginById.value.get(id))
    .filter((plugin): plugin is QuickPlugin => plugin !== undefined),
);
const selectedCount = computed(() => selectedPlugins.value.length);
const rerunCount = computed(() => selectedPlugins.value.filter(plugin => stateOf(plugin).published).length);

type CheckState = 'all' | 'some' | 'none';

function selectable(plugins: QuickPlugin[]): QuickPlugin[] {
  return plugins.filter(plugin => !stateOf(plugin).busy);
}
function checkState(plugins: QuickPlugin[]): CheckState {
  const free = selectable(plugins);
  const picked = free.filter(plugin => selection.isSelected(plugin.id)).length;
  if (!picked) return 'none';
  return picked === free.length ? 'all' : 'some';
}
function toggleMany(plugins: QuickPlugin[]): void {
  const ids = selectable(plugins).map(plugin => plugin.id);
  if (checkState(plugins) === 'all') selection.deselectMany(ids);
  else selection.selectMany(ids);
}
function countText(plugins: QuickPlugin[]): string {
  const picked = plugins.filter(plugin => selection.isSelected(plugin.id)).length;
  return picked ? `${picked}/${plugins.length}` : String(plugins.length);
}

watch(search, () => { collapsed.value = new Set(); });
function isExpanded(key: string): boolean {
  return !collapsed.value.has(key);
}
function toggleExpanded(key: string): void {
  const next = new Set(collapsed.value);
  if (!next.delete(key)) next.add(key);
  collapsed.value = next;
}
const allExpanded = computed(() => groups.value.every(group => isExpanded(group.key)));
function toggleExpandAll(): void {
  collapsed.value = allExpanded.value ? new Set(groups.value.map(group => group.key)) : new Set();
}

/** Category icons; unknown plugin categories share a neutral one. */
const GROUP_ICON: Record<string, LucideIcon> = {
  favour: Star, baseinfo: Info, system: MonitorCog, application: AppWindow, browserinfo: Globe, loginfo: ScrollText,
  databaseinfo: Database, fileanalyzer: FileSearch, filesystem: FolderTree, analysis: ChartColumn, longtime: Hourglass,
};
function groupIcon(key: string): LucideIcon {
  return GROUP_ICON[key] ?? Shapes;
}

function onPickSystem(systemId: string): void {
  activeSystemId.value = systemId;
}

function familyIcon(family: string): LucideIcon {
  const icons: Record<string, LucideIcon> = {
    android: Bot, harmony: Workflow, ios: Apple, windows: AppWindow, macos: Laptop, linux: Terminal,
    openwrt: Router, mikrotik: Router, esxi: MonitorCog,
  };
  return icons[family] ?? Workflow;
}

/** 确定:已有结果时确认替换,再为每个插件的每个功能建任务(默认选项)。 */
async function onConfirm(): Promise<void> {
  const chosen = selectedPlugins.value;
  if (!chosen.length || submitting.value) return;
  submitting.value = true;
  const targetImageId = props.imageId;
  const targetSystemId = activeSystemId.value;
  try {
    const rerun = chosen.filter(plugin => stateOf(plugin).published);
    if (rerun.length > 0) {
      const ok = await confirm({
        title: tr("已有分析结果"),
        message: tr("{0} 已有分析结果。重新分析成功后，新结果会替换旧结果。是否继续？", { 0: joinList(rerun.map(plugin => plugin.name), tr) }),
        confirmText: tr("重新分析"),
      });
      if (!ok || disposed) return;
    }
    let createdTasks = 0;
    let completePlugins = 0;
    for (const plugin of chosen) {
      let complete = true;
      for (const fn of plugin.functions) {
        if (disposed) return;
        const systemId = fn.scope === 'system' ? targetSystemId : null;
        const task = await create({ function: fn.function, imageId: targetImageId, systemId }, { quiet: true });
        if (task) createdTasks += 1;
        else complete = false;
      }
      if (complete) {
        completePlugins += 1;
        selection.deselectMany([plugin.id]);
      }
    }
    if (disposed || !createdTasks) return;
    notifyInfo(tr("快速分析"), tr("已提交 {0} 个分析任务，可在任务列表查看进度", { 0: createdTasks }));
    if (completePlugins === chosen.length) emit('submitted');
  } finally {
    submitting.value = false;
  }
}

/** 重新分析该检材的全部任务;新结果成功发布后替换旧结果。 */
async function onRestartAll(): Promise<void> {
  moreOpen.value = false;
  const candidates = evidenceTasks.value;
  if (!candidates.length || submitting.value) return;
  const ok = await confirm({
    title: tr("重新分析全部任务"),
    message: tr("将重新运行该检材的 {0} 个任务，成功后新结果替换之前的结果。是否继续？", { 0: candidates.length }),
    confirmText: tr("重新分析"),
  });
  if (!ok || disposed) return;
  submitting.value = true;
  try {
    let restartedCount = 0;
    for (const task of candidates) {
      if (disposed) return;
      if (task.status === 'pause' || task.status === 'offline') {
        if (await start(task.id)) restartedCount += 1;
      } else if (isTerminalStatus(task.status) && await restart(task.id)) {
        restartedCount += 1;
      }
    }
    if (disposed || !restartedCount) return;
    notifyInfo(tr("快速分析"), tr("已重新排队 {0} 个任务", { 0: restartedCount }));
    emit('submitted');
  } finally {
    submitting.value = false;
  }
}
</script>

<style scoped>
.quick-analysis { flex: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; color: var(--text-primary); }
.qa-head { flex-shrink: 0; border-bottom: 1px solid var(--border-color); background: var(--surface-secondary); }
.qa-toolbar { display: flex; align-items: center; gap: 8px; padding: 10px 12px; }
.qa-system { flex-shrink: 1; min-width: 0; max-width: 220px; }
.qa-system-label > svg { flex-shrink: 0; }
.qa-system-label > span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.qa-system-retry { flex-shrink: 0; }
.qa-system-label { display: inline-flex; align-items: center; gap: 6px; flex-shrink: 1; min-width: 0; max-width: 220px; height: 30px; padding: 0 10px; border-radius: var(--radius-md); background: var(--surface-tertiary); color: var(--text-secondary); font-size: 12px; font-weight: 500; }
.qa-search { flex: 1; }

/* Select-all line: its box sits in the same column as the category boxes below. */
.qa-selectbar { display: flex; align-items: center; gap: 10px; min-height: 32px; padding: 0 8px 6px 14px; }
.qa-check-all { font-weight: 600; }
.qa-selectbar-count { color: var(--text-secondary); font-size: 11px; }
.qa-expand-all { margin-left: auto; }

.qa-scroll { flex: 1; min-height: 0; overflow-y: auto; padding-bottom: 8px; background: var(--surface-secondary); scrollbar-gutter: stable; }

/* Category: a header line that stays on top while its tiles scroll past. */
.qa-group-head { position: sticky; top: 0; z-index: 1; display: flex; align-items: center; gap: 10px; min-height: 38px; padding: 0 8px 0 14px; background: var(--surface-secondary); }
.qa-group + .qa-group .qa-group-head { border-top: 1px solid color-mix(in srgb, var(--border-color) 70%, transparent); }
.qa-group-check { flex-shrink: 0; width: 14px; height: 14px; margin: 0; cursor: pointer; }
.qa-group-check:disabled { cursor: not-allowed; }
.qa-group-toggle { display: flex; flex: 1; align-items: center; gap: 8px; min-width: 0; height: 38px; padding: 0 4px; border: 0; border-radius: var(--radius-sm); background: transparent; color: var(--text-primary); font: 600 13px var(--font-ui); text-align: left; cursor: pointer; }
.qa-group-toggle:hover .qa-group-label { color: var(--primary-color); }
.qa-group-toggle:focus-visible { outline: 2px solid var(--primary-color); outline-offset: -2px; }
.qa-group-icon { flex-shrink: 0; color: var(--text-secondary); }
.qa-group-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; transition: color var(--transition-fast); }
.qa-group-count { margin-left: auto; color: var(--text-secondary); font: 500 11px var(--font-mono); }
.qa-group-chevron { flex-shrink: 0; color: var(--text-secondary); transition: transform var(--transition-fast); }
.qa-group-toggle[aria-expanded='false'] .qa-group-chevron { transform: rotate(-90deg); }

/* Plugin tiles: two or more per line, the whole tile toggles its plugin. */
.qa-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(176px, 1fr)); gap: 6px; margin: 0; padding: 2px 12px 10px 14px; list-style: none; }
.qa-tile { position: relative; display: flex; align-items: center; min-width: 0; min-height: 32px; border: 1px solid var(--border-color); border-radius: var(--radius-md); background: var(--surface-secondary); transition: background-color var(--transition-fast), border-color var(--transition-fast); }
.qa-tile:hover { border-color: color-mix(in srgb, var(--primary-color) 45%, var(--border-color)); }
.qa-tile.selected { border-color: color-mix(in srgb, var(--primary-color) 55%, var(--border-color)); background: var(--sidebar-active-bg); }
.qa-tile-choice { display: flex; flex: 1; align-items: center; align-self: stretch; gap: 8px; min-width: 0; padding: 0 4px 0 10px; cursor: pointer; }
.qa-tile-choice input { flex-shrink: 0; width: 14px; height: 14px; margin: 0; cursor: pointer; }
.qa-tile-name { flex: 1; min-width: 0; overflow: hidden; font-size: 12px; font-weight: 500; text-overflow: ellipsis; white-space: nowrap; }
.selected .qa-tile-name { color: var(--primary-color); }
.busy .qa-tile-choice, .busy .qa-tile-choice input { cursor: default; }
.busy .qa-tile-name { color: var(--text-secondary); }
.qa-tile-status { display: inline-flex; flex-shrink: 0; margin-right: 8px; }
/* The star floats over the end of the name until it is saved, so names keep the room. */
.qa-star { --ui-icon-btn-size: 22px; position: absolute; top: 50%; right: 4px; translate: 0 -50%; background: inherit; opacity: 0; transition: opacity var(--transition-fast), color var(--transition-fast); }
.qa-tile.has-status .qa-star { right: 26px; }
.qa-star:hover:not(:disabled) { color: var(--sev-warn-fg); }
.qa-tile:hover .qa-star, .qa-tile:focus-within .qa-star { opacity: 1; }
.qa-star.saved { position: static; flex-shrink: 0; margin-right: 2px; translate: none; opacity: 1; color: var(--sev-warn-fg); }
.qa-star.saved svg { fill: currentColor; }
@media (hover: none) { .qa-star { position: static; flex-shrink: 0; translate: none; opacity: 1; } }

.qa-foot { display: flex; align-items: center; gap: 8px; flex-shrink: 0; min-height: 54px; padding: 10px 12px; border-top: 1px solid var(--border-color); background: var(--surface-secondary); }
.qa-more { position: relative; flex-shrink: 0; }
.qa-more-menu { --ui-pop-offset: 4px; position: absolute; bottom: calc(100% + 6px); left: 0; z-index: 5; white-space: nowrap; }
.qa-summary { display: flex; flex: 1; flex-direction: column; gap: 1px; min-width: 0; color: var(--text-secondary); font-size: 12px; }
.qa-summary strong { display: inline-block; color: var(--text-primary); font-family: var(--font-mono); font-weight: 600; }
.qa-rerun { overflow: hidden; color: var(--sev-warn-fg); font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }

.state-box { min-height: 220px; padding: 40px 20px; overflow-wrap: anywhere; }
.state-box.error .ui-empty-icon { background: var(--sev-critical-bg); color: var(--sev-critical-fg); }
</style>
