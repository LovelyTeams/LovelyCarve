import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { startImageHash, useImageHashJob, disposeImageHashJobs } from './useImageHash';
import { startBitlockerMemoryScan, useBitlockerScanJob, disposeBitlockerMemoryScans } from './useBitlockerMemoryScan';
const ipc = vi.hoisted(() => ({ listen: vi.fn(), hash: vi.fn(), scan: vi.fn() }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: ipc.listen }));
vi.mock('../../../lib/ipc/image', () => ({ imageHashCompute: ipc.hash }));
vi.mock('../../../lib/ipc/encrypted', () => ({ bitlockerScanMemory: ipc.scan, BITLOCKER_SCAN_PROGRESS_EVENT: 'bitlocker-scan-progress' }));
vi.mock('../../../composables/useToast', () => ({ notifyError: vi.fn(), notifySuccess: vi.fn() }));
beforeEach(() => { vi.clearAllMocks(); ipc.listen.mockResolvedValue(vi.fn()); });
afterEach(() => { disposeImageHashJobs(); disposeBitlockerMemoryScans(); });
describe('evidence job subscription ownership', () => {
  it('removes a failed hash listener claim so retry can start', async () => {
    ipc.listen.mockRejectedValueOnce(new Error('bridge'));
    expect(await startImageHash('image', ['md5'])).toBe(false); expect(useImageHashJob(() => 'image').value).toBeUndefined();
    ipc.hash.mockResolvedValue({md5:'digest',sha1:null,sha256:null,sm3:null,bad_regions:[],synthetic_bytes:0,read_faults_detected:false});
    expect(await startImageHash('image', ['md5'])).toBe(true); expect(ipc.hash).toHaveBeenCalledOnce();
  });
  it('removes a failed memory-scan listener claim instead of leaving the partition busy', async () => {
    ipc.listen.mockRejectedValueOnce(new Error('bridge'));
    expect(await startBitlockerMemoryScan('image', 1, 'D:/memory.raw')).toBeNull();
    expect(useBitlockerScanJob(() => 'image', () => 1).value).toBeUndefined();
    ipc.scan.mockResolvedValue({ partition_id: 1, candidates: [] });
    expect(await startBitlockerMemoryScan('image', 1, 'D:/memory.raw')).toEqual({ partition_id: 1, candidates: [] });
  });
  it('releases a hash listener attaching after main-window disposal', async () => {
    let resolve!: (stop: () => void) => void; const stop = vi.fn(); ipc.listen.mockReturnValueOnce(new Promise(yes => { resolve = yes; }));
    const running = startImageHash('image', ['md5']); disposeImageHashJobs(); resolve(stop);
    expect(await running).toBe(false); expect(stop).toHaveBeenCalledOnce(); expect(ipc.hash).not.toHaveBeenCalled();
  });
  it('releases a memory-scan listener attaching after main-window disposal', async () => {
    let resolve!: (stop: () => void) => void; const stop = vi.fn(); ipc.listen.mockReturnValueOnce(new Promise(yes => { resolve = yes; }));
    const running = startBitlockerMemoryScan('image', 1, 'D:/memory.raw'); disposeBitlockerMemoryScans(); resolve(stop);
    expect(await running).toBeNull(); expect(stop).toHaveBeenCalledOnce(); expect(ipc.scan).not.toHaveBeenCalled();
  });
});
