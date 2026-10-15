// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, nextTick, ref, type App } from 'vue';
import { initTabManagerHooks, __resetTabManagerForTesting } from './useTabManager';

const {
  caseCreateMock,
  caseOpenMock,
  caseCloseMock,
  caseActiveMock,
  caseEvidenceRemoveMock,
  snapshotListMock,
  snapshotBuildMock,
  snapshotDropMock,
  onEventMock,
  taskCancelMock,
  systemPathInfoMock,
  getCurrentWebviewWindowMock,
  onDragDropEventMock,
  unlistenDragDropMock,
  notifyInfoMock,
  maybeAutoMountCaseMock,
} = vi.hoisted(() => ({
  caseCreateMock: vi.fn(),
  caseOpenMock: vi.fn(),
  caseCloseMock: vi.fn(),
  caseActiveMock: vi.fn(),
  caseEvidenceRemoveMock: vi.fn(),
  snapshotListMock: vi.fn(),
  snapshotBuildMock: vi.fn(),
  snapshotDropMock: vi.fn(),
  onEventMock: vi.fn(),
  taskCancelMock: vi.fn(),
  systemPathInfoMock: vi.fn(),
  getCurrentWebviewWindowMock: vi.fn(),
  onDragDropEventMock: vi.fn(),
  unlistenDragDropMock: vi.fn(),
  notifyInfoMock: vi.fn(),
  maybeAutoMountCaseMock: vi.fn(),
}));

vi.mock('../../lib/ipc/cases', () => ({
  SNAPSHOT_PROGRESS_EVENT: 'snapshot-build-progress',
  autoSnapshotTaskId: (imageId: string, volumeKey: string) => `snapshot:${imageId}:${volumeKey}`,
  caseCreate: caseCreateMock,
  caseOpen: caseOpenMock,
  caseClose: caseCloseMock,
  caseActive: caseActiveMock,
  caseEvidenceRemove: caseEvidenceRemoveMock,
  caseList: vi.fn(async () => []),
  caseRemove: vi.fn(async () => undefined),
  caseLock: vi.fn(async () => null),
  caseUnlock: vi.fn(async () => null),
  caseRename: vi.fn(async () => null),
  snapshotList: snapshotListMock,
  snapshotBuild: snapshotBuildMock,
  snapshotDrop: snapshotDropMock,
}));

vi.mock('../../lib/ipc/events', () => ({
  onEvent: onEventMock,
}));

vi.mock('../../lib/ipc/taskRuntime', () => ({ taskCancel: taskCancelMock }));

vi.mock('../../lib/ipc/system', () => ({
  systemPathInfo: systemPathInfoMock,
}));

vi.mock('@tauri-apps/api/webviewWindow', () => ({
  getCurrentWebviewWindow: getCurrentWebviewWindowMock,
}));

vi.mock('../../features/cases/composables/useRecentCases', () => ({
  useRecentCases: () => ({
    recentCases: ref([]),
    recordCase: vi.fn(),
    removeCase: vi.fn(),
  }),
}));

vi.mock('../../features/evidence/composables/useAutoMount', () => ({
  useAutoMount: () => ({
    maybeAutoMountCase: maybeAutoMountCaseMock,
    autoMounting: ref(false),
  }),
}));

vi.mock('../../composables/useToast', () => ({
  notifyInfo: notifyInfoMock,
  notifyError: vi.fn(),
}));

import { useCaseSession } from '../../features/cases/composables/useCaseSession';
import { useWorkspaceShell } from './useWorkspaceShell';
import { notifyError } from '../../composables/useToast';

type DragDropEventHandler = (event: { payload: { type: 'enter' | 'over' | 'drop' | 'leave'; paths?: string[] } }) => void | Promise<void>;

let dragDropHandler: DragDropEventHandler | null = null;

function makeCase(caseId = 'case-1') {
  return {
    id: caseId,
    name: 'Case One',
    dir: 'D:/cases/one',
    examiner: 'examiner',
    created_at: '2026-07-02T00:00:00Z',
    updated_at: '2026-07-02T00:00:00Z',
    evidence: [],
    snapshots: [],
  };
}

async function flushPendingResume(): Promise<void> {
  await nextTick();
  await Promise.resolve();
  await nextTick();
}

async function flushMountedDragSetup(): Promise<void> {
  await nextTick();
  await Promise.resolve();
  await vi.dynamicImportSettled();
  await nextTick();
}

describe('useWorkspaceShell case gate', () => {
  beforeEach(async () => {
    __resetTabManagerForTesting();
    dragDropHandler = null;
    vi.clearAllMocks();
    caseCloseMock.mockResolvedValue(undefined);
    snapshotListMock.mockResolvedValue([]);
    onEventMock.mockResolvedValue(() => undefined);
    getCurrentWebviewWindowMock.mockReturnValue({
      onDragDropEvent: onDragDropEventMock.mockImplementation(async (handler: DragDropEventHandler) => {
        dragDropHandler = handler;
        return unlistenDragDropMock;
      }),
    });
    await useCaseSession().closeCase();
    __resetTabManagerForTesting();
    dragDropHandler = null;
    vi.clearAllMocks();
    caseCloseMock.mockResolvedValue(undefined);
    snapshotListMock.mockResolvedValue([]);
    onEventMock.mockResolvedValue(() => undefined);
    getCurrentWebviewWindowMock.mockReturnValue({
      onDragDropEvent: onDragDropEventMock.mockImplementation(async (handler: DragDropEventHandler) => {
        dragDropHandler = handler;
        return unlistenDragDropMock;
      }),
    });
  });

  it('resumes the add-evidence chooser after a case is created', async () => {
    const onNeedCase = vi.fn();
    const onRequestAddImage = vi.fn();
    const onCaseGateSatisfied = vi.fn();
    const shell = useWorkspaceShell({
      isEvidenceMode: ref(false),
      openByPath: vi.fn(),
      onNeedCase,
      onRequestAddImage,
      onCaseGateSatisfied,
    });

    await shell.handleOpenFile();

    expect(onNeedCase).toHaveBeenCalledTimes(1);
    expect(onRequestAddImage).not.toHaveBeenCalled();
    expect(notifyInfoMock).toHaveBeenCalledWith(
      '请先创建或打开案件',
      '证据镜像会登记到当前案件，并用于卷快照、标签和哈希结果持久化。',
    );

    caseCreateMock.mockResolvedValue(makeCase());
    await useCaseSession().createCase('D:/cases/one', 'Case One');
    await flushPendingResume();

    expect(onCaseGateSatisfied).toHaveBeenCalledTimes(1);
    expect(onRequestAddImage).toHaveBeenCalledTimes(1);
  });

  it('does not resume a cancelled pending chooser after the case later changes', async () => {
    const onNeedCase = vi.fn();
    const onRequestAddImage = vi.fn();
    const onCaseGateSatisfied = vi.fn();
    const shell = useWorkspaceShell({
      isEvidenceMode: ref(false),
      openByPath: vi.fn(),
      onNeedCase,
      onRequestAddImage,
      onCaseGateSatisfied,
    });

    await shell.handleOpenFile();
    shell.clearPendingOpenRequest();

    caseCreateMock.mockResolvedValue(makeCase('case-2'));
    await useCaseSession().createCase('D:/cases/two', 'Case Two');
    await flushPendingResume();

    expect(onNeedCase).toHaveBeenCalledTimes(1);
    expect(onCaseGateSatisfied).not.toHaveBeenCalled();
    expect(onRequestAddImage).not.toHaveBeenCalled();
  });

  it('opens every dropped path and forces image loading for dropped folders', async () => {
    const openByPath = vi.fn().mockResolvedValue({ opened: true, nextPanel: 'dashboard' });
    caseCreateMock.mockResolvedValue(makeCase('case-drag'));
    caseActiveMock.mockResolvedValueOnce(makeCase('case-drag')).mockResolvedValueOnce(makeCase('case-drag'));
    await useCaseSession().createCase('D:/cases/drag', 'Drag Case');

    let shell: ReturnType<typeof useWorkspaceShell> | null = null;
    const host = document.createElement('div');
    document.body.appendChild(host);
    const app: App = createApp(defineComponent({
      setup() {
        shell = useWorkspaceShell({
          isEvidenceMode: ref(false),
          openByPath,
          onNeedCase: vi.fn(),
          onRequestAddImage: vi.fn(),
        });
        return () => null;
      },
    }));

    try {
      app.mount(host);
      await flushMountedDragSetup();
      expect(dragDropHandler).toBeTruthy();
      if (!shell || !dragDropHandler) throw new Error('workspace shell did not mount drag handler');
      initTabManagerHooks({
        onLoadFile: shell.loadFileForTab,
        onTeardown: () => {},
        getActivePanel: () => shell!.activePanel.value,
        setActivePanel: panel => { shell!.activePanel.value = panel; },
        isBusy: () => false,
        captureSession: () => ({}),
        restoreSession: () => {},
      });
      systemPathInfoMock.mockImplementation(async (path: string) => ({
        exists: true,
        is_file: !path.endsWith('/folder'),
        is_directory: path.endsWith('/folder'),
        is_symlink: false,
      }));

      await dragDropHandler({
        payload: {
          type: 'drop',
          paths: ['D:/evidence/disk.E01', 'D:/evidence/folder'],
        },
      });

      expect(openByPath).toHaveBeenCalledTimes(2);
      expect(openByPath).toHaveBeenNthCalledWith(1, 'D:/evidence/disk.E01', { forceImage: false, caseDir: 'D:/cases/one' });
      expect(openByPath).toHaveBeenNthCalledWith(2, 'D:/evidence/folder', { forceImage: true, caseDir: 'D:/cases/one' });
      expect(systemPathInfoMock).toHaveBeenCalledTimes(2);
    } finally {
      app.unmount();
      host.remove();
    }
  });

  it('lets the RAID workflow consume native drops without opening tabs or invoking the case gate', async () => {
    const onDroppedPaths = vi.fn(async () => true);
    const onNeedCase = vi.fn();
    const openByPath = vi.fn();
    let shell!: ReturnType<typeof useWorkspaceShell>;
    const host = document.createElement('div');
    const app = createApp(defineComponent({ setup() {
      shell = useWorkspaceShell({ isEvidenceMode: ref(false), openByPath,
        onNeedCase, onRequestAddImage: vi.fn(), onDroppedPaths });
      return () => null;
    } }));
    try {
      app.mount(host);
      await flushMountedDragSetup();
      if (!dragDropHandler) throw new Error('missing native drag subscription');
      await dragDropHandler({ payload: { type: 'enter', paths: ['D:/sdb.E01'] } });
      expect(shell.isDragOver.value).toBe(true);
      await dragDropHandler({ payload: { type: 'drop', paths: ['D:/sdb.E01', 'D:/sdc.E01'] } });
      expect(onDroppedPaths).toHaveBeenCalledExactlyOnceWith(['D:/sdb.E01', 'D:/sdc.E01']);
      expect(shell.isDragOver.value).toBe(false);
      expect(onNeedCase).not.toHaveBeenCalled();
      expect(systemPathInfoMock).not.toHaveBeenCalled();
      expect(openByPath).not.toHaveBeenCalled();
    } finally { app.unmount(); host.remove(); }
  });

  it('opens the selected case and restores its unique evidence paths', async () => {
    const onOpenCaseEvidence = vi.fn().mockResolvedValue(undefined);
    const shell = useWorkspaceShell({
      isEvidenceMode: ref(false), openByPath: vi.fn(),
      onNeedCase: vi.fn(), onRequestAddImage: vi.fn(), onOpenCaseEvidence,
    });
    caseOpenMock.mockResolvedValue({
      ...makeCase('case-recent'),
      evidence: [{ path: 'D:/evidence/folder' }, { path: 'assembled://raid' }, { path: 'D:/evidence/folder' }],
    });
    await shell.handleOpenRecentCase({ dir: 'D:/cases/recent', name: 'Recent Case', openedAt: '2026-09-19T00:00:00Z' });
    expect(caseOpenMock).toHaveBeenCalledExactlyOnceWith({ dir: 'D:/cases/recent' });
    expect(onOpenCaseEvidence).toHaveBeenCalledExactlyOnceWith(['D:/evidence/folder', 'assembled://raid']);
    expect(shell.isOpeningRecentCase.value).toBe(false);
  });

  it('shows the case manager for an empty case and ignores repeated clicks while opening', async () => {
    const onNeedCase = vi.fn();
    const shell = useWorkspaceShell({
      isEvidenceMode: ref(false), openByPath: vi.fn(),
      onNeedCase, onRequestAddImage: vi.fn(),
    });
    caseOpenMock.mockResolvedValue(makeCase('case-empty'));
    const entry = { dir: 'D:/cases/empty', name: 'Empty Case', openedAt: '2026-09-19T00:00:00Z' };
    const opening = shell.handleOpenRecentCase(entry);
    await shell.handleOpenRecentCase(entry);
    await opening;
    expect(caseOpenMock).toHaveBeenCalledOnce();
    expect(onNeedCase).toHaveBeenCalledOnce();
    expect(shell.isOpeningRecentCase.value).toBe(false);
  });

  it('reports a missing case and does not attempt evidence restoration', async () => {
    const onOpenCaseEvidence = vi.fn();
    const shell = useWorkspaceShell({
      isEvidenceMode: ref(false), openByPath: vi.fn(),
      onNeedCase: vi.fn(), onRequestAddImage: vi.fn(), onOpenCaseEvidence,
    });
    caseOpenMock.mockRejectedValue(new Error('案件目录不存在'));
    await shell.handleOpenRecentCase({ dir: 'D:/missing', name: 'Missing Case', openedAt: '2026-09-19T00:00:00Z' });
    expect(onOpenCaseEvidence).not.toHaveBeenCalled();
    expect(notifyError).toHaveBeenCalledWith('打开案件失败', expect.stringContaining('案件目录不存在'));
    expect(shell.isOpeningRecentCase.value).toBe(false);
  });

  it('reports a failed tab load with the backend reason unless the caller is quiet', async () => {
    const openByPath = vi.fn().mockRejectedValue(new Error('镜像头损坏'));
    const shell = useWorkspaceShell({
      isEvidenceMode: ref(false), openByPath,
      onNeedCase: vi.fn(), onRequestAddImage: vi.fn(),
    });

    expect(await shell.loadFileForTab('D:\\evidence\\disk.E01')).toEqual({ opened: false, error: '镜像头损坏' });
    expect(notifyError).toHaveBeenCalledExactlyOnceWith('打开证据失败', 'disk.E01：镜像头损坏');

    vi.mocked(notifyError).mockClear();
    openByPath.mockResolvedValue({ opened: false, error: '暂不支持该文件类型' });
    expect(await shell.loadFileForTab('D:/evidence/notes.txt', { quiet: true })).toEqual({ opened: false, error: '暂不支持该文件类型' });
    expect(notifyError).not.toHaveBeenCalled();
  });

  it('re-syncs the case when a load registers evidence it does not list yet', async () => {
    caseCreateMock.mockResolvedValue(makeCase('case-sync'));
    await useCaseSession().createCase('D:/cases/sync', 'Sync Case');
    const synced = { ...makeCase('case-sync'), evidence: [{ image_id: 'img-new', path: 'D:/evidence/new.vmdk' }] };
    caseActiveMock.mockResolvedValueOnce(synced);
    const shell = useWorkspaceShell({
      isEvidenceMode: ref(false),
      openByPath: vi.fn().mockResolvedValue({ opened: true, mode: 'image', nextPanel: 'dashboard', imageId: 'img-new' }),
      onNeedCase: vi.fn(), onRequestAddImage: vi.fn(),
    });

    await shell.loadFileForTab('D:/evidence/new.vmdk');
    expect(caseActiveMock).toHaveBeenCalledOnce();
    expect(useCaseSession().activeCase.value).toEqual(synced);

    // Evidence the case already lists (case reopen, evicted tab reload) needs no round trip.
    await shell.loadFileForTab('D:/evidence/new.vmdk');
    expect(caseActiveMock).toHaveBeenCalledOnce();
  });

  it('keeps a loaded image open when the case re-sync fails', async () => {
    caseCreateMock.mockResolvedValue(makeCase('case-offline'));
    await useCaseSession().createCase('D:/cases/offline', 'Offline Case');
    caseActiveMock.mockRejectedValueOnce(new Error('case database busy'));
    const loaded = { opened: true, mode: 'image', nextPanel: 'dashboard', imageId: 'img-new' };
    const shell = useWorkspaceShell({
      isEvidenceMode: ref(false), openByPath: vi.fn().mockResolvedValue(loaded),
      onNeedCase: vi.fn(), onRequestAddImage: vi.fn(),
    });

    expect(await shell.loadFileForTab('D:/evidence/new.vmdk')).toEqual(loaded);
    expect(shell.activePanel.value).toBe('dashboard');
    expect(notifyError).not.toHaveBeenCalled();
  });
});
