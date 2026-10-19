// 插件包目录:已发现的插件包、加载失败的包与各包的语言包。模块级单例,首次
// 使用时读取,安装 / 卸载后重读(调度器的功能清单随之刷新)。插件声明的标签
// 按界面语言取包内译文,缺省为插件声明的中文。
import { computed, shallowRef } from 'vue';

import { useLocale } from '../../../composables/useLocale';
import {
  pluginInstall,
  pluginList,
  pluginRestart,
  pluginUninstall,
  type InstalledPluginPackage,
  type PluginListResponse,
} from '../../../lib/ipc/plugins';
import { pluginText, type PluginTextLookup } from '../../../lib/models/pluginText';
import { useTaskCenter } from './useTaskCenter';

const catalog = shallowRef<PluginListResponse | null>(null);
const loading = shallowRef(false);
const error = shallowRef<string | null>(null);
let pending: Promise<void> | null = null;
let generation = 0;

function refresh(): Promise<void> {
  if (pending) return pending;
  const current = ++generation;
  loading.value = true;
  error.value = null;
  pending = pluginList()
    .then(next => { if (current === generation) catalog.value = next; })
    .catch(cause => { if (current === generation) error.value = String(cause); })
    .finally(() => {
      if (current === generation) loading.value = false;
      pending = null;
    });
  return pending;
}

export function usePluginCatalog() {
  const { locale } = useLocale();
  const taskCenter = useTaskCenter();
  if (!catalog.value && !pending) void refresh();

  const packages = computed(() => catalog.value?.packages ?? []);
  const rejected = computed(() => catalog.value?.rejected ?? []);
  const installDirectory = computed(() => catalog.value?.installDirectory ?? null);
  const byId = computed(() => new Map(packages.value.map(item => [item.id, item])));

  /** A plugin-declared label in the interface language. */
  const text: PluginTextLookup = (pluginId, key, fallback) =>
    pluginText(byId.value.get(pluginId)?.locales, locale.value, key, fallback);

  /** Package and scheduler function lists change together. */
  async function reloadAll(): Promise<void> {
    await Promise.all([refresh(), taskCenter.refresh()]);
  }

  async function install(archive: string): Promise<InstalledPluginPackage> {
    const installed = await pluginInstall(archive);
    await reloadAll();
    return installed;
  }

  async function uninstall(pluginId: string): Promise<void> {
    await pluginUninstall(pluginId);
    await reloadAll();
  }

  async function restart(pluginId: string): Promise<void> {
    await pluginRestart(pluginId);
    await refresh();
  }

  return { packages, rejected, installDirectory, loading, error, refresh, text, install, uninstall, restart };
}

/** Forget the catalog (tests; the workspace keeps it for the session). */
export function resetPluginCatalogForTests(): void {
  generation++;
  pending = null;
  catalog.value = null;
  loading.value = false;
  error.value = null;
}
