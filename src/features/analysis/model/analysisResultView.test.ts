import { describe, expect, it } from 'vitest';
import type { AnalysisResultNode } from '../../../lib/ipc/analysisResults';
import { buildAnalysisGroups } from './analysisResultView';
function node(nodeId: string, kind: AnalysisResultNode['kind'], label: string, children: AnalysisResultNode[] = [], imageId = 'one'): AnalysisResultNode {
  return { nodeId, kind, label, children, imageId, typeName: '', analyzer: 'linux', function: 'plugin/linux', sourceNodeId: null, artifactType: '', runId: '', completedAt: '', childCount: children.length, columns: [] };
}
const users = node('users', 'result', '用户列表');
const logins = node('logins', 'result', '登录记录');
const fixture = [node('disk', 'evidence', 'disk.vmdk', [node('base', 'class', '基本信息', [node('linux', 'function', 'Linux 基本信息', [users, logins])])]), node('other', 'result', '其他检材数据', [], 'two')];
describe('analysis result directory', () => {
  it('flattens redundant ancestors and keeps only current evidence results', () => {
    expect(buildAnalysisGroups(fixture, 'one')).toEqual([{ id: 'linux', label: 'Linux 基本信息', category: '基本信息', results: [users, logins] }]);
    expect(buildAnalysisGroups(fixture, 'missing')).toEqual([]);
  });
  it('matches result names and their function context', () => {
    expect(buildAnalysisGroups(fixture, 'one', ' 用户 ')[0].results).toEqual([users]);
    expect(buildAnalysisGroups(fixture, 'one', 'LINUX')[0].results).toEqual([users, logins]);
    expect(buildAnalysisGroups(fixture, 'one', '没有')).toEqual([]);
  });
});
