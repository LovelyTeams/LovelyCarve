// Plugin market page state: the configured index sources, the selected one
// and its listing. Instance-scoped (one market page); sources persist in the
// backend. A listing that arrives after the selection moved on is dropped.
import { computed, ref, shallowRef, watch } from 'vue';

import { tr } from '../../../composables/useLocale';
import {
  pluginMarketList,
  pluginMarketSetSources,
  pluginMarketSources,
  type MarketCatalog,
} from '../../../lib/ipc/pluginMarket';
import { pluginFailure, type PluginFailure } from '../model/pluginPackages';

export function usePluginMarket() {
  const sources = shallowRef<string[]>([]);
  const selected = ref<string | null>(null);
  const catalog = shallowRef<MarketCatalog | null>(null);
  const loadingSources = ref(false);
  const loadingCatalog = ref(false);
  const saving = ref(false);
  const sourcesFailure = shallowRef<PluginFailure | null>(null);
  const catalogFailure = shallowRef<PluginFailure | null>(null);
  let generation = 0;

  function adopt(next: string[]) {
    sources.value = next;
    if (!selected.value || !next.includes(selected.value)) selected.value = next[0] ?? null;
  }

  async function loadSources(): Promise<void> {
    loadingSources.value = true;
    sourcesFailure.value = null;
    try {
      adopt((await pluginMarketSources()).sources);
    } catch (cause) {
      sourcesFailure.value = pluginFailure(cause, tr("读取索引源失败"), tr);
    } finally {
      loadingSources.value = false;
    }
  }

  /** Stores `next`; throws the backend's refusal for the caller to show at its field. */
  async function saveSources(next: string[]): Promise<void> {
    saving.value = true;
    try {
      adopt((await pluginMarketSetSources(next)).sources);
    } finally {
      saving.value = false;
    }
  }

  async function addSource(source: string): Promise<void> {
    await saveSources([...sources.value, source]);
    selected.value = sources.value[sources.value.length - 1] ?? selected.value;
  }

  function removeSource(source: string): Promise<void> {
    return saveSources(sources.value.filter(stored => stored !== source));
  }

  async function loadCatalog(refresh = false): Promise<void> {
    const source = selected.value;
    const current = ++generation;
    catalogFailure.value = null;
    if (!source) {
      catalog.value = null;
      loadingCatalog.value = false;
      return;
    }
    if (catalog.value?.source !== source) catalog.value = null;
    loadingCatalog.value = true;
    try {
      const listing = await pluginMarketList(source, refresh);
      if (current === generation) catalog.value = listing;
    } catch (cause) {
      if (current === generation) catalogFailure.value = pluginFailure(cause, tr("读取插件清单失败"), tr);
    } finally {
      if (current === generation) loadingCatalog.value = false;
    }
  }

  watch(selected, () => { void loadCatalog(); });

  const sourceOptions = computed(() => sources.value.map(source => ({ value: source, label: source })));

  return {
    sources, selected, sourceOptions, catalog, loadingSources, loadingCatalog, saving, sourcesFailure, catalogFailure,
    loadSources, addSource, removeSource, loadCatalog,
  };
}
