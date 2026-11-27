// Plugin market page state: the official index's listing. Instance-scoped
// (one market page). A listing that arrives after a newer request is dropped.
import { ref, shallowRef } from 'vue';

import { tr } from '../../../composables/useLocale';
import { pluginMarketList, type MarketCatalog } from '../../../lib/ipc/pluginMarket';
import { pluginFailure, type PluginFailure } from '../model/pluginPackages';

export function usePluginMarket() {
  const catalog = shallowRef<MarketCatalog | null>(null);
  const loadingCatalog = ref(false);
  const catalogFailure = shallowRef<PluginFailure | null>(null);
  let generation = 0;

  async function loadCatalog(refresh = false): Promise<void> {
    const current = ++generation;
    catalogFailure.value = null;
    loadingCatalog.value = true;
    try {
      const listing = await pluginMarketList(refresh);
      if (current === generation) catalog.value = listing;
    } catch (cause) {
      if (current === generation) catalogFailure.value = pluginFailure(cause, tr("读取插件清单失败"), tr);
    } finally {
      if (current === generation) loadingCatalog.value = false;
    }
  }

  return { catalog, loadingCatalog, catalogFailure, loadCatalog };
}
