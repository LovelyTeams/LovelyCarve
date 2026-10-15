
import { tr } from '../../composables/useLocale';
import type { BcpCatalog } from '../../lib/ipc/bcp';
import type { NavGroup } from './types';
import { BCP_CATEGORIES, bcpCategory } from '../../features/bcp/model/bcpView';


export function bcpNavGroup(catalog: BcpCatalog): NavGroup {
  const entries = [...BCP_CATEGORIES, { prefix: 'other', get label() { return tr("其他数据"); }, icon: 'Database' }];
  const categoryItems = entries.flatMap(category => {
    const rows = catalog.datasets.filter(dataset => bcpCategory(dataset.code) === category.prefix)
      .reduce((sum, dataset) => sum + dataset.rows, 0);
    return rows ? [{
      id: `bcp:${category.prefix}`,
      icon: category.icon,
      get label() { return tr(category.label); },
      badge: rows,
      badgeType: 'info',
    }] : [];
  });
  return {
    groupId: 'bcp',
    get groupLabel() { return tr("BCP 数据"); },
    groupIcon: 'Database',
    groupColor: 'var(--primary-color)',
    items: [{ id: 'bcp', icon: 'Database', get label() { return tr("全部数据"); }, badge: null, badgeType: '' }, ...categoryItems],
  };
}
