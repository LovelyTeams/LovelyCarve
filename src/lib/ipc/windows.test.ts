// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  find: vi.fn(), invoke: vi.fn(), destroyed: vi.fn(), cleanup: vi.fn(),
}));
vi.mock('@tauri-apps/api/webviewWindow', () => ({ WebviewWindow: { getByLabel: mocks.find } }));
vi.mock('./client', () => ({ invokeCmd: mocks.invoke }));
vi.mock('./events', () => ({ onWindowDestroyed: mocks.destroyed }));
import { openFilePreviewWindow, type FilePreviewWindowArgs } from './windows';

const args: FilePreviewWindowArgs = {
  imageId: 'image', nodeId: 'node', title: '文件.txt', fullPath: '/目录/文件.txt',
  node: { id: 'node', name: '文件.txt' }, ownerId: 'owner', previewId: 'preview',
  position: { x: -240, y: 80 }, focusTarget: { offset: 1024, length: 8 },
  indexOnly: true, ident: { label: 'text' }, digest: { md5: 'digest' },
};

beforeEach(() => {
  vi.resetAllMocks();
  mocks.find.mockResolvedValue(null);
  mocks.invoke.mockResolvedValue(undefined);
  mocks.destroyed.mockResolvedValue(mocks.cleanup);
  document.documentElement.style.setProperty('--bg-primary', '#202020');
});

describe('preview window creation', () => {
  it('passes preview data to the native adapter and subscribes before creating', async () => {
    const closed = vi.fn();
    await openFilePreviewWindow(args, closed);
    expect(mocks.destroyed).toHaveBeenCalledExactlyOnceWith('file-preview-preview', closed);
    expect(mocks.destroyed.mock.invocationCallOrder[0]).toBeLessThan(mocks.invoke.mock.invocationCallOrder[0]!);
    const [command, { options }] = mocks.invoke.mock.calls[0]!;
    expect(command).toBe('plugin:preview-window|open');
    expect(options).toMatchObject({ label: 'file-preview-preview', title: args.title, position: args.position, backgroundColor: '#202020' });
    const params = new URL(options.url, 'https://tauri.localhost').searchParams;
    expect(params.get('view')).toBe('file-preview');
    expect(params.get('fullPath')).toBe(args.fullPath);
    expect(JSON.parse(params.get('node')!)).toEqual(args.node);
    expect(JSON.parse(params.get('focusTarget')!)).toEqual(args.focusTarget);
    expect(JSON.parse(params.get('ident')!)).toEqual(args.ident);
    expect(JSON.parse(params.get('digest')!)).toEqual(args.digest);
    expect(params.get('indexOnly')).toBe('true');
    expect(params.get('ownerId')).toBe('owner');
    expect(mocks.cleanup).not.toHaveBeenCalled();
  });

  it('shares concurrent opens for the same preview', async () => {
    let complete!: () => void;
    mocks.invoke.mockImplementationOnce(() => new Promise<void>(resolve => { complete = resolve; }));
    const first = openFilePreviewWindow(args);
    const second = openFilePreviewWindow(args);
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledOnce());
    complete();
    await Promise.all([first, second]);
    expect(mocks.find).toHaveBeenCalledOnce();
  });

  it('propagates creation failures, cleans up, and allows the next open', async () => {
    const error = new Error('WebView2 HRESULT(0x80010108)');
    mocks.invoke.mockRejectedValueOnce(error);
    await expect(openFilePreviewWindow(args, vi.fn())).rejects.toBe(error);
    expect(mocks.cleanup).toHaveBeenCalledOnce();
    await openFilePreviewWindow(args, vi.fn());
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
  });

  it('does not create a window if the close subscription fails', async () => {
    mocks.destroyed.mockRejectedValueOnce(new Error('listen failed'));
    await expect(openFilePreviewWindow(args, vi.fn())).rejects.toThrow('listen failed');
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it('restores and focuses an existing preview', async () => {
    const existing = { unminimize: vi.fn(), setFocus: vi.fn() };
    mocks.find.mockResolvedValueOnce(existing);
    await openFilePreviewWindow(args);
    expect(existing.unminimize).toHaveBeenCalledOnce();
    expect(existing.setFocus).toHaveBeenCalledOnce();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });
});
