import { computed } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ImageExportProgress, ImageExportResult } from '../../../lib/ipc/image';
import { registerEvidenceContext } from './evidenceContext';
import { disposeImageExportJobs, useImageExport } from './useImageExport';

const ipc = vi.hoisted(() => ({
  save: vi.fn(),
  exportRaw: vi.fn(),
  exportBitlocker: vi.fn(),
  cancel: vi.fn(),
  listen: vi.fn(),
  stop: vi.fn(),
  error: vi.fn(),
  update: vi.fn(),
  dismiss: vi.fn(),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: ipc.save }));
vi.mock('../../../lib/ipc/image', () => ({
  imageExportRaw: ipc.exportRaw,
  IMAGE_EXPORT_PROGRESS_EVENT: 'image-export-progress',
}));
vi.mock('../../../lib/ipc/bitlocker', () => ({ bitlockerExportDecryptedImage: ipc.exportBitlocker }));
vi.mock('../../../lib/ipc/taskRuntime', () => ({ taskCancel: ipc.cancel }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: ipc.listen }));
vi.mock('../../../composables/useToast', () => ({
  notifyError: ipc.error,
  pushPersistentToast: () => ({ update: ipc.update, dismiss: ipc.dismiss }),
}));

const options = { imageId: 'array', defaultFileName: 'raid.dd' };
const result: ImageExportResult = {
  bytes_written: 4096,
  bad_regions: [],
  synthetic_bytes: 0,
  read_faults_detected: false,
};
let context = 0;
let stopContext: () => void;

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.resetAllMocks();
  context = 0;
  stopContext = registerEvidenceContext(() => context);
  ipc.save.mockResolvedValue('D:/exports/raid.dd');
  ipc.listen.mockResolvedValue(ipc.stop);
  ipc.exportRaw.mockResolvedValue(result);
  ipc.exportBitlocker.mockResolvedValue(result.bytes_written);
  ipc.cancel.mockResolvedValue(undefined);
});

afterEach(() => {
  disposeImageExportJobs();
  stopContext();
});

describe('image export task lifecycle', () => {
  it.each(['raw', 'bitlocker'] as const)('releases a completed %s export so the image can be exported again', async mode => {
    const { startImageExport, startBitlockerDecryptedImageExport, jobFor } = useImageExport();
    const startExport = () => mode === 'raw'
      ? startImageExport(options)
      : startBitlockerDecryptedImageExport({ ...options, partitionId: 2 });
    const exporter = mode === 'raw' ? ipc.exportRaw : ipc.exportBitlocker;

    expect(await startExport()).toBe(true);
    expect(jobFor(options.imageId)).toBeUndefined();
    expect(await startExport()).toBe(true);
    expect(jobFor(options.imageId)).toBeUndefined();
    expect(exporter).toHaveBeenCalledTimes(2);
    expect(ipc.stop).toHaveBeenCalledTimes(2);
  });

  it('allows retry after a denied destination', async () => {
    ipc.exportRaw.mockRejectedValueOnce(new Error('permission denied'));
    const { startImageExport, jobFor } = useImageExport();

    expect(await startImageExport(options)).toBe(false);
    expect(jobFor(options.imageId)).toBeUndefined();
    expect(ipc.stop).toHaveBeenCalledOnce();
    expect(await startImageExport(options)).toBe(true);
  });

  it('keeps progress reactive for consumers of the export job', async () => {
    const pending = deferred<ImageExportResult>();
    ipc.exportRaw.mockReturnValueOnce(pending.promise);
    const { startImageExport, jobFor } = useImageExport();
    const operation = startImageExport(options);
    await vi.waitFor(() => expect(ipc.exportRaw).toHaveBeenCalledOnce());
    const bytesDone = computed(() => jobFor(options.imageId)?.bytesDone ?? 0);
    expect(bytesDone.value).toBe(0);

    const onProgress = ipc.listen.mock.calls[0]![1] as (event: { payload: ImageExportProgress }) => void;
    onProgress({ payload: { task_id: jobFor(options.imageId)!.taskId, bytes_done: 1024, bytes_total: 4096 } });
    expect(bytesDone.value).toBe(1024);
    expect(ipc.update).toHaveBeenLastCalledWith({ detail: expect.any(String), progress: 25 });

    pending.resolve(result);
    expect(await operation).toBe(true);
    expect(bytesDone.value).toBe(0);
  });

  it('prevents concurrent exports until the backend confirms cancellation', async () => {
    const pending = deferred<ImageExportResult>();
    ipc.exportRaw.mockReturnValueOnce(pending.promise);
    const { startImageExport, cancelImageExport, jobFor } = useImageExport();
    const operation = startImageExport(options);
    await vi.waitFor(() => expect(ipc.exportRaw).toHaveBeenCalledOnce());
    const taskId = jobFor(options.imageId)!.taskId;

    expect(await startImageExport(options)).toBe(false);
    expect(ipc.save).toHaveBeenCalledOnce();
    await cancelImageExport(options.imageId);
    await cancelImageExport(options.imageId);
    expect(ipc.cancel).toHaveBeenCalledExactlyOnceWith({ taskId });
    expect(jobFor(options.imageId)?.cancelling).toBe(true);
    expect(await startImageExport(options)).toBe(false);
    expect(ipc.stop).not.toHaveBeenCalled();

    pending.reject(new Error('export cancelled'));
    expect(await operation).toBe(false);
    expect(jobFor(options.imageId)).toBeUndefined();
    expect(ipc.stop).toHaveBeenCalledOnce();
    expect(await startImageExport(options)).toBe(true);
  });

  it('releases an abandoned job when the case changes during progress subscription', async () => {
    const subscription = deferred<() => void>();
    ipc.listen.mockReturnValueOnce(subscription.promise);
    const { startImageExport, jobFor } = useImageExport();
    const operation = startImageExport(options);
    await vi.waitFor(() => expect(ipc.listen).toHaveBeenCalledOnce());

    context += 1;
    subscription.resolve(ipc.stop);
    expect(await operation).toBe(false);
    expect(jobFor(options.imageId)).toBeUndefined();
    expect(ipc.exportRaw).not.toHaveBeenCalled();
    expect(ipc.stop).toHaveBeenCalledOnce();
    expect(ipc.dismiss).toHaveBeenCalledOnce();
  });

  it('ends as cancelled when the task center cancels the export', async () => {
    const pending = deferred<ImageExportResult>();
    ipc.exportRaw.mockReturnValueOnce(pending.promise);
    const { startImageExport, jobFor } = useImageExport();
    const operation = startImageExport(options);
    await vi.waitFor(() => expect(ipc.exportRaw).toHaveBeenCalledOnce());
    // The task center cancels the backend task by its id; the command ends with a cancellation.
    expect(ipc.exportRaw.mock.calls[0]![0]).toMatchObject({ taskId: jobFor(options.imageId)!.taskId });
    pending.reject(new Error('export cancelled'));
    expect(await operation).toBe(false);
    expect(ipc.update).toHaveBeenLastCalledWith(expect.objectContaining({ kind: 'info', title: '导出已取消' }));
    expect(jobFor(options.imageId)).toBeUndefined();
  });

  it('does not clear a newer job when an earlier session completes late', async () => {
    const earlier = deferred<ImageExportResult>();
    const newer = deferred<ImageExportResult>();
    ipc.exportRaw.mockReturnValueOnce(earlier.promise).mockReturnValueOnce(newer.promise);
    const { startImageExport, jobFor } = useImageExport();
    const first = startImageExport(options);
    await vi.waitFor(() => expect(ipc.exportRaw).toHaveBeenCalledOnce());
    disposeImageExportJobs();
    const second = startImageExport(options);
    await vi.waitFor(() => expect(ipc.exportRaw).toHaveBeenCalledTimes(2));
    const currentTaskId = jobFor(options.imageId)!.taskId;

    earlier.resolve(result);
    expect(await first).toBe(true);
    expect(jobFor(options.imageId)?.taskId).toBe(currentTaskId);
    expect(ipc.stop).toHaveBeenCalledOnce();
    newer.resolve(result);
    expect(await second).toBe(true);
    expect(jobFor(options.imageId)).toBeUndefined();
    expect(ipc.stop).toHaveBeenCalledTimes(2);
  });
});
