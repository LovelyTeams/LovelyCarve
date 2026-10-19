import { describe, expect, it } from 'vitest';
import type { AnalysisResultNode } from '../../../lib/ipc/analysisResults';
import { buildAnalysisGroups, groupOfNode, sectionAnalysisGroups } from './analysisResultView';

function node(nodeId: string, kind: AnalysisResultNode['kind'], label: string, children: AnalysisResultNode[] = [], extra: Partial<AnalysisResultNode> = {}): AnalysisResultNode {
  return {
    nodeId, kind, label, children, imageId: 'one', typeName: 'baseinfo', pluginId: 'plugin', analyzer: 'linux', function: 'plugin/linux',
    sourceNodeId: null, targetLabel: '', artifactType: '', runId: '', completedAt: '', childCount: children.length, warningCount: 0, legacy: false,
    ...extra,
  };
}
const users = node('users', 'result', '用户列表');
const logins = node('logins', 'result', '登录记录');
const kali = node('linux', 'function', 'Linux 基本信息', [users, logins], { targetLabel: 'Kali · part:2', warningCount: 2, runId: 'run-1' });
const quiet = node('quiet', 'function', '只有告警', [], { targetLabel: 'Debian · part:3', warningCount: 1, runId: 'run-2' });
const fixture = [
  node('disk', 'evidence', 'disk.vmdk', [node('base', 'class', 'baseinfo', [kali, quiet])]),
  node('other', 'result', '其他检材数据', [], { imageId: 'two' }),
];

describe('analysis result directory', () => {
  it('keeps one group per run, with its target, warnings and record types', () => {
    expect(buildAnalysisGroups(fixture, 'one')).toEqual([
      { id: 'linux', label: 'Linux 基本信息', category: 'baseinfo', target: 'Kali · part:2', run: kali, warningCount: 2, legacy: false, results: [users, logins] },
      { id: 'quiet', label: '只有告警', category: 'baseinfo', target: 'Debian · part:3', run: quiet, warningCount: 1, legacy: false, results: [] },
    ]);
    expect(buildAnalysisGroups(fixture, 'missing')).toEqual([]);
  });
  it('matches result names, the run and its target', () => {
    expect(buildAnalysisGroups(fixture, 'one', ' 用户 ').map(group => [group.id, group.results])).toEqual([['linux', [users]]]);
    expect(buildAnalysisGroups(fixture, 'one', 'KALI')[0].results).toEqual([users, logins]);
    expect(buildAnalysisGroups(fixture, 'one', 'debian').map(group => group.id)).toEqual(['quiet']);
    expect(buildAnalysisGroups(fixture, 'one', '没有')).toEqual([]);
  });
  it('heads runs by category in first-seen order and finds the run holding a node', () => {
    const browser = node('chrome', 'function', 'Chrome', [node('visits', 'result', '访问记录')], { typeName: 'browserinfo', runId: 'run-3' });
    const groups = buildAnalysisGroups([node('disk', 'evidence', 'disk.vmdk', [
      node('base', 'class', 'baseinfo', [kali]),
      node('browser', 'class', 'browserinfo', [browser]),
      node('base-late', 'class', 'baseinfo', [quiet]),
    ])], 'one');
    expect(sectionAnalysisGroups(groups).map(section => [section.category, section.groups.map(group => group.id)]))
      .toEqual([['baseinfo', ['linux', 'quiet']], ['browserinfo', ['chrome']]]);
    expect([kali, logins, browser.children[0], null].map(item => groupOfNode(groups, item)?.id ?? null)).toEqual(['linux', 'linux', 'chrome', null]);
  });
  it('names nodes and categories through the given labels', () => {
    const [group] = buildAnalysisGroups(fixture, 'one', 'Users', {
      node: item => item.kind === 'result' ? `${item.nodeId}!` : item.label.toUpperCase(),
      category: item => `${item.typeName}?`,
    });
    expect([group.label, group.category, group.results]).toEqual(['LINUX 基本信息', 'baseinfo?', [users]]);
  });
});
