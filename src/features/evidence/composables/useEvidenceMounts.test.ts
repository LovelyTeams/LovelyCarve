// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, ref, shallowRef, type App } from 'vue';
import type { ImageInfo } from '../../../lib/ipc/image';
import type { DokanMountView } from '../../../lib/ipc/mount';
import { useEvidenceMounts } from './useEvidenceMounts';

const ipc = vi.hoisted(() => ({ mounts: vi.fn(), drives: vi.fn(), create: vi.fn(), mount: vi.fn(), unmount: vi.fn(), share: vi.fn(), unshare: vi.fn(), open: vi.fn() }));
vi.mock('../../../lib/ipc/mount', () => ({ fsMounts: ipc.mounts, fsAvailableMountPoints: ipc.drives, fsCreateMountDir: ipc.create, fsMountReadonly: ipc.mount, fsUnmountReadonly: ipc.unmount, fsShareMount: ipc.share, fsUnshareMount: ipc.unshare }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: ipc.open }));
vi.mock('../../../composables/useToast', () => ({ notifySuccess: vi.fn(), notifyError: vi.fn() }));

function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
function image(id: string): ImageInfo { return { id, path: id, source_format: 'raw', size_bytes: 1024, sector_size: 512, table_scheme: 'gpt', partitions: [{ id: 1, kind: 'bitlocker', label: null, start_lba: 0, length_lba: 2 }], disk_id: null, on_disk_bytes: null, modified_unix_ms: null, created_unix_ms: null, ewf: null }; }
function view(image_id: string): DokanMountView { return { id: `${image_id}-mount`, image_id, root_node_id: 'part:1', mount_point: 'H:\\', share: null } as DokanMountView; }
let app: App; let host: HTMLElement;
async function settle() { for (let index = 0; index < 5; index++) await nextTick(); }
async function setup(target: 'drive' | 'folder' = 'folder') {
  const info = shallowRef(image('a')); const caseDirectory = ref<string | null>(null); const folder = ref('');
  let controller!: ReturnType<typeof useEvidenceMounts>;
  host = document.createElement('div'); document.body.append(host);
  app = createApp(defineComponent({ setup() { controller = useEvidenceMounts({ info: () => info.value, mountTarget: () => target, mountFolderRoot: () => folder.value, caseDirectory: () => caseDirectory.value, onUnlocked: vi.fn() }); return () => h('div'); } }));
  app.mount(host); await settle(); return { controller, info, caseDirectory, folder };
}
beforeEach(() => { vi.resetAllMocks(); ipc.mounts.mockResolvedValue([]); });
afterEach(() => { app?.unmount(); host?.remove(); });

describe('evidence mount context ownership', () => {
  it('does not mount an old node on a new image after a folder dialog returns', async () => {
    const picked = deferred<string>(); ipc.open.mockReturnValue(picked.promise);
    const { controller, info } = await setup(); const pending = controller.beginMount('part:1', 1, '分区1');
    info.value = image('b'); picked.resolve('D:/mount'); await pending;
    expect(ipc.mount).not.toHaveBeenCalled();
  });
  it('captures the case root and abandons directory creation after a case switch', async () => {
    const directory = deferred<string>(); ipc.create.mockReturnValue(directory.promise);
    const { controller, caseDirectory } = await setup(); caseDirectory.value = 'D:/case-a';
    const pending = controller.beginMount('part:1', 1, '分区1');
    expect(ipc.create).toHaveBeenCalledWith({ root: 'D:/case-a', imageId: 'a', name: '分区1' });
    caseDirectory.value = 'D:/case-b'; directory.resolve('D:/case-a/mount'); await pending;
    expect(ipc.mount).not.toHaveBeenCalled(); expect(controller.busyPartitions.value.size).toBe(0);
  });
  it('does not let an old mount completion clear the new image busy state', async () => {
    const first = deferred<DokanMountView>(); const second = deferred<DokanMountView>();
    ipc.mount.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const { controller, info } = await setup();
    const pendingA = controller.finishMountAt('part:1', 1, 'A', 'H:\\'); info.value = image('b');
    const pendingB = controller.finishMountAt('part:1', 1, 'B', 'I:\\'); first.resolve(view('a')); await pendingA;
    expect(controller.busyPartitions.value.has(1)).toBe(true);
    expect(ipc.mount.mock.calls.map(([args]) => args.imageId)).toEqual(['a', 'b']);
    second.resolve(view('b')); await pendingB; expect(controller.busyPartitions.value.size).toBe(0);
  });
  it('does not publish a late drive picker after disposal', async () => {
    const drives = deferred<string[]>(); ipc.drives.mockReturnValue(drives.promise);
    const { controller } = await setup('drive'); const pending = controller.beginMount('part:1', 1, '分区1');
    app.unmount(); drives.resolve(['H:\\']); await pending;
    expect(controller.availableDrives.value).toEqual([]); expect(controller.pickerOpenFor.value).toBeNull();
  });
  it('uses the synthetic node identity and preserves the existing mount/unmount commands', async () => {
    const { controller, info, folder } = await setup(); info.value = { ...image('a'), partitions: [], source_format: 'docker' }; folder.value = 'D:/root';
    ipc.create.mockResolvedValue('D:/root/container'); ipc.mount.mockResolvedValue({ ...view('a'), root_node_id: 'docker:' });
    ipc.mounts.mockResolvedValue([{ ...view('a'), root_node_id: 'docker:' }]);
    expect(controller.syntheticRoot.value).toEqual({ id: 'docker:', label: '容器根' });
    await controller.beginMount('docker:', -1, '容器根');
    expect(ipc.mount).toHaveBeenCalledWith({ imageId: 'a', nodeId: 'docker:', mountPoint: 'D:/root/container' });
    await controller.beginMount('docker:', -1, '容器根'); expect(ipc.unmount).toHaveBeenCalledWith({ mountId: 'a-mount' });
  });
});
