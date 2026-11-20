import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useEvidenceOpen } from './useEvidenceOpen';

const notifications = vi.hoisted(() => ({ error: vi.fn() }));
vi.mock('../../composables/useToast', () => ({ notifyError: notifications.error }));

function setup() {
  const options = {
    pickFiles: vi.fn(async () => [] as string[]),
    pickFolder: vi.fn(async (): Promise<string | null> => null),
    pickBcpFiles: vi.fn(async () => [] as string[]),
    openImagePath: vi.fn(async () => ({ ok: true as const })),
    openSelectedEvidence: vi.fn(async () => ({ ok: true as const })),
    openRaidMemberDialog: vi.fn(async () => {}),
    importBcp: vi.fn(),
  };
  return { options, flow: useEvidenceOpen(options) };
}

beforeEach(() => vi.clearAllMocks());

describe('evidence source opening', () => {
  it('groups loose BCP files by folder and opens ZIP packages directly', async () => {
    const { options, flow } = setup();
    options.pickBcpFiles.mockResolvedValue(['D:/a/first.bcp', 'D:/b/only.bcp', 'D:/a/last.bcp', 'D:/a/package.ZIP']);
    await flow.onAddImageSelect('bcp');
    expect(options.openImagePath.mock.calls).toEqual([
      ['D:/a', { forceImage: true, waitUntilReady: true }],
      ['D:/b', { forceImage: true, waitUntilReady: true }],
      ['D:/a/package.ZIP', { forceImage: true, waitUntilReady: true }],
    ]);
    expect(options.importBcp.mock.calls).toEqual([['D:/a'], ['D:/b'], ['D:/a/package.ZIP']]);
  });

  it('cancels the physical disk chooser without loading or reporting an error', async () => {
    const { options, flow } = setup();
    const operation = flow.onAddImageSelect('physical_disk');
    expect(flow.showPhysicalDiskPicker.value).toBe(true);
    flow.onPhysicalDiskPickerClose();
    await operation;
    expect(options.openImagePath).not.toHaveBeenCalled();
    expect(notifications.error).not.toHaveBeenCalled();
    expect(flow.showPhysicalDiskPicker.value).toBe(false);
  });

  it('forces physical disks through image loading and reports the elevation hint', async () => {
    const { options, flow } = setup();
    options.openImagePath.mockResolvedValue({ ok: false, error: '拒绝访问' } as never);
    const operation = flow.onAddImageSelect('physical_disk');
    flow.onPhysicalDiskSelect('\\\\.\\PhysicalDrive0');
    await operation;
    expect(options.openImagePath).toHaveBeenCalledWith('\\\\.\\PhysicalDrive0', { forceImage: true, quiet: true });
    expect(notifications.error).toHaveBeenCalledWith('打开物理磁盘失败', expect.stringContaining('管理员身份'));
  });

  it('keeps a synthetic-volume recovery step with the backend reason', async () => {
    const { options, flow } = setup();
    options.openImagePath.mockResolvedValue({ ok: false, error: '不存在' } as never);
    expect(await flow.onOpenImageFromInfo('decrypted://old')).toBe(false);
    expect(notifications.error).toHaveBeenCalledWith('打开镜像失败', '不存在；解密会话已关闭，请重新解锁源镜像');
  });
});
