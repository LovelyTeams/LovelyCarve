
import { tr } from '../../../composables/useLocale';
// 快速分析的数据组装逻辑:功能清单过滤、平台克隆、按插件归并、分类分组与收藏持久化。
//
// - preprocessFuncs:剔除 Internal 与文件级功能,按检材类型(evidencetype)过滤;
// - Platform === "all"(null)的功能深拷贝到每个平台(去重按 function 名);
// - 同一插件的功能归并为一项,勾选插件即运行它的全部功能;
// - 分类 = { favour(收藏), ...各 Type 分组, longtime };
// - 平台上次选择按检材记忆(localStorage),缺省预选检材平台。
import { computed, ref } from 'vue';

import { usePersistedRef } from '../../../composables/usePersistedRef';
import type { SchedFunction } from '../../../lib/ipc/tasks';

/** 平台列展示顺序。 */
export const PLATFORM_ORDER = [
  'android',
  'harmony',
  'ios',
  'windows',
  'macos',
  'linux',
  'iot',
] as const;

export const PLATFORM_LABELS: Record<string, string> = {
  android: 'Android',
  get harmony() { return tr("鸿蒙"); },
  ios: 'iOS',
  windows: 'Windows',
  macos: 'macOS',
  linux: 'Linux',
  iot: 'IoT',
};

/** longtime 耗时任务分类固定在侧栏末尾。 */
export const LONGTIME_TYPE = 'longtime';

/** 快速分析的勾选单位:一个插件在当前平台上的全部检材级功能。 */
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
const PLATFORM_MEMORY_PREFIX = 'lovelycarve-quick-analysis-platform:';

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

/** 平台列:由功能清单推导,证据平台优先预选。 */
export function buildPlatforms(
  functions: SchedFunction[],
  evidencePlatform: string | null,
): { platforms: string[]; defaultPlatform: string } {
  const distinct = new Set<string>();
  for (const fn of functions) {
    for (const platform of platformsOf(fn)) {
      distinct.add(platform);
    }
  }
  const platforms: string[] = PLATFORM_ORDER.filter((platform) => distinct.has(platform));
  for (const platform of distinct) {
    if (!platforms.includes(platform)) {
      platforms.push(platform);
    }
  }
  const defaultPlatform =
    evidencePlatform && platforms.includes(evidencePlatform)
      ? evidencePlatform
      : (platforms[0] ?? '');
  return { platforms, defaultPlatform };
}

/** 清单里表示「全平台」的写法:缺省、`all` 或 `any`。 */
const ALL_PLATFORMS = new Set(['all', 'any']);

function platformsOf(fn: SchedFunction): string[] {
  if (fn.platform && !ALL_PLATFORMS.has(fn.platform)) {
    return [fn.platform];
  }
  // 全平台功能对每个平台克隆一份展示。
  return [...PLATFORM_ORDER] as string[];
}

/**
 * preprocessFuncs:剔除 Internal、按检材类型过滤、平台克隆到目标平台。
 * 文件级功能要指定目标文件(从文件列表右键提交),快速分析不列出。
 * 返回去重后的清单(deduplicateTasks:同名只留一个)。
 */
export function preprocessFunctions(
  functions: SchedFunction[],
  args: {
    platform: string;
    evidenceType?: string | null;
  },
): SchedFunction[] {
  const seen = new Set<string>();
  const result: SchedFunction[] = [];
  for (const fn of functions) {
    if (fn.internal || !fn.evidenceLevel) continue;
    // 检材类型未知(尚未自动识别)时不过滤。
    if (
      args.evidenceType &&
      fn.evidenceTypes.length > 0 &&
      !fn.evidenceTypes.includes(args.evidenceType)
    ) {
      continue;
    }
    if (!platformsOf(fn).includes(args.platform)) continue;
    // 克隆体同名去重:"all" 功能在每个平台只留一份。
    if (seen.has(fn.function)) continue;
    seen.add(fn.function);
    result.push(fn);
  }
  return result;
}

/** 按插件归并功能,插件与其功能都保持清单顺序。 */
export function groupByPlugin(functions: SchedFunction[]): QuickPlugin[] {
  const plugins = new Map<string, QuickPlugin>();
  for (const fn of functions) {
    const plugin = plugins.get(fn.pluginId);
    if (plugin) {
      plugin.functions.push(fn);
    } else {
      plugins.set(fn.pluginId, { id: fn.pluginId, name: fn.pluginName, typeName: fn.typeName, functions: [fn] });
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
 * 按收藏 → 各 Type 分组 → longtime 组装分类;收藏的插件同时留在自己的分类里。
 * 搜索命中插件名、插件 id,或其任一功能的显示名与 function 名(小写包含)。
 */
export function buildPluginGroups(
  plugins: QuickPlugin[],
  args: {
    favourites: string[];
    search: string;
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
    groups.push({ key: typeName, label: typeLabel(typeName), plugins: byType.get(typeName) ?? [] });
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

/** 插件清单 `[task] type` 的中文名;未知类型原样显示。 */
function typeLabel(typeName: string): string {
  const known: Record<string, string> = {
    baseinfo: tr("基本信息"),
    system: tr("系统记录"),
    application: tr("应用程序"),
    browserinfo: tr("浏览器"),
    loginfo: tr("日志"),
    databaseinfo: tr("数据库"),
    fileanalyzer: tr("文件分析"),
    filesystem: tr("文件系统"),
    analysis: tr("数据分析"),
    longtime: tr("耗时任务"),
  };
  return known[typeName] ?? typeName;
}

/** 记忆检材上次选择的平台。 */
export function usePlatformMemory(imageId: string) {
  const remembered = usePersistedRef<string | null>(`${PLATFORM_MEMORY_PREFIX}${imageId}`, null);
  return remembered;
}

/** 勾选的插件 id,按勾选顺序提交。插件整体运行:它的每个功能各建一个任务,
 *  不带子项(任务子项为空 = 插件执行全部子项)。 */
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
