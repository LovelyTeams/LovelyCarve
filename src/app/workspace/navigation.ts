
import { tr } from '../../composables/useLocale';

import { type NavItem, type NavGroup } from './types';


/** Categories for the workspace rail and their second-level pages. */
export function buildImageNavGroups(options: { supportsRawAnalysis?: boolean } = {}): NavGroup[] {
  const toolItems: NavItem[] = [
    { id: 'search-results', icon: 'Search', get label() { return tr("全局搜索"); }, badge: null, badgeType: '' },
    { id: 'timeline', icon: 'BarChart3', get label() { return tr("时间线"); }, badge: null, badgeType: '' },
    ...(options.supportsRawAnalysis === false
      ? []
      : [{ id: 'carve', icon: 'FileSearch', get label() { return tr("文件雕刻"); }, badge: null, badgeType: '' }]),
  ];
  return [
    {
      groupId: 'image',
      get groupLabel() { return tr("镜像"); },
      groupIcon: 'HardDrive',
      groupColor: 'var(--primary-color)',
      items: [
        { id: 'dashboard', icon: 'Info', get label() { return tr("镜像信息"); }, badge: null, badgeType: '' },
        { id: 'files', icon: 'Files', get label() { return tr("文件列表"); }, badge: null, badgeType: '' },
      ],
    },
    {
      groupId: 'tools',
      get groupLabel() { return tr("工具"); },
      groupIcon: 'Wrench',
      groupColor: 'var(--primary-color)',
      items: toolItems,
    },
    {
      groupId: 'analysis',
      get groupLabel() { return tr("分析"); },
      groupIcon: 'ScanSearch',
      groupColor: 'var(--primary-color)',
      items: [{ id: 'analysis', icon: 'ScanSearch', get label() { return tr("分析结果"); }, badge: null, badgeType: '' }],
    },
  ];
}
