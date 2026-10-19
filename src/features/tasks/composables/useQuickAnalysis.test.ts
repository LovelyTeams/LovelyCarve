import { beforeEach, describe, expect, it } from 'vitest';

import type { SchedFunction } from '../../../lib/ipc/tasks';
import {
  appliesToFamily,
  buildPluginGroups,
  favouriteNames,
  groupByPlugin,
  isFavouritePlugin,
  preprocessFunctions,
  toggleFavouritePlugin,
  useQuickAnalysisSelection,
} from './useQuickAnalysis';

function fn(overrides: Partial<SchedFunction> = {}): SchedFunction {
  return {
    function: 'org.test/demo',
    pluginId: 'org.test',
    pluginName: 'Demo',
    pluginVersion: '1',
    analyzer: 'demo',
    displayName: '演示功能',
    typeName: 'baseinfo',
    scope: 'system',
    platforms: ['linux'],
    trigger: 'manually',
    internal: false,
    singleInstance: false,
    provides: [],
    requires: [],
    extensions: [],
    mimeTypes: [],
    inputs: [],
    options: [],
    ...overrides,
  };
}

describe('preprocessFunctions', () => {
  const functions = [
    fn({ function: 'internal/one', internal: true }),
    fn({ function: 'linux/only' }),
    fn({ function: 'any/system', platforms: [] }),
    fn({ function: 'linux/file', scope: 'files' }),
    fn({ function: 'whole/evidence', scope: 'evidence', platforms: [] }),
    fn({ function: 'win/only', platforms: ['Windows'] }),
  ];

  it('lists the selected system’s analyzers and every evidence analyzer, never internal or file ones', () => {
    expect(preprocessFunctions(functions, { family: 'linux' }).map((item) => item.function))
      .toEqual(['linux/only', 'any/system', 'whole/evidence']);
    expect(preprocessFunctions(functions, { family: 'windows' }).map((item) => item.function))
      .toEqual(['any/system', 'whole/evidence', 'win/only']);
  });

  it('keeps only evidence analyzers when no system was detected', () => {
    expect(preprocessFunctions(functions, null).map((item) => item.function)).toEqual(['whole/evidence']);
  });

  it('matches system families case-insensitively and treats no platforms as every family', () => {
    expect(appliesToFamily(fn({ platforms: ['Linux'] }), 'linux')).toBe(true);
    expect(appliesToFamily(fn({ platforms: [] }), 'macos')).toBe(true);
    expect(appliesToFamily(fn({ platforms: ['windows'] }), 'linux')).toBe(false);
  });
});

describe('plugin groups', () => {
  const plugin = (id: string, name: string, typeName: string, analyzers: Array<[string, string]>) =>
    analyzers.map(([analyzer, displayName]) =>
      fn({ function: `${id}/${analyzer}`, pluginId: id, pluginName: name, analyzer, displayName, typeName }));
  const functions = [
    ...plugin('org.user', 'Windows用户痕迹', 'baseinfo', [['wps', '最近访问的WPS文件'], ['desktop', '用户桌面']]),
    ...plugin('org.mail', '邮件分析', 'fileanalyzer', [['mail', '邮件']]),
    ...plugin('org.hash', '全盘哈希', 'longtime', [['hash', '哈希比对']]),
    ...plugin('org.todesk', 'ToDesk配置登记', 'baseinfo', [['config', 'ToDesk配置与clientId声明']]),
    ...plugin('org.evtx', 'Windows完整事件日志', 'system', [['evtx', '事件日志']]),
  ];

  it('merges the functions of one plugin into a single entry in list order', () => {
    const plugins = groupByPlugin(functions);
    expect(plugins.map((item) => item.id)).toEqual(['org.user', 'org.mail', 'org.hash', 'org.todesk', 'org.evtx']);
    expect(plugins[0]).toMatchObject({ name: 'Windows用户痕迹', typeName: 'baseinfo' });
    expect(plugins[0].functions.map((item) => item.displayName)).toEqual(['最近访问的WPS文件', '用户桌面']);
  });

  it('groups favourites first, then types with Chinese labels, longtime last', () => {
    const groups = buildPluginGroups(groupByPlugin(functions), { favourites: ['org.todesk'], search: '' });
    expect(groups.map((group) => [group.key, group.label])).toEqual([
      ['favour', '收藏'], ['baseinfo', '基本信息'], ['system', '系统记录'], ['fileanalyzer', '文件分析'], ['longtime', '耗时任务'],
    ]);
    expect(groups[0].plugins.map((item) => item.id)).toEqual(['org.todesk']);
    expect(groups[1].plugins.map((item) => item.id)).toEqual(['org.user', 'org.todesk']);
  });

  it('names plugins, analyzers and plugin-defined categories from the package translations', () => {
    const text = (pluginId: string, key: string, fallback: string) =>
      ({ 'org.mail:plugin.name': 'Mail', 'org.mail:analyzer.mail': 'Messages', 'org.car:category.vehicle': '汽车取证' })[`${pluginId}:${key}`] ?? fallback;
    const plugins = groupByPlugin([...functions, fn({ function: 'org.car/can', pluginId: 'org.car', pluginName: 'Car', analyzer: 'can', typeName: 'vehicle' })], text);
    expect(plugins.find((item) => item.id === 'org.mail')).toMatchObject({ name: 'Mail', functions: [{ displayName: 'Messages' }] });
    const groups = buildPluginGroups(plugins, { favourites: [], search: 'messages', text });
    expect(groups.map((group) => group.label)).toEqual(['文件分析']);
    expect(buildPluginGroups(plugins, { favourites: [], search: 'car', text }).map((group) => group.label)).toEqual(['汽车取证']);
  });

  it('finds a plugin by its name, id or any of its functions', () => {
    const plugins = groupByPlugin(functions);
    const ids = (search: string) =>
      buildPluginGroups(plugins, { favourites: [], search }).flatMap((group) => group.plugins.map((item) => item.id));
    expect(ids('todesk')).toEqual(['org.todesk']);
    expect(ids('用户桌面')).toEqual(['org.user']);
    expect(ids('org.mail/mail')).toEqual(['org.mail']);
    expect(ids('不存在')).toEqual([]);
  });
});

describe('favourites', () => {
  beforeEach(() => {
    favouriteNames.value = [];
  });

  it('toggles a plugin and still honours entries saved per function', () => {
    expect(isFavouritePlugin('org.user')).toBe(false);
    toggleFavouritePlugin('org.user');
    expect(isFavouritePlugin('org.user')).toBe(true);
    expect(isFavouritePlugin('org.use')).toBe(false);
    toggleFavouritePlugin('org.user');
    expect(isFavouritePlugin('org.user')).toBe(false);

    favouriteNames.value = ['org.user/wps', 'org.mail'];
    expect(isFavouritePlugin('org.user')).toBe(true);
    toggleFavouritePlugin('org.user');
    expect(favouriteNames.value).toEqual(['org.mail']);
  });
});

describe('useQuickAnalysisSelection', () => {
  it('tracks whole plugins by id', () => {
    const selection = useQuickAnalysisSelection();
    selection.toggle('org.user');
    expect(selection.isSelected('org.user')).toBe(true);
    expect(selection.selectionList.value).toEqual(['org.user']);
    selection.toggle('org.user');
    expect(selection.isSelected('org.user')).toBe(false);
    selection.toggle('org.user');
    selection.clear();
    expect(selection.selectionList.value).toEqual([]);
  });

  it('selects and clears a category in the order plugins were ticked', () => {
    const selection = useQuickAnalysisSelection();
    selection.toggle('org.hash');
    selection.selectMany(['org.user', 'org.hash']);
    expect(selection.selectionList.value).toEqual(['org.hash', 'org.user']);
    selection.deselectMany(['org.user', 'org.hash']);
    expect(selection.selectionList.value).toEqual([]);
  });
});
