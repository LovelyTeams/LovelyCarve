import { describe, expect, it } from 'vitest';
import { analysisSources } from './analysisSource';
import type { AnalysisRecord, RecordSource } from '../../../lib/ipc/analysisResults';

const record = (sources: RecordSource[], imageId = ''): AnalysisRecord => ({
  recordId: '1', runId: 'run', recordType: 'test.record', title: 'Title', severity: null, values: {}, sources, imageId, legacy: false,
});
const legacy = (values: Record<string, unknown>, sources: RecordSource[] = []): AnalysisRecord => ({ ...record(sources), recordId: 'artifact', values, legacy: true });

describe('plugin source addresses', () => {
  it('locates a volume path before a node, keeping the byte range and the note', () => {
    expect(analysisSources(record([{ root: 'host-docker:container:', path: 'etc/passwd', nodeId: 'ext4:part:0:42', offset: 128, length: 32, note: '第 3 行' }]), 'disk-b'))
      .toEqual([{ label: 'etc/passwd', note: '第 3 行', target: { imageId: 'disk-b', rootId: 'host-docker:container:', path: 'etc/passwd', offset: 128, length: 32 } }]);
  });
  it('uses the node when the source names no volume path, and drops duplicates', () => {
    const source = { nodeId: 'node-7' };
    expect(analysisSources(record([source, source, {}]), 'disk'))
      .toEqual([{ label: 'node-7', note: null, target: { imageId: 'disk', nodeId: 'node-7' } }]);
  });
  it('lists every source and keeps the evidence the record belongs to', () => {
    expect(analysisSources(record([{ root: 'part:1', path: 'file;name' }, { root: 'part:1', path: 'etc/shadow' }], 'source-disk'), 'active-disk').map(source => source.target))
      .toEqual([
        { imageId: 'source-disk', rootId: 'part:1', path: 'file;name' },
        { imageId: 'source-disk', rootId: 'part:1', path: 'etc/shadow' },
      ]);
  });
  it('reads the provenance attributes of earlier versions', () => {
    expect(analysisSources(legacy({ 'source.root': 'part:1', 'source.path': 'file;name', 'source.paths': ['file;name', 'etc/shadow'] }), 'disk').map(source => source.label))
      .toEqual(['file;name', 'etc/shadow']);
    expect(analysisSources(legacy({ 'linux.root': 'part:0', 'linux.source': 'etc/passwd;etc/shadow' }), 'disk').map(source => source.label))
      .toEqual(['etc/passwd', 'etc/shadow']);
    expect(analysisSources(legacy({ 'linux.root': 'part:0', 'linux.source': 'dpkg' }), 'disk')).toEqual([]);
    expect(analysisSources(legacy({ 'linux.root': 'part:0', 'linux.source': 'systemd/multi-user.target.wants' }), 'disk')).toEqual([]);
    expect(analysisSources(legacy({ 'source.path': 'etc/passwd' }), 'disk')).toEqual([]);
    expect(analysisSources(legacy({ 'source.node': 42 }), 'disk')).toEqual([]);
  });
  it('falls back to the input node of an earlier file run', () => {
    expect(analysisSources(legacy({}, [{ nodeId: 'input-file' }]), 'disk'))
      .toEqual([{ label: 'input-file', note: null, target: { imageId: 'disk', nodeId: 'input-file' } }]);
  });
});
