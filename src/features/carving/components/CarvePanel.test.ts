// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, ref, type App } from 'vue';
import CarvePanel from './CarvePanel.vue';
import type { RecoveryScan } from '../../../lib/ipc/carve';
import type { FsNode } from '../../../lib/ipc/fs/types';
import type { ImageInfo } from '../../../lib/ipc/image';
const ipc = vi.hoisted(() => ({ list: vi.fn(), remove: vi.fn(), confirm: vi.fn(), progress: vi.fn(), start: vi.fn(), status: vi.fn(), page: vi.fn(), pause: vi.fn(), resume: vi.fn(), recover: vi.fn(), open: vi.fn(), filesystem: vi.fn() }));
vi.mock('../../tasks/composables/useBuiltinTasks', () => ({ trackedCarveStart: ipc.start }));
vi.mock('../../../composables/useConfirm', () => ({ confirm: ipc.confirm }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: ipc.open }));
vi.mock('../../../composables/useToast', () => ({ notifyError: vi.fn(), notifyInfo: vi.fn(), notifySuccess: vi.fn() }));
vi.mock('../../../lib/ipc/taskRuntime', () => ({ onTaskProgress: vi.fn(async () => () => {}), taskCancel: vi.fn() }));
vi.mock('../../../lib/ipc/carve', () => ({
  carveFormats: vi.fn(async () => [
    { id: 'jpg', label: 'JPEG', category: 'image', extension: 'jpg' },
    { id: 'png', label: 'PNG', category: 'image', extension: 'png' },
    { id: 'pdf', label: 'PDF', category: 'document', extension: 'pdf' },
  ]), carveScanList: ipc.list, carveScanRemove: ipc.remove, carveStart: ipc.start, carveScanStatus: ipc.status,
  carveResultsPage: ipc.page, carveScanPause: ipc.pause, carveScanResume: ipc.resume,
  carveRecover: ipc.recover, carvePreview: vi.fn(), onCarveScanProgress: ipc.progress, carveFilesystemResults: ipc.filesystem,
}));
const scan = { scan_id: 'scan-a', task_id: 'task-a', image_id: 'disk-a', created_unix_ms: 100, scope_label: '整个镜像', status: 'completed', phase: 'signature', signatures: true, filesystem: null, artifacts_found: 2, signatures_found: 2, bytes_scanned: 1024, bytes_total: 1024 };
const artifacts = ['one.pdf', 'two.pdf'].map((name, index) => ({ id: `file-${index}`, name, format_id: 'pdf', extension: 'pdf', category: 'document', offset: index * 512, length: 512, confidence: 90, region: '分区 0', read_fault: false }));
function node(id: string, name: string, parent: string, kind: 'file' | 'dir', deleted = false): FsNode { return { id, name, parent_id: parent, kind, size: kind === 'file' ? 10 : 0, deleted, inode: null, timestamps: { modified: null, accessed: null, created: null, mft_changed: null, birth: null } }; }
const recovered: RecoveryScan = {
  scan_id: 'scan-fs', image_id: 'disk-a', options: { fat: true, ntfs: true, exfat: true, include_existing: false }, ranges: [], warnings: [], truncated: false,
  read_faults: { bad_regions: [], synthetic_bytes: 0, read_faults_detected: false },
  volumes: [
    { partition: { id: 0, kind: 'fat32', start_lba: 100, length_lba: 1000, label: 'FAT32' }, lost: false, reconstruction: null, error: null },
    { partition: { id: 0, kind: 'ntfs', start_lba: 4096, length_lba: 1000, label: '新加卷' }, lost: true, reconstruction: { mft_offset: 200, cluster_size: 4096, record_size: 1024, sector_size: 512 }, error: null },
  ],
  directories: [{ volume_index: 1, path: '/apk', node: node('ntfs:part:0:30', 'apk', 'part:0', 'dir') }],
  candidates: [
    { id: '0', volume_index: 0, path: '/gone.txt', node: node('fat:0:file', 'gone.txt', 'part:0', 'file', true), initialized_size: null, readable: true },
    { id: '1', volume_index: 1, path: '/full.bin', node: node('ntfs:part:0:31', 'full.bin', 'part:0', 'file'), initialized_size: null, readable: true },
    { id: '2', volume_index: 1, path: '/apk/app.apk', node: node('ntfs:part:0:32', 'app.apk', 'ntfs:part:0:30', 'file', true), initialized_size: null, readable: true },
  ],
};
const filesystemScan = { ...scan, scan_id: 'scan-fs', phase: 'filesystem', artifacts_found: 0, filesystem: { options: recovered.options, done: 3, total: null, finished: true, volumes: 2, directories: 1, files: 3, truncated: false, read_faults_detected: false, error: null } };
let app: App | null = null;
const imageId = ref('disk-a');
async function settle() { for (let i = 0; i < 15; i++) await nextTick(); }
async function mount(imageInfo: ImageInfo | null = null) { const host = document.createElement('div'); document.body.append(host); app = createApp({ render: () => h(CarvePanel, { imageId: imageId.value, imageInfo }) }); app.mount(host); await settle(); }
function button(text: string) { const element = [...document.querySelectorAll('button')].find(item => item.textContent?.trim() === text); if (!element) throw new Error(`Missing ${text}`); return element; }
function rowNames() { return [...document.querySelectorAll('tbody tr .name-content')].map(cell => cell.querySelector('.name-text, .name-link')?.textContent?.trim()); }
function row(name: string) { const found = [...document.querySelectorAll<HTMLElement>('tbody tr')].find(item => item.querySelector('.name-text, .name-link')?.textContent?.trim() === name); if (!found) throw new Error(`Missing row ${name}`); return found; }
async function openFolder(...names: string[]) {
  for (const name of names) {
    const link = [...document.querySelectorAll<HTMLButtonElement>('.name-link')].find(item => item.textContent?.trim() === name);
    if (!link) throw new Error(`Missing folder ${name}`);
    link.click(); await settle();
  }
}
async function filter(text: string) {
  const search = document.querySelector('input[type="search"]') as HTMLInputElement;
  search.value = text; search.dispatchEvent(new Event('input')); await settle();
  search.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); await settle();
}
beforeEach(() => { vi.resetAllMocks(); imageId.value = 'disk-a'; ipc.list.mockResolvedValue([]); ipc.progress.mockResolvedValue(() => {}); ipc.confirm.mockResolvedValue(true); ipc.status.mockResolvedValue(scan); ipc.start.mockResolvedValue({ scan_id: 'scan-a' }); ipc.page.mockResolvedValue({ items: artifacts, total: 2, next_cursor: null, resume_cursor: null }); ipc.filesystem.mockResolvedValue(recovered); ipc.open.mockResolvedValue('D:/recovered'); ipc.recover.mockResolvedValue({ recovered: 1, failed: 0, bytes_written: 512, report_path: 'report.xml', files: null }); });
afterEach(() => { app?.unmount(); app = null; document.body.replaceChildren(); });

describe('recovery workspace', () => {
  it('selects formats by category and submits both recovery methods with collapsed advanced settings', async () => {
    await mount();
    expect(document.querySelector('[role=dialog]')).toBeNull();
    button('新建扫描').click(); await settle();
    expect(document.querySelector('[role=dialog]')).not.toBeNull();
    expect(document.querySelector('details')?.open).toBe(false);
    expect(button('指定分区').disabled).toBe(true);
    button('清空').click(); await settle();
    expect(button('开始扫描').disabled).toBe(true);
    expect(document.querySelector('#carve-method-error')?.textContent).toContain('签名雕刻至少需要一种文件类型');
    (document.querySelector('.group-label input') as HTMLInputElement).click(); await settle();
    expect(button('开始扫描').disabled).toBe(false);
    (document.querySelectorAll('.format-groups .format-chip input')[0] as HTMLInputElement).click(); await settle();
    expect((document.querySelector('.group-label input') as HTMLInputElement).indeterminate).toBe(true);
    ipc.list.mockResolvedValue([scan]); button('开始扫描').click(); await settle();
    expect(ipc.start).toHaveBeenCalledWith({ imageId: 'disk-a', options: {
      format_ids: ['png'], scope: 'whole', partition_id: null, start_sector: null, sector_count: null, max_file_size: 10 * 1024 ** 3, max_results: 20000,
      filesystems: { fat: true, ntfs: true, exfat: true, include_existing: false }, signatures: true,
    } });
  });

  it('submits a filesystem-only scan of a validated sector range', async () => {
    await mount({ size_bytes: 1024 * 512, sector_size: 512, partitions: [] } as unknown as ImageInfo);
    button('新建扫描').click(); await settle();
    const [, carving] = [...document.querySelectorAll<HTMLInputElement>('.method-choice > input')];
    carving!.click(); await settle();
    expect(document.querySelector('.format-groups')).toBeNull();
    button('扇区范围').click(); await settle();
    const [start] = [...document.querySelectorAll<HTMLInputElement>('.range-fields input')];
    start!.value = '2000'; start!.dispatchEvent(new Event('input')); await settle();
    expect(start!.getAttribute('aria-invalid')).toBe('true');
    expect(button('开始扫描').disabled).toBe(true);
    start!.value = '100'; start!.dispatchEvent(new Event('input')); await settle();
    const ntfs = [...document.querySelectorAll<HTMLLabelElement>('.family-row .format-chip')].find(item => item.textContent?.trim() === 'NTFS')!;
    ntfs.querySelector('input')!.click(); await settle();
    button('开始扫描').click(); await settle();
    expect(ipc.start).toHaveBeenCalledWith({ imageId: 'disk-a', options: expect.objectContaining({
      format_ids: [], scope: 'range', start_sector: 100, sector_count: 924, signatures: false,
      filesystems: { fat: true, ntfs: false, exfat: true, include_existing: false },
    }) });
  });

  it('browses recovered volumes like the file list and recovers a checked folder', async () => {
    ipc.list.mockResolvedValue([filesystemScan]); ipc.page.mockResolvedValue({ items: [], total: 0, next_cursor: null, resume_cursor: null });
    await mount();
    expect(ipc.filesystem).toHaveBeenCalledWith({ scanId: 'scan-fs' });
    expect(rowNames()).toEqual(['FAT32', '新加卷', '签名雕刻']);
    expect([...document.querySelectorAll('.tree-row')].map(item => item.querySelector('.tree-name')?.textContent)).toEqual(['FAT32', '新加卷', '签名雕刻']);
    await openFolder('新加卷');
    expect(rowNames()).toEqual(['apk', 'full.bin']);
    expect(document.querySelector('.breadcrumb')?.textContent).toContain('新加卷');
    await openFolder('apk');
    expect(rowNames()).toEqual(['app.apk']);
    expect(row('app.apk').classList.contains('deleted')).toBe(true);
    (document.querySelector('[aria-label="返回上一级"]') as HTMLButtonElement).click(); await settle();
    expect(rowNames()).toEqual(['apk', 'full.bin']);
    row('full.bin').click(); await settle();
    expect(document.querySelector('.detail-dock')?.textContent).toContain('/full.bin');
    row('apk').querySelector('input')!.click(); await settle();
    button('恢复所选（1）').click(); await settle();
    expect(ipc.recover).toHaveBeenCalledWith({ scanId: 'scan-fs', options: { artifact_ids: [], file_ids: ['2'], destination: 'D:/recovered', task_id: expect.any(String) } });
  });

  it('reports a failed filesystem phase without requesting its results', async () => {
    ipc.list.mockResolvedValue([{ ...filesystemScan, filesystem: { ...filesystemScan.filesystem, error: '引导扇区读取失败' } }]);
    await mount();
    expect(document.body.textContent).toContain('文件系统扫描失败：引导扇区读取失败');
    expect(ipc.filesystem).not.toHaveBeenCalled();
  });

  it('opens and closes details on demand, filters by filename, and recovers only checked results', async () => {
    ipc.list.mockResolvedValue([scan]); await mount();
    expect(rowNames()).toEqual(['签名雕刻']);
    await openFolder('签名雕刻', 'PDF');
    expect(document.querySelector('.detail-dock')).toBeNull();
    row('one.pdf').click(); await settle();
    expect(document.querySelector('.detail-dock')?.textContent).toContain('one.pdf');
    (document.querySelector('[aria-label="关闭文件详情"]') as HTMLButtonElement).click(); await settle();
    expect(document.querySelector('.detail-dock')).toBeNull();
    await filter('two.pdf');
    expect(rowNames()).toEqual(['two.pdf']);
    (document.querySelector('tbody input') as HTMLInputElement).click(); await settle();
    button('恢复所选（1）').click(); await settle();
    expect(ipc.recover).toHaveBeenCalledWith({ scanId: 'scan-a', options: { artifact_ids: ['file-1'], file_ids: [], destination: 'D:/recovered', task_id: expect.any(String) } });
  });

  it('keeps pause and resume available without exposing configuration during scanning', async () => {
    ipc.list.mockResolvedValue([{ ...scan, status: 'running' }]); ipc.pause.mockResolvedValue({ ...scan, status: 'paused' }); ipc.resume.mockResolvedValue({ ...scan, status: 'running' }); await mount();
    expect(document.querySelector('[role=dialog]')).toBeNull();
    button('暂停').click(); await settle(); expect(ipc.pause).toHaveBeenCalledWith({ scanId: 'scan-a' });
    button('继续').click(); await settle(); expect(ipc.resume).toHaveBeenCalledWith({ scanId: 'scan-a' });
  });

  it('dismisses scan settings without replacing the current results', async () => {
    ipc.list.mockResolvedValue([scan]); await mount();
    button('新建扫描').click(); await settle();
    expect(document.querySelector('[role=dialog]')).not.toBeNull();
    button('取消').click(); await settle();
    await vi.waitFor(() => expect(document.querySelector('[role=dialog]')).toBeNull());
    expect(rowNames()).toEqual(['签名雕刻']);
    expect(ipc.start).not.toHaveBeenCalled();
  });
});


describe('scan history', () => {
  it('opens retained records and switches the selected scan without accepting stale results', async () => {
    const older = { ...scan, scan_id: 'scan-b', scope_label: '分区 1', created_unix_ms: 50 };
    ipc.list.mockResolvedValue([scan, older]);
    let resolve!: (value: unknown) => void;
    ipc.page.mockReturnValueOnce(new Promise(finish => { resolve = finish; }));
    await mount();
    expect((document.querySelector('.task-pane') as HTMLElement).style.display).not.toBe('none');
    ipc.page.mockResolvedValue({ items: [{ ...artifacts[1], name: 'older.pdf' }], total: 1, next_cursor: null });
    (document.querySelectorAll('.task-open')[1] as HTMLButtonElement).click(); await settle();
    expect(ipc.page).toHaveBeenLastCalledWith({ scanId: 'scan-b', cursor: null, limit: 500 });
    await openFolder('签名雕刻', 'PDF');
    expect(rowNames()).toEqual(['older.pdf']);
    resolve({ items: artifacts, total: 2, next_cursor: null }); await settle();
    expect(rowNames()).toEqual(['older.pdf']);
    ipc.progress.mock.calls[0][0]({ payload: { ...scan, status: 'running', artifacts_found: 3 } }); await settle();
    expect(document.querySelector('.result-summary')?.textContent).toContain('分区 1');
  });

  it('deletes a finished record and selects the next, while active records cannot be deleted', async () => {
    const active = { ...scan, scan_id: 'scan-b', scope_label: '分区 1', status: 'running' };
    ipc.list.mockResolvedValue([scan, active]); await mount();
    expect((document.querySelectorAll('.task-delete')[1] as HTMLButtonElement).disabled).toBe(true);
    ipc.list.mockResolvedValue([active]);
    (document.querySelector('.task-delete') as HTMLButtonElement).click(); await settle();
    expect(ipc.remove).toHaveBeenCalledWith({ imageId: 'disk-a', scanId: 'scan-a' });
    expect(document.querySelectorAll('.task-open')).toHaveLength(1);
    expect(document.querySelector('.result-summary')?.textContent).toContain('分区 1');
  });

  it('automatically opens the first scan arriving through progress events', async () => {
    await mount();
    expect((document.querySelector('.task-pane') as HTMLElement).style.display).toBe('none');
    ipc.progress.mock.calls[0][0]({ payload: { ...scan, status: 'running' } }); await settle();
    expect((document.querySelector('.task-pane') as HTMLElement).style.display).not.toBe('none');
    expect(document.querySelectorAll('.task-open')).toHaveLength(1);
    expect(document.querySelector('.result-summary')?.textContent).toContain('整个镜像');
  });
});


it('focuses scan settings so Escape dismisses them and restores focus', async () => {
  await mount();
  const trigger = button('新建扫描'); trigger.focus(); trigger.click(); await settle();
  const dialog = document.querySelector('[role=dialog]') as HTMLElement;
  expect(dialog.contains(document.activeElement)).toBe(true);
  document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await vi.waitFor(() => expect(document.querySelector('[role=dialog]')).toBeNull());
  expect(document.activeElement).toBe(trigger);
});


it('fetches final results even when completion arrives during an earlier page request', async () => {
  ipc.list.mockResolvedValue([{ ...scan, status: 'running', artifacts_found: 1 }]);
  let resolve!: (value: unknown) => void;
  ipc.page.mockReturnValueOnce(new Promise(finish => { resolve = finish; }));
  await mount();
  ipc.progress.mock.calls[0][0]({ payload: scan }); await settle();
  resolve({ items: [artifacts[0]], total: 1, next_cursor: null, resume_cursor: 'cursor-1' });
  await settle();
  expect(ipc.page).toHaveBeenLastCalledWith({ scanId: 'scan-a', cursor: 'cursor-1', limit: 500 });
  await openFolder('签名雕刻', 'PDF');
  expect(rowNames()).toEqual(['one.pdf', 'two.pdf']);
});

it('does not restore an old scan after A→B→A during scan creation', async () => {
  let finish!: (value: unknown) => void;
  ipc.start.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  await mount(); button('新建扫描').click(); await settle(); button('开始扫描').click(); await settle();
  imageId.value = 'disk-b'; await settle(); imageId.value = 'disk-a'; await settle();
  finish({ scan_id: 'old' }); await settle();
  expect(ipc.status).not.toHaveBeenCalled(); expect(document.querySelector('tbody')).toBeNull();
});
it('does not recover to a folder chosen for an old evidence view', async () => {
  let finish!: (value: string) => void; ipc.open.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  ipc.list.mockResolvedValue([scan]); await mount(); button('恢复全部').click(); await settle();
  imageId.value = 'disk-b'; await settle(); finish('D:/old'); await settle(); expect(ipc.recover).not.toHaveBeenCalled();
});

it('reads every result page before building the signature folders', async () => {
  ipc.list.mockResolvedValue([scan]);
  ipc.page.mockResolvedValueOnce({ items: [artifacts[0]], total: 2, next_cursor: 'next', resume_cursor: 'next' })
    .mockResolvedValueOnce({ items: [artifacts[1]], total: 2, next_cursor: null, resume_cursor: 'last' });
  await mount();
  expect(ipc.page).toHaveBeenLastCalledWith({ scanId: 'scan-a', cursor: 'next', limit: 500 });
  await openFolder('签名雕刻', 'PDF');
  expect(rowNames()).toEqual(['one.pdf', 'two.pdf']);
  expect(document.querySelector('.footer-total')?.textContent).toContain('2');
});

it('keeps read failure separate from an empty scan and retries in place', async () => {
  ipc.list.mockResolvedValue([scan]); ipc.page.mockRejectedValueOnce(new Error('读取中断'));
  await mount();
  expect(document.querySelector('.result-read-error')?.textContent).toContain('读取中断');
  expect(document.body.textContent).not.toContain('本次扫描没有发现可恢复的文件');
  button('重试').click(); await settle();
  expect(document.querySelector('.result-read-error')).toBeNull();
  await openFolder('签名雕刻', 'PDF');
  expect(rowNames()).toEqual(['one.pdf', 'two.pdf']);
});

it('retains loaded rows and resumes a failed page on retry', async () => {
  ipc.list.mockResolvedValue([scan]);
  ipc.page.mockResolvedValueOnce({ items: [artifacts[0]], total: 2, next_cursor: 'next', resume_cursor: 'next' })
    .mockRejectedValueOnce(new Error('分页中断'));
  await mount();
  expect(document.querySelector('.result-read-error')?.textContent).toContain('分页中断');
  await openFolder('签名雕刻', 'PDF');
  expect(rowNames()).toEqual(['one.pdf']);
  ipc.page.mockResolvedValueOnce({ items: [artifacts[1]], total: 2, next_cursor: null, resume_cursor: 'last' });
  button('重试').click(); await settle();
  expect(ipc.page).toHaveBeenLastCalledWith({ scanId: 'scan-a', cursor: 'next', limit: 500 });
  expect(rowNames()).toEqual(['one.pdf', 'two.pdf']);
  expect(document.querySelector('.result-read-error')).toBeNull();
});

it('virtualizes a large scan while selecting every listed result for recovery', async () => {
  const many = Array.from({ length: 5000 }, (_, index) => ({ ...artifacts[0], id: `item-${index}`, name: `${index}.pdf`, offset: index * 512 }));
  ipc.list.mockResolvedValue([{ ...scan, artifacts_found: many.length }]);
  ipc.page.mockResolvedValue({ items: many, total: many.length, next_cursor: null });
  await mount();
  await openFolder('签名雕刻', 'PDF');
  expect(document.querySelectorAll('tbody tr:not(.virtual-spacer)').length).toBeLessThan(100);
  expect(document.querySelector('.virtual-spacer')).not.toBeNull();
  (document.querySelector('thead input') as HTMLInputElement).click(); await settle();
  button('恢复所选（5000）').click(); await settle();
  expect(ipc.recover.mock.calls[0][0].options.artifact_ids).toHaveLength(5000);
});

it('offers retry when history cannot be read instead of showing an empty history', async () => {
  ipc.list.mockRejectedValueOnce(new Error('记录不可读')); await mount();
  expect(document.querySelector('.workspace-start')?.textContent).toContain('记录不可读');
  expect(document.body.textContent).not.toContain('还没有恢复扫描');
  ipc.list.mockResolvedValue([scan]); button('重试').click(); await settle();
  expect(rowNames()).toEqual(['签名雕刻']);
});
