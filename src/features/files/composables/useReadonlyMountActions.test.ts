import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope, ref } from 'vue';
import { useReadonlyMountActions } from './useReadonlyMountActions';
import type { DiskFsNode } from '../model/model';
const ipc = vi.hoisted(() => ({ mounts: vi.fn(), choose: vi.fn(), mount: vi.fn() }));
vi.mock('../../../lib/ipc/mount', () => ({ fsMounts: ipc.mounts, fsSuggestMountPoint: vi.fn(), fsCreateMountDir: vi.fn(), fsMountReadonly: ipc.mount, fsUnmountReadonly: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: ipc.choose }));
vi.mock('../../../composables/useToast', () => ({ notifyError: vi.fn(), pushPersistentToast: () => ({ update: vi.fn(), dismiss: vi.fn() }) }));
let scope: ReturnType<typeof effectScope>;
beforeEach(() => { vi.clearAllMocks(); ipc.mounts.mockResolvedValue([]); });
afterEach(() => scope?.stop());
it('does not mount an old Docker root after an A to B to A chooser round trip', async () => {
  const image = ref('A'); const node = { id: 'host-docker:container:', name: 'Container', kind: 'dir' } as DiskFsNode;
  let resolve!: (value: string) => void; ipc.choose.mockReturnValue(new Promise(yes => { resolve = yes; }));
  scope = effectScope(); const actions = scope.run(() => useReadonlyMountActions({ supported: true, imageId: () => image.value, node: () => node,
    target: () => 'folder', folderRoot: () => '', caseDirectory: () => null, closeMenu: () => {} }))!;
  const mounting = actions.onToggleRowMount(); for (let index = 0; index < 5; index++) await Promise.resolve();
  expect(ipc.choose).toHaveBeenCalledOnce(); image.value = 'B'; image.value = 'A'; resolve('C:/empty'); await mounting;
  expect(ipc.mount).not.toHaveBeenCalled(); expect(actions.mountBusy.value).toBe(false);
});
