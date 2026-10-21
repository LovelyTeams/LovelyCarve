import { beforeEach, describe, expect, it, vi } from 'vitest';

const { openMock, invokeMock, listenMock } = vi.hoisted(() => ({
  openMock: vi.fn(),
  invokeMock: vi.fn(),
  listenMock: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: openMock,
}));

vi.mock('../../../lib/ipc/client', () => ({
  invokeCmd: invokeMock,
}));

vi.mock('../../../lib/ipc/events', () => ({
  onEvent: listenMock,
}));

import { DISK_IMAGE_EXTENSIONS, isDiskImageExtension, useFileSession } from './useFileSession';
import { rememberUnlockedImageInfo, clearUnlockedImageInfo } from './unlockedImageInfo';
import type { ImageInfo } from '../../../lib/ipc/image';

it('opens an indexed offline image directly in the file list with the saved identity', async () => {
  listenMock.mockResolvedValue(vi.fn());
  invokeMock.mockResolvedValue({ id: 'saved-id', source_format: 'ewf', partitions: [], index_only: true });
  const session = useFileSession();
  expect(await session.openByPath('missing.E01', { forceImage: true })).toEqual({ opened: true, mode: 'image', nextPanel: 'files', imageId: 'saved-id' });
  expect(session.imageInfo.value?.index_only).toBe(true); expect(session.parserName.value).toBe('离线索引（ewf）');
});

beforeEach(() => {
  openMock.mockReset();
  invokeMock.mockReset();
  listenMock.mockReset();
  clearUnlockedImageInfo();
});

describe('useFileSession disk image extension handling', () => {
  it.each(['disc.cue', 'DISC.CUE'])('loads CUE %s through image_load', async fileName => {
    listenMock.mockResolvedValue(vi.fn());
    invokeMock.mockResolvedValue({ id: 'disc', source_format: 'raw', partitions: [], optical_disc: { files: [], tracks: [], lead_outs: [] } });
    const session = useFileSession();
    expect(await session.openByPath(fileName)).toMatchObject({ opened: true, mode: 'image', imageId: 'disc' });
    expect(invokeMock).toHaveBeenCalledWith('image_load', expect.objectContaining({ path: fileName }));
    expect(session.imageInfo.value?.optical_disc?.tracks).toEqual([]);
  });
  it.each(['phone.ab', 'app.bak'])('loads Android backup %s through image_load', async fileName => {
    listenMock.mockResolvedValue(vi.fn());
    invokeMock.mockResolvedValue({ id: 'phone', source_format: 'android_backup', partitions: [] });
    const session = useFileSession();
    expect(await session.openByPath(fileName)).toMatchObject({ opened: true, mode: 'image', imageId: 'phone' });
    expect(invokeMock).toHaveBeenCalledWith('image_load', expect.objectContaining({ path: fileName }));
  });
  it('routes ISO images through the disk-image loader', () => {
    expect(DISK_IMAGE_EXTENSIONS).toContain('iso');
    expect(isDiskImageExtension('iso')).toBe(true);
    expect(isDiskImageExtension('ISO')).toBe(true);
  });

  it('loads ISO paths through the image_load IPC command', async () => {
    invokeMock.mockResolvedValue({
      id: 'image-iso',
      path: 'D:/evidence/test.iso',
      source_format: 'raw',
      size_bytes: 815104,
      sector_size: 512,
      partitions: [
        {
          id: 0,
          kind: 'cdfs',
          start_lba: 0,
          length_lba: 1592,
          label: null,
        },
      ],
    });

    const session = useFileSession();
    const outcome = await session.openByPath('D:/evidence/test.iso');

    expect(outcome).toEqual({ opened: true, mode: 'image', nextPanel: 'dashboard', imageId: 'image-iso' });
    expect(invokeMock).toHaveBeenCalledWith('image_load', { path: 'D:/evidence/test.iso' });
    expect(invokeMock).not.toHaveBeenCalledWith('open_and_parse', expect.anything());
    expect(session.imageInfo.value?.partitions[0].kind).toBe('cdfs');
  });
});

describe('file session request ownership', () => {
  it('restores inactive tabs with unlocked partition metadata without loading a new image', async () => {
    listenMock.mockResolvedValue(vi.fn());
    const original = { id: 'source', path: 'server.E01', source_format: 'ewf', partitions: [{ id: 2, kind: 'luks' }] } as ImageInfo;
    invokeMock.mockResolvedValue(original);
    const session = useFileSession();
    await session.openByPath('server.E01');
    const snapshot = session.captureSnapshot();
    session.resetSession();
    const updated = { ...original, unlocked_partitions: [2], partitions: [{ ...original.partitions[0], kind: 'ext4' }] };
    rememberUnlockedImageInfo(updated);
    session.restoreSnapshot(snapshot);
    expect(session.imageInfo.value).toBe(updated);
    expect(invokeMock).toHaveBeenCalledOnce();
    expect(session.imageId.value).toBe('source');
  });

  it('does not start a load after reset while the listener is attaching', async () => {
    let resolve!: (stop: () => void) => void; const stop = vi.fn();
    listenMock.mockReturnValue(new Promise(yes => { resolve = yes; }));
    const session = useFileSession(); const opening = session.openByPath('old.img');
    session.resetSession(); resolve(stop); expect(await opening).toEqual({ opened: false });
    expect(stop).toHaveBeenCalledOnce(); expect(invokeMock).not.toHaveBeenCalled();
  });
  it('does not publish old image bytes into a replacement session', async () => {
    let resolve!: (value: unknown) => void;
    listenMock.mockResolvedValue(vi.fn()); invokeMock.mockReturnValue(new Promise(yes => { resolve = yes; }));
    const session = useFileSession(); const opening = session.openByPath('old.img');
    await Promise.resolve(); await Promise.resolve(); session.resetSession();
    resolve({ id: 'old', source_format: 'raw', partitions: [] });
    expect(await opening).toEqual({ opened: false }); expect(session.imageInfo.value).toBeNull(); expect(session.loading.value).toBe(false);
  });
});
