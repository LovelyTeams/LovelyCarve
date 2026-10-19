import { describe, expect, it } from 'vitest';
import type { BackendTaskKind, BuiltinTaskEntry, TaskView } from '../../../lib/ipc/taskRuntime';
import type { SchedTaskRecord } from '../../../lib/ipc/tasks';
import {
  builtinMessage,
  builtinSubject,
  builtinTaskTitle,
  pluginTaskEntry,
  taskEntryElapsedSecs,
  taskEntryIndeterminate,
  taskEntryPercent,
  taskEntryResultPanel,
} from './taskEntryView';

function view(extra: Partial<TaskView> = {}): TaskView {
  return { subject: [], stage: null, progress: null, counts: {}, labels: {}, flags: [], ...extra };
}

function builtin(kind: BackendTaskKind, extra: Partial<BuiltinTaskEntry> = {}): BuiltinTaskEntry {
  return {
    id: `${kind}-1`, imageId: 'disk-a', status: 'running', cancelling: false, automatic: false,
    createdAtMs: 1_000, startedAtMs: 1_000, finishedAtMs: null, progress: null, error: null, actions: ['cancel'],
    source: 'builtin', kind, view: view(), ...extra,
  };
}

function job(extra: Partial<SchedTaskRecord> = {}): SchedTaskRecord {
  return {
    id: 7, name: 'org.lovelycarve.linuxinfo/linuxbaseinfo', displayName: 'Linux 基本信息', typeName: 'baseinfo', evidenceId: 1,
    progress: 40, params: { nodeId: null, fileName: null, systemId: 'linux@part:2', targetLabel: 'Kali · part:2', options: {} },
    result: null, dependence: '', status: 'running',
    info: { pluginId: 'org.lovelycarve.linuxinfo', analyzer: 'linuxbaseinfo', imageId: 'disk-a', trigger: 'auto', parentTid: null, error: null },
    canPause: false, singleInstance: true, internal: false, spendTimeSecs: 3, remainTimeSecs: null,
    startAt: '2026-10-19T08:00:01.000Z', createAt: '2026-10-19T08:00:00.000Z', updateAt: '2026-10-19T08:00:04.000Z', ...extra,
  };
}

describe('native task titles and subjects', () => {
  it('names each listed kind and formats what it works on', () => {
    expect(['imageLoad', 'imageExport', 'bitlockerExport', 'volumeSnapshot', 'carveScan', 'dockerPrepare'].map(kind => builtinTaskTitle(kind as BackendTaskKind)))
      .toEqual(['检材加载', '镜像导出', '镜像导出', '文件系统加载', '文件恢复', 'Docker 分析']);
    expect(builtinSubject(builtin('volumeSnapshot', { view: view({ subject: ['part:2'] }) }))).toBe('分区 2');
    expect(builtinSubject(builtin('sessionSnapshot', { view: view({ subject: ['root'] }) }))).toBe('根目录');
    expect(builtinSubject(builtin('encryptedVolumeUnlock', { view: view({ subject: ['part:2', 'Linux'], labels: { scheme: 'luks' } }) })))
      .toBe('LUKS · 分区 2 · Linux');
    expect(builtinSubject(builtin('encryptedVolumeUnlock', { view: view({ labels: { scheme: 'veracrypt' } }) })))
      .toBe('VeraCrypt · 整个镜像 / 容器');
    expect(builtinSubject(builtin('globalSearch', { view: view({ subject: ['password', 'token'] }) }))).toBe('password、token');
    expect(builtinSubject(builtin('imageExport', { view: view({ subject: ['D:/exports/disk.dd'] }) }))).toBe('D:/exports/disk.dd');
  });
});

describe('native task status lines', () => {
  it('reports exports, their read faults and their cancellation', () => {
    const running = builtin('imageExport', { progress: { unit: 'bytes', done: 1024, total: 4096 } });
    expect(builtinMessage(running)).toBe('1.0 KB / 4.0 KB');
    expect(builtinMessage({ ...running, status: 'success', view: view({ counts: { syntheticBytes: 512, badRegions: 2 } }) }))
      .toBe('已导出 1.0 KB · 警告：含 512 B 补零字节（2 个坏区）');
    expect(builtinMessage({ ...running, status: 'cancel' })).toBe('导出已取消（保留部分文件）');
  });
  it('follows snapshot stages and a reused index', () => {
    const walking = builtin('volumeSnapshot', { view: view({ stage: 'filesystem', counts: { entries: 1200 } }) });
    expect(builtinMessage(walking)).toBe('已读取 1,200 项');
    expect(builtinMessage({ ...walking, view: view({ stage: 'index', counts: { entries: 1200 } }) })).toBe('已写入 1,200 项，正在建立索引');
    expect(builtinMessage({ ...walking, status: 'success', view: view({ counts: { entries: 9 }, flags: ['truncated'] }) }))
      .toBe('索引已就绪 · 9 项 · 已达条目上限，结果不完整');
    expect(builtinMessage({ ...walking, status: 'success', view: view({ counts: { entries: 9 }, flags: ['reused'] }) })).toBe('已复用现有索引 · 9 项');
  });
  it('states detection conclusions, search hits, carving and container merges', () => {
    const detected = builtin('autoDetect', { status: 'success', view: view({ labels: { platform: 'windows', evidenceType: 'disk' } }) });
    expect(builtinMessage(detected)).toBe('Windows · 磁盘镜像');
    expect(builtinMessage({ ...detected, view: view({ labels: { platform: 'android', platformName: 'Huawei Android', evidenceType: 'disk' }, flags: ['backup'] }) }))
      .toBe('Huawei Android · 备份');
    expect(builtinMessage({ ...detected, view: view({ labels: { evidenceType: 'disk' } }) })).toBe('未识别到平台 · 磁盘镜像');
    const search = builtin('globalSearch', { progress: { unit: 'entries', done: 2, total: 10 }, view: view({ counts: { hits: 5, matchedFiles: 1 } }) });
    expect(builtinMessage(search)).toBe('已扫描 2 / 10 个文件 · 5 处命中');
    expect(builtinMessage({ ...search, status: 'success' })).toBe('1 个命中文件 · 5 处命中');
    const carve = builtin('carveScan', { progress: { unit: 'bytes', done: 256, total: 1024 }, view: view({ stage: 'signature', counts: { found: 3 } }) });
    expect(builtinMessage(carve)).toBe('已扫描 256 B / 1.0 KB · 发现 3 个文件');
    expect(builtinMessage({ ...carve, status: 'success', view: view({ stage: 'filesystem', counts: { found: 4 }, flags: ['truncated'] }) }))
      .toBe('发现 4 个文件 · 已达结果上限');
    expect(builtinMessage(builtin('dockerPrepare', { status: 'success', view: view({ counts: { entries: 42 }, flags: ['persisted'] }) })))
      .toBe('索引已保存 · 42 项，下次直接打开');
    expect(builtinMessage(builtin('imageLoad', { progress: { unit: 'bytes', done: 0, total: 2048 }, view: view({ stage: 'decompressing', labels: { item: 'disk.vmdk' } }) })))
      .toBe('解压 disk.vmdk · 0 B / 2.0 KB');
  });
});

describe('task entry progress, time and result pages', () => {
  it('derives shares, unknown amounts and elapsed time', () => {
    expect(taskEntryPercent({ status: 'running', progress: { unit: 'bytes', done: 1, total: 4 } })).toBe(25);
    expect(taskEntryPercent({ status: 'success', progress: null })).toBe(100);
    expect(taskEntryPercent({ status: 'running', progress: { unit: 'percent', done: 140, total: 100 } })).toBe(100);
    expect(taskEntryIndeterminate({ status: 'running', progress: null })).toBe(true);
    expect(taskEntryIndeterminate({ status: 'running', progress: { unit: 'entries', done: 9, total: null } })).toBe(true);
    expect(taskEntryIndeterminate({ status: 'running', progress: { unit: 'percent', done: 0, total: 100 } })).toBe(false);
    expect(taskEntryElapsedSecs(builtin('imageHash'), 4_000)).toBe(3);
    expect(taskEntryElapsedSecs(builtin('imageHash', { finishedAtMs: 2_000 }), 9_000)).toBe(1);
    expect(taskEntryElapsedSecs(pluginTaskEntry(job()), 9_000)).toBe(3);
  });
  it('leads native work to its page and plugin jobs to published results', () => {
    expect((['globalSearch', 'timelineBuild', 'carveScan', 'dockerPrepare', 'volumeSnapshot', 'autoDetect', 'imageLoad'] as const)
      .map(kind => taskEntryResultPanel(builtin(kind)))).toEqual(['search-results', 'timeline', 'carve', 'files', 'files', 'dashboard', null]);
    const published = pluginTaskEntry(job({ status: 'success', result: { summary: null, recordCount: 0, timelineCount: 0, warningCount: 2, syntheticBytes: 0, runId: 'run-1' } }));
    expect(taskEntryResultPanel(published)).toBe('analysis');
    expect(taskEntryResultPanel(pluginTaskEntry(job({ status: 'success' })))).toBeNull();
  });
  it('lists a plugin job the way the backend does', () => {
    expect(pluginTaskEntry(job())).toMatchObject({
      id: 'plugin:7', imageId: 'disk-a', status: 'running', automatic: true, actions: ['cancel'],
      createdAtMs: Date.UTC(2026, 9, 19, 8), startedAtMs: Date.UTC(2026, 9, 19, 8, 0, 1), finishedAtMs: null,
      progress: { unit: 'percent', done: 40, total: 100 },
    });
    expect(pluginTaskEntry(job({ status: 'failed' }))).toMatchObject({ actions: ['restart', 'remove'], finishedAtMs: Date.UTC(2026, 9, 19, 8, 0, 4) });
    expect(pluginTaskEntry(job({ status: 'offline' })).actions).toEqual(['cancel', 'start', 'restart']);
  });
});
