
import { tr } from '../../../composables/useLocale';
// 快速分析的数据组装:适用于选中系统的系统级功能与检材级功能,按插件归并、
// 分类分组与收藏持久化。
//
// - preprocessFunctions:剔除内部与文件级功能;系统级功能只在选中系统的系统族
//   适用时列出,检材级功能总是列出;
// - 同一插件的功能归并为一项,勾选插件即以默认选项运行它的全部功能;
// - 分类 = { favour(收藏), ...各分类, longtime };
// - 选中的系统按检材记忆(localStorage),缺省为第一个识别出的系统。
import { computed, ref } from 'vue';

import { usePersistedRef } from '../../../composables/usePersistedRef';
import type { DetectedSystem, SchedFunction } from '../../../lib/ipc/tasks';
import { categoryLabel, declaredPluginText, pluginTextKey, type PluginTextLookup } from '../../../lib/models/pluginText';

/** longtime 耗时任务分类固定在侧栏末尾。 */
export const LONGTIME_TYPE = 'longtime';

/** 快速分析的勾选单位:一个插件在选中系统(与整个检材)上的全部功能。 */
export interface QuickPlugin {
  id: string;
  name: string;
  typeName: string;
  functions: SchedFunction[];
}

export interface PluginGroup {
  key: string;
  label: string;
  plugins: QuickPlugin[];
}

const FAVOURITE_KEY = 'lovelycarve-quick-analysis-favourites';
const SYSTEM_MEMORY_PREFIX = 'lovelycarve-quick-analysis-system:';

/** 模块级收藏列表(所有快速分析对话框共享,localStorage 持久化)。按插件 id 记;
 *  早先按 function 名(`插件 id/analyzer`)记的条目同样算作其插件。 */
export const favouriteNames = usePersistedRef<string[]>(FAVOURITE_KEY, []);

function namesPlugin(name: string, pluginId: string): boolean {
  return name === pluginId || name.startsWith(`${pluginId}/`);
}

export function isFavouritePlugin(pluginId: string, favourites: string[] = favouriteNames.value): boolean {
  return favourites.some((name) => namesPlugin(name, pluginId));
}

export function toggleFavouritePlugin(pluginId: string): void {
  favouriteNames.value = isFavouritePlugin(pluginId)
    ? favouriteNames.value.filter((name) => !namesPlugin(name, pluginId))
    : [...favouriteNames.value, pluginId];
}

/** Whether a system-scope function runs on systems of `family` (no platforms = every family). */
export function appliesToFamily(fn: SchedFunction, family: string): boolean {
  return fn.platforms.length === 0 || fn.platforms.some((platform) => platform.toLowerCase() === family.toLowerCase());
}

/**
 * 列出可运行的功能:剔除内部功能;文件级功能要指定目标文件(从文件列表右键
 * 提交),快速分析不列出;系统级功能只在选中系统适用时列出。
 */
export function preprocessFunctions(
  functions: SchedFunction[],
  system: Pick<DetectedSystem, 'family'> | null,
): SchedFunction[] {
  return functions.filter((fn) => !fn.internal && (
    fn.scope === 'evidence' || (fn.scope === 'system' && system !== null && appliesToFamily(fn, system.family))
  ));
}

/** 按插件归并功能,插件与其功能都保持清单顺序;名称按界面语言取插件译文。 */
export function groupByPlugin(functions: SchedFunction[], text: PluginTextLookup = declaredPluginText): QuickPlugin[] {
  const plugins = new Map<string, QuickPlugin>();
  for (const fn of functions) {
    const named = { ...fn, displayName: text(fn.pluginId, pluginTextKey.analyzer(fn.analyzer), fn.displayName) };
    const plugin = plugins.get(fn.pluginId);
    if (plugin) {
      plugin.functions.push(named);
    } else {
      plugins.set(fn.pluginId, {
        id: fn.pluginId,
        name: text(fn.pluginId, pluginTextKey.plugin(), fn.pluginName),
        typeName: fn.typeName,
        functions: [named],
      });
    }
  }
  return [...plugins.values()];
}

function pluginMatches(plugin: QuickPlugin, keyword: string): boolean {
  return (
    plugin.name.toLowerCase().includes(keyword) ||
    plugin.id.toLowerCase().includes(keyword) ||
    plugin.functions.some(
      (fn) => fn.displayName.toLowerCase().includes(keyword) || fn.function.toLowerCase().includes(keyword),
    )
  );
}

/**
 * 按收藏 → 各分类 → longtime 组装分类;收藏的插件同时留在自己的分类里。
 * 搜索命中插件名、插件 id,或其任一功能的显示名与 function 名(小写包含)。
 */
export function buildPluginGroups(
  plugins: QuickPlugin[],
  args: {
    favourites: string[];
    search: string;
    text?: PluginTextLookup;
  },
): PluginGroup[] {
  const keyword = args.search.trim().toLowerCase();
  const matched = keyword ? plugins.filter((plugin) => pluginMatches(plugin, keyword)) : plugins;
  const favour = matched.filter((plugin) => isFavouritePlugin(plugin.id, args.favourites));
  const byType = new Map<string, QuickPlugin[]>();
  for (const plugin of matched) {
    const list = byType.get(plugin.typeName) ?? [];
    list.push(plugin);
    byType.set(plugin.typeName, list);
  }
  const groups: PluginGroup[] = [];
  if (favour.length > 0) {
    groups.push({ key: 'favour', get label() { return tr("收藏"); }, plugins: favour });
  }
  // Known types in a fixed reading order, then any other type as first listed.
  const rank = (typeName: string) => {
    const index = TYPE_ORDER.indexOf(typeName);
    return index < 0 ? TYPE_ORDER.length : index;
  };
  const types = [...byType.keys()].filter((typeName) => typeName !== LONGTIME_TYPE);
  types.sort((left, right) => rank(left) - rank(right));
  for (const typeName of types) {
    const members = byType.get(typeName) ?? [];
    groups.push({ key: typeName, label: categoryLabel(typeName, members[0]?.id ?? '', args.text, tr), plugins: members });
  }
  const longtime = byType.get(LONGTIME_TYPE) ?? [];
  if (longtime.length > 0) {
    groups.push({ key: LONGTIME_TYPE, get label() { return tr("耗时任务"); }, plugins: longtime });
  }
  return groups;
}

/** 分类的展示顺序:先系统与用户层面的基本信息,再各类应用,最后文件级分析。 */
const TYPE_ORDER = [
  'baseinfo', 'system', 'browserinfo', 'application', 'loginfo', 'databaseinfo', 'fileanalyzer', 'filesystem', 'analysis',
];

/** 记忆检材上次选择的系统。 */
export function useSystemMemory(imageId: string) {
  return usePersistedRef<string | null>(`${SYSTEM_MEMORY_PREFIX}${imageId}`, null);
}

/** 勾选的插件 id,按勾选顺序提交。插件整体运行:它的每个功能各建一个任务,
 *  选项取默认值(多选子项默认 = 全部)。 */
export function useQuickAnalysisSelection() {
  const selected = ref<Set<string>>(new Set());
  const selectionList = computed(() => [...selected.value]);

  function isSelected(id: string): boolean {
    return selected.value.has(id);
  }

  function toggle(id: string): void {
    const next = new Set(selected.value);
    if (!next.delete(id)) next.add(id);
    selected.value = next;
  }

  /** Select a whole category or list; entries already chosen keep their place. */
  function selectMany(ids: string[]): void {
    const next = new Set(selected.value);
    for (const id of ids) next.add(id);
    selected.value = next;
  }

  function deselectMany(ids: string[]): void {
    const next = new Set(selected.value);
    for (const id of ids) next.delete(id);
    selected.value = next;
  }

  function clear(): void {
    selected.value = new Set();
  }

  return { selectionList, isSelected, toggle, selectMany, deselectMany, clear };
}
