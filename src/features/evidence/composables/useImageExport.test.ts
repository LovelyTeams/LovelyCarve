import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { registerEvidenceContext } from './evidenceContext';
import { disposeImageExportJobs, useImageExport } from './useImageExport';

const ipc = vi.hoisted(() => ({ save: vi.fn(), export: vi.fn(), cancel: vi.fn(), listen: vi.fn(), stop: vi.fn(), error: vi.fn(), update: vi.fn(), dismiss: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: ipc.save }));
vi.mock('../../../lib/ipc/image', () => ({ imageExportRaw: ipc.export, IMAGE_EXPORT_PROGRESS_EVENT: 'image-export-progress' }));
vi.mock('../../../lib/ipc/taskRuntime', () => ({ taskCancel: ipc.cancel }));
vi.mock('../../../lib/ipc/index', () => ({ onEvent: ipc.listen }));
vi.mock('../../../composables/useToast', () => ({ notifyError: ipc.error, pushPersistentToast: () => ({ update: ipc.update, dismiss: ipc.dismiss }) }));

const options = { imageId: 'array', defaultFileName: 'raid.dd' };
const result = { bytes_written: 4096, bad_regions: [], synthetic_bytes: 0, read_faults_detected: false };
let context = 0;
let stopContext: () => void;
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
async function settle() { for (let index = 0; index < 8; index++) await Promise.resolve(); }
beforeEach(() => {
  vi.resetAllMocks();
  context = 0;
  stopContext = registerEvidenceContext(() => context);
  ipc.save.mockResolvedValue('D:/exports/raid.dd');
  ipc.listen.mockResolvedValue(ipc.stop);
  ipc.export.mockResolvedValue(result);
  ipc.cancel.mockResolvedValue(undefined);
});
afterEach(() => { disposeImageExportJobs(); stopContext(); });

describe('image export task lifecycle', () => {
  it('releases a completed export so the same array can be exported again', async () => {
    const { startImageExport, jobFor } = useImageExport();
    expect(await startImageExport(options)).toBe(true);
    expect(jobFor(options.imageId)).toBeUndefined();
    expect(await startImageExport(options)).toBe(true);
    expect(ipc.export).toHaveBeenCalledTimes(2);
    expect(ipc.stop).toHaveBeenCalledTimes(2);
  });

  it('allows retry after a denied destination', async () => {
    ipc.export.mockRejectedValueOnce(new Error('permission denied'));
    const { startImageExport, jobFor } = useImageExport();
    expect(await startImageExport(options)).toBe(false);
    expect(jobFor(options.imageId)).toBeUndefined();
    expect(await startImageExport(options)).toBe(true);
  });

  it('prevents concurrent exports and releases a cancelled task', async () => {
    const pending = deferred<typeof result>();
    ipc.export.mockReturnValueOnce(pending.promise);
    const { startImageExport, cancelImageExport, jobFor } = useImageExport();
    const first = startImageExport(options);
    await settle();
    const taskId = jobFor(options.imageId)!.taskId;
    expect(await startImageExport(options)).toBe(false);
    expect(ipc.save).toHaveBeenCalledOnce();
    await cancelImageExport(options.imageId);
    expect(ipc.cancel).toHaveBeenCalledWith({ taskId });
    expect(jobFor(options.imageId)?.cancelling).toBe(true);
    pending.reject(new Error('export cancelled'));
    expect(await first).toBe(false);
    expect(jobFor(options.imageId)).toBeUndefined();
    expect(await startImageExport(options)).toBe(true);
  });

  it('releases an abandoned job when the case changes during progress subscription', async () => {
    const subscription = deferred<() => void>();
    ipc.listen.mockReturnValueOnce(subscription.promise);
    const { startImageExport, jobFor } = useImageExport();
    const operation = startImageExport(options);
    await settle();
    context++;
    subscription.resolve(ipc.stop);
    expect(await operation).toBe(false);
    expect(jobFor(options.imageId)).toBeUndefined();
    expect(ipc.export).not.toHaveBeenCalled();
    expect(ipc.stop).toHaveBeenCalledOnce();
  });

  it('does not clear a newer job when an earlier session completes late', async () => {
    const earlier = deferred<typeof result>();
    const newer = deferred<typeof result>();
    ipc.export.mockReturnValueOnce(earlier.promise).mockReturnValueOnce(newer.promise);
    const { startImageExport, jobFor } = useImageExport();
    const first = startImageExport(options);
    await settle();
    disposeImageExportJobs();
    const second = startImageExport(options);
    await settle();
    const currentTaskId = jobFor(options.imageId)!.taskId;
    earlier.resolve(result);
    await first;
    expect(jobFor(options.imageId)?.taskId).toBe(currentTaskId);
    newer.resolve(result);
    await second;
    expect(jobFor(options.imageId)).toBeUndefined();
  });
});
