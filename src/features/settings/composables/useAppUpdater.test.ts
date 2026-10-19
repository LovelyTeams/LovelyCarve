// @vitest-environment happy-dom
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ check: vi.fn(), install: vi.fn(), listen: vi.fn(), unlisten: vi.fn() }));
vi.mock('../../../lib/ipc/system', () => ({ releaseUpdateCheck: mocks.check, releaseUpdateInstall: mocks.install }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: mocks.listen }));

const newer = { available: true, version: '1.2.0', signed: false, canInstall: true };

beforeEach(() => {
  vi.resetModules();
  vi.clearAllMocks();
  Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true });
  mocks.check.mockResolvedValue(newer);
  mocks.install.mockResolvedValue(undefined);
  mocks.listen.mockResolvedValue(mocks.unlisten);
});
afterEach(() => { Reflect.deleteProperty(window, '__TAURI_INTERNALS__'); });

describe('GitHub app updates', () => {
  it('recognizes the current legacy release without requesting missing latest.json', async () => {
    mocks.check.mockResolvedValue({ ...newer, available: false, version: '1.1.1' });
    const updater = (await import('./useAppUpdater')).useAppUpdater();
    await updater.checkForUpdates();
    expect(updater.status.value).toBe('up-to-date');
    expect(mocks.install).not.toHaveBeenCalled();
  });

  it('coalesces startup and manual checks and allows subsequent checks', async () => {
    let finish!: (value: typeof newer) => void;
    mocks.check.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    const updater = (await import('./useAppUpdater')).useAppUpdater();
    const first = updater.checkForUpdates();
    const second = updater.checkForUpdates();
    expect(mocks.check).toHaveBeenCalledTimes(1);
    finish(newer);
    await Promise.all([first, second]);
    await updater.checkForUpdates();
    expect(mocks.check).toHaveBeenCalledTimes(2);
    expect(updater.hasUpdate.value).toBe(true);
  });

  it('uses the backend for signed installation and permits later checks', async () => {
    mocks.check.mockResolvedValue({ ...newer, signed: true });
    const updater = (await import('./useAppUpdater')).useAppUpdater();
    await updater.checkForUpdates();
    await updater.checkForUpdates();
    expect(mocks.check).toHaveBeenCalledTimes(2);
    await updater.installUpdate();
    expect(mocks.install).toHaveBeenCalledWith('1.2.0');
    expect(mocks.unlisten).toHaveBeenCalledTimes(1);
  });

  it('does not downgrade to the legacy installer after a signed channel failure', async () => {
    mocks.check.mockResolvedValue({ ...newer, signed: true });
    mocks.install.mockRejectedValueOnce(new Error('signature metadata invalid'));
    const updater = (await import('./useAppUpdater')).useAppUpdater();
    await updater.checkForUpdates();
    await updater.installUpdate();
    expect(updater.status.value).toBe('error');
    expect(mocks.install).toHaveBeenCalledTimes(1);
    expect(mocks.unlisten).toHaveBeenCalledTimes(1);
  });

  it('reports an update without a compatible installer instead of claiming it is current', async () => {
    mocks.check.mockResolvedValue({ ...newer, canInstall: false });
    const updater = (await import('./useAppUpdater')).useAppUpdater();
    await updater.checkForUpdates();
    expect(updater.status.value).toBe('unsupported');
    expect(updater.updateVersion.value).toBe('1.2.0');
  });

  it('recovers from a failed request on retry', async () => {
    mocks.check.mockRejectedValueOnce(new Error('network offline'));
    const updater = (await import('./useAppUpdater')).useAppUpdater();
    await updater.checkForUpdates();
    expect(updater.status.value).toBe('error');
    await updater.checkForUpdates();
    expect(updater.hasUpdate.value).toBe(true);
    expect(updater.error.value).toBe('');
  });

  it('shows progress, prevents overlapping checks and installs, and unsubscribes on failure', async () => {
    let progress!: (event: { payload: { version: string; downloaded: number; total: number; installing: boolean } }) => void;
    mocks.listen.mockImplementation(async (_name, handler) => { progress = handler; return mocks.unlisten; });
    let fail!: (reason: Error) => void;
    mocks.install.mockReturnValueOnce(new Promise((_resolve, reject) => { fail = reject; }));
    const updater = (await import('./useAppUpdater')).useAppUpdater();
    await updater.checkForUpdates();
    const installing = updater.installUpdate();
    await vi.waitFor(() => expect(mocks.install).toHaveBeenCalledWith('1.2.0'));
    await updater.checkForUpdates();
    await updater.installUpdate();
    expect(mocks.check).toHaveBeenCalledTimes(1);
    expect(mocks.install).toHaveBeenCalledTimes(1);
    progress({ payload: { version: '1.2.0', downloaded: 25, total: 100, installing: false } });
    expect(updater.downloadProgress.value).toBe(25);
    fail(new Error('checksum mismatch'));
    await installing;
    expect(updater.status.value).toBe('error');
    expect(mocks.unlisten).toHaveBeenCalledTimes(1);
  });
});
