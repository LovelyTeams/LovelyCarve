import { beforeEach, describe, expect, it, vi } from 'vitest';

const { invokeMock, listenMock, toastMock, caseMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
  toastMock: {
    notifyError: vi.fn(),
    notifyInfo: vi.fn(),
    notifySuccess: vi.fn(),
  },
  caseMock: {
    refreshActive: vi.fn(),
  },
}));

vi.mock('../../../lib/ipc/client', async importOriginal => ({
  ...await importOriginal<typeof import('../../../lib/ipc/client')>(),
  invokeCmd: invokeMock,
}));

vi.mock('../../../lib/ipc/events', () => ({
  onEvent: listenMock,
}));

vi.mock('../../../composables/useToast', () => toastMock);

vi.mock('../../cases/composables/useCaseSession', async () => {
  const { shallowRef } = await import('vue');
  const activeCase = shallowRef<{ evidence: Array<{ image_id: string }> } | null>(null);
  return {
    useCaseSession: () => ({ activeCase, refreshActive: caseMock.refreshActive }),
    __activeCase: activeCase,
  };
});

import type { AutoDetectFailure, AutoDetectReport } from '../../../lib/ipc/autodetect';
import { IpcCommandError } from '../../../lib/ipc/client';
import * as caseSessionModule from '../../cases/composables/useCaseSession';
import { registerAutoDetectReportPublished, applyAutoDetectReport, isAutoDetectRunPending, resetAutoDetectForTests, runAutoDetect, useAutoDetectError, useAutoDetectPending, useAutoDetectReport } from './useAutoDetect';
import { evidenceTypeLabel, manufactorLabel, platformLabel, routeLabel } from '../model/autodetectLabels';

const activeCaseRef = (caseSessionModule as unknown as {
  __activeCase: { value: { evidence: Array<{ image_id: string }> } | null };
}).__activeCase;

function report(overrides: Partial<AutoDetectReport> = {}): AutoDetectReport {
  return {
    image_id: 'img-1',
    route: 'disk_image',
    platform: 'windows',
    platforms: ['windows'],
    evidence_type: 'disk',
    is_backup: false,
    manufactor: null,
    display_platform_name: 'Windows',
    friend_platform_name: 'Windows',
    partitions: [],
    backup: null,
    warnings: [],
    ...overrides,
  };
}

beforeEach(() => {
  resetAutoDetectForTests();
  invokeMock.mockReset();
  listenMock.mockReset();
  listenMock.mockResolvedValue(() => {});
  toastMock.notifyError.mockReset();
  caseMock.refreshActive.mockReset();
  caseMock.refreshActive.mockResolvedValue(undefined);
  activeCaseRef.value = null;
  registerAutoDetectReportPublished(report => {
    if (activeCaseRef.value?.evidence.some(item => item.image_id === report.image_id)) void caseMock.refreshActive();
  });
});

describe('useAutoDetect labels', () => {
  it('maps reference platform words to Chinese-first labels', () => {
    expect(platformLabel('android')).toBe('Android');
    expect(platformLabel('harmony')).toBe('鸿蒙 HarmonyOS');
    expect(platformLabel('huaweihmos')).toBe('鸿蒙 HMOS');
    expect(platformLabel('othermobile')).toBe('其他移动设备');
    expect(platformLabel(null)).toBe('未识别');
    expect(platformLabel(undefined)).toBe('未识别');
  });

  it('labels routes, evidence types and manufactors', () => {
    expect(routeLabel('disk_image')).toBe('磁盘镜像路');
    expect(routeLabel('archive')).toBe('归档/备份文件路');
    expect(routeLabel('phone_image')).toBe('手机镜像路');
    expect(evidenceTypeLabel('disk')).toBe('磁盘镜像');
    expect(evidenceTypeLabel('compressfile')).toBe('归档 / 逻辑检材');
    expect(evidenceTypeLabel('backup')).toBe('备份文件');
    expect(evidenceTypeLabel('huaweiotgbackuptotar')).toBe('huaweiotgbackuptotar');
    expect(manufactorLabel('huawei')).toBe('Huawei');
    expect(manufactorLabel(null)).toBeNull();
    expect(manufactorLabel('')).toBeNull();
  });
});

describe('useAutoDetect state', () => {
  it('runs the command, stores the report and clears pending', async () => {
    const payload = report();
    invokeMock.mockResolvedValue(payload);
    const current = useAutoDetectReport(() => 'img-1');
    const pending = useAutoDetectPending(() => 'img-1');
    expect(current.value).toBeUndefined();

    const promise = runAutoDetect('img-1');
    expect(pending.value).toBe(true);
    const result = await promise;

    expect(result).toEqual(payload);
    expect(invokeMock).toHaveBeenCalledWith('autodetect_evidence', { imageId: 'img-1' });
    expect(current.value).toEqual(payload);
    expect(pending.value).toBe(false);
  });

  it('records the error and toasts when the command fails', async () => {
    invokeMock.mockRejectedValue(new Error('image not loaded'));
    const error = useAutoDetectError(() => 'img-1');

    const result = await runAutoDetect('img-1');

    expect(result).toBeNull();
    expect(error.value).toContain('image not loaded');
    expect(toastMock.notifyError).toHaveBeenCalledWith('自动识别失败', expect.stringContaining('image not loaded'));
  });

  it('applies finished-event payloads pushed by the backend first task', async () => {
    const current = useAutoDetectReport(() => 'img-2');
    expect(listenMock).toHaveBeenCalledWith('autodetect-finished', expect.any(Function));
    const handler = listenMock.mock.calls[0]![1] as (event: { payload: AutoDetectReport }) => void;

    handler({ payload: report({ image_id: 'img-2', platform: 'android', evidence_type: 'backup', is_backup: true }) });

    expect(current.value?.platform).toBe('android');
    expect(current.value?.is_backup).toBe(true);
  });

  it('keeps reports per image and clears a stale error on a fresh report', async () => {
    invokeMock.mockRejectedValueOnce(new Error('boom'));
    await runAutoDetect('img-3');
    expect(useAutoDetectError(() => 'img-3').value).toContain('boom');

    applyAutoDetectReport(report({ image_id: 'img-3', platform: 'linux' }));
    expect(useAutoDetectError(() => 'img-3').value).toBeUndefined();
    expect(useAutoDetectReport(() => 'img-3').value?.platform).toBe('linux');
    expect(useAutoDetectReport(() => 'img-1').value).toBeUndefined();
  });

  it('refreshes the active case only when the image is case evidence', () => {
    applyAutoDetectReport(report({ image_id: 'img-1' }));
    expect(caseMock.refreshActive).not.toHaveBeenCalled();

    activeCaseRef.value = { evidence: [{ image_id: 'img-1' }] };
    applyAutoDetectReport(report({ image_id: 'img-9' }));
    expect(caseMock.refreshActive).not.toHaveBeenCalled();

    applyAutoDetectReport(report({ image_id: 'img-1' }));
    expect(caseMock.refreshActive).toHaveBeenCalledTimes(1);
  });

  it('shows a failed background run but leaves a cancelled one silent', () => {
    useAutoDetectReport(() => 'img-4');
    expect(listenMock).toHaveBeenCalledWith('autodetect-failed', expect.any(Function));
    const handler = listenMock.mock.calls[1]![1] as (event: { payload: AutoDetectFailure }) => void;
    const error = useAutoDetectError(() => 'img-4');

    handler({ payload: { image_id: 'img-4', error: { code: 'cancelled', message: '自动识别已取消', context: null, retryable: false } } });
    expect(error.value).toBeUndefined();

    handler({ payload: { image_id: 'img-4', error: { code: 'taskPanicked', message: 'autodetect_evidence task crashed', context: null, retryable: false } } });
    expect(error.value).toBe('autodetect_evidence task crashed');
  });

  it('does not report a cancelled manual run as a failure', async () => {
    invokeMock.mockRejectedValue(new IpcCommandError({ code: 'cancelled', message: '自动识别已取消', context: null, retryable: false }));

    const promise = runAutoDetect('img-5');
    expect(isAutoDetectRunPending('img-5')).toBe(true);
    expect(await promise).toBeNull();

    expect(isAutoDetectRunPending('img-5')).toBe(false);
    expect(useAutoDetectError(() => 'img-5').value).toBeUndefined();
    expect(toastMock.notifyError).not.toHaveBeenCalled();
  });
});

it('releases the successful half of a failed subscription attempt before retry', async () => {
  const stop = vi.fn(); listenMock.mockResolvedValueOnce(stop).mockRejectedValueOnce(new Error('bridge'));
  useAutoDetectReport(() => 'image'); for (let index = 0; index < 8; index++) await Promise.resolve();
  expect(stop).toHaveBeenCalledOnce(); useAutoDetectReport(() => 'image');
  for (let index = 0; index < 8; index++) await Promise.resolve(); expect(listenMock).toHaveBeenCalledTimes(4);
});
it('rejects a manual result arriving after the evidence was unloaded', async () => {
  let resolve!: (value: AutoDetectReport) => void; invokeMock.mockReturnValueOnce(new Promise(yes => { resolve = yes; }));
  const running = runAutoDetect('unloaded');
  const { clearAutoDetectReport } = await import('./useAutoDetect'); clearAutoDetectReport('unloaded');
  resolve(report({ image_id: 'unloaded' })); expect(await running).toBeNull();
  expect(useAutoDetectReport(() => 'unloaded').value).toBeUndefined();
});
