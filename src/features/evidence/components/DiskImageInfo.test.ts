// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, shallowRef, type App } from 'vue';
import type { ImageInfo } from '../../../lib/ipc/image';
import type { EwfAcquiryInfo } from '../../../lib/ipc/image';
import type { ShadowCopyInfo, ShadowVolumeInfo } from '../../../lib/ipc/shadow';
import { fsAvailableMountPoints, fsMountReadonly, fsMounts, type DokanMountView } from '../../../lib/ipc/mount';
import { useUiPreferences } from '../../../composables/useUiPreferences';

vi.mock('../../../lib/ipc/mount', () => ({ fsMounts: vi.fn(async () => []), fsAvailableMountPoints: vi.fn(), fsCreateMountDir: vi.fn(), fsMountReadonly: vi.fn(), fsShareMount: vi.fn(), fsUnmountReadonly: vi.fn(), fsUnshareMount: vi.fn() }));
vi.mock('../../../lib/ipc/bitlocker', () => ({ bitlockerStatus: vi.fn(async () => ({ locked: true })) }));
const shadow = vi.hoisted(() => ({ list: vi.fn(), open: vi.fn() }));
vi.mock('../../../lib/ipc/shadow', () => ({ shadowCopyList: shadow.list, shadowCopyOpen: shadow.open }));
const hash = vi.hoisted(() => ({ result: { value: undefined as Record<string, string | null> | undefined } }));
const imageExport = vi.hoisted(() => ({ start: vi.fn(), job: vi.fn() }));
vi.mock('../composables/useImageExport', () => ({ useImageExport: () => ({ startImageExport: imageExport.start, jobFor: imageExport.job }) }));
vi.mock('../composables/useImageHash', () => ({ useImageHashResult: () => hash.result }));
vi.mock('../../cases/composables/useCaseSession', () => ({ useCaseSession: () => ({ activeCase: { value: null } }) }));
vi.mock('../../../composables/useToast', () => ({ notifySuccess: vi.fn(), notifyError: vi.fn() }));
vi.mock('./ImageHashPanel.vue', () => ({ default: { render: () => h('input', { 'aria-label': '哈希校验测试状态' }) } }));
vi.mock('./AutoDetectBadge.vue', () => ({ default: defineComponent({ setup: (_, { slots }) => () => h('div', ['平台结论', slots.default?.(), h('button', { 'aria-label': '重新识别平台' }, '刷新')]) }) }));
vi.mock('../../raid/components/RaidStackPanel.vue', () => ({ default: { render: () => h('div', 'RAID 检测结果') } }));
vi.mock('./BitlockerUnlockControl.vue', () => ({ default: { render: () => h('button', '解锁 BitLocker') } }));
vi.mock('./EncryptedVolumeUnlockDialog.vue', async () => {
  const { defineComponent: define, h: render } = await import('vue');
  return {
    default: define({
      props: { open: Boolean, partitionId: { type: Number, default: null } },
      setup: props => () => (props.open ? render('div', { class: 'unlock-dialog', 'data-partition': String(props.partitionId) }) : null),
    }),
  };
});

import DiskImageInfo from './DiskImageInfo.vue';

const fixture: ImageInfo = {
  id: 'disk-one', path: 'D:/evidence/disk.E01', source_format: 'ewf',
  size_bytes: 1024 * 1024 * 1024, sector_size: 512,
  partitions: [
    { id: 1, kind: 'ntfs', label: '系统卷', start_lba: 2048, length_lba: 1024000 },
    { id: 2, kind: 'bitlocker', label: '加密卷', start_lba: 1026048, length_lba: 1024000 },
  ],
  table_scheme: 'gpt', disk_id: 'disk-guid', on_disk_bytes: 512 * 1024 * 1024,
  modified_unix_ms: 1_700_000_000_000, created_unix_ms: 1_700_000_000_000, ewf: null,
};

const acquisition: EwfAcquiryInfo = {
  case_number: 'CASE-7', evidence_number: 'E-1', examiner_name: '检验员', description: null, notes: '现场提取',
  acquiry_date: '2026-09-14', system_date: null, acquiry_operating_system: null, acquiry_software_version: null,
  model: 'SSD 250GB', serial_number: null, stored_md5: '9E107D9D372BB6826BD81D3542A419D6',
  stored_sha1: '2fd4e1c67a2d28fced849ee1bb76e7391b93eb12', format: null, media_type: 'fixed', compression: null, segment_count: null,
};

function copy(index: number): ShadowCopyInfo {
  return {
    index, store_id: `store-${index}`, shadow_copy_id: `copy-${index}`, shadow_copy_set_id: null, creation_filetime: 0,
    created_unix_ms: 1_619_890_803_223, volume_size: 1, snapshot_context: null, attributes: null, persistent: true,
    client_accessible: false, operating_machine: 'DESKTOP-01', service_machine: null, unavailable_reason: null, open_image_id: null,
  };
}

function volume(partition_id: number, shadow_copies: ShadowCopyInfo[], error: string | null = null): ShadowVolumeInfo {
  return { partition_id, volume_size: 1, shadow_copies, error };
}

let app: App | undefined;
let host: HTMLDivElement;
const opened = vi.fn();

async function settle() {
  for (let i = 0; i < 4; i += 1) await nextTick();
}

async function mount(info: ImageInfo = fixture) {
  host = document.createElement('div');
  document.body.append(host);
  const currentInfo = shallowRef(info);
  app = createApp(defineComponent({ render: () => h(DiskImageInfo, { info: currentInfo.value, 'onOpen-image': opened }) }));
  app.mount(host);
  await settle();
  return currentInfo;
}

function section(name: string) {
  return host.querySelector<HTMLElement>(`[data-section="${name}"]`);
}

function row(id: number) {
  return host.querySelector<HTMLElement>(`#part-row-${id}`)!;
}

function button(scope: ParentNode, label: string) {
  return [...scope.querySelectorAll<HTMLButtonElement>('button')].find(candidate => candidate.textContent?.includes(label));
}

beforeEach(() => {
  vi.clearAllMocks();
  imageExport.job.mockReset();
  vi.mocked(fsMounts).mockReset().mockResolvedValue([]);
  hash.result.value = undefined;
  shadow.list.mockResolvedValue([]);
});

afterEach(() => {
  app?.unmount();
  host?.remove();
});

describe('DiskImageInfo evidence overview', () => {
  it('shows a readable reconstruction name and exports its original image from the header', async () => {
    const path = 'assembled://md:auto-raid5-59494dc69af24768b6a690819d35420e';
    await mount({ ...fixture, path, source_format: 'assembled', on_disk_bytes: null,
      partitions: [{ id: 0, kind: 'ext4', label: null, start_lba: 0, length_lba: 2097152 }] });
    expect(host.querySelector('.evidence-name')?.textContent).toBe('RAID 5 重组镜像');
    expect(host.querySelector('.evidence-path')).toBeNull();
    expect(section('details')?.textContent).toContain(path);
    expect(section('details')?.querySelector('[aria-label="复制重组镜像标识"]')).not.toBeNull();
    expect(row(0).querySelector('.part-index')).toBeNull();
    const exportButton = button(host.querySelector('.evidence-head')!, '导出原始镜像')!;
    expect(exportButton.disabled).toBe(false);
    exportButton.click(); await settle();
    expect(imageExport.start).toHaveBeenCalledWith({ imageId: fixture.id, defaultFileName: 'RAID_5_重组镜像.dd',
      mode: 'unlockedPartitions', title: '导出原始镜像为 RAW', runningTitle: '导出原始镜像中…', doneTitle: '原始镜像导出完成' });
    expect(opened).not.toHaveBeenCalled();
  });
  it('keeps the drive picker open inside the extracted mount control and dispatches the selected drive', async () => {
    useUiPreferences().mountTarget.value = 'drive';
    vi.mocked(fsAvailableMountPoints).mockResolvedValue(['H:\\', 'I:\\']);
    const mountView = { id: 'mount-one', image_id: fixture.id, root_node_id: 'part:1', mount_point: 'I:\\', share: null } as DokanMountView;
    vi.mocked(fsMountReadonly).mockResolvedValue(mountView);
    vi.mocked(fsMounts).mockResolvedValueOnce([]).mockResolvedValueOnce([mountView]);
    await mount();
    row(1).querySelector<HTMLButtonElement>('.mount-btn')!.click(); await settle();
    expect(row(1).querySelector('.drive-picker')).not.toBeNull();
    button(row(1), 'I:')!.click(); await settle();
    expect(fsMountReadonly).toHaveBeenCalledWith({ imageId: fixture.id, nodeId: 'part:1', mountPoint: 'I:\\' });
    expect(row(1).querySelector('.drive-picker')).toBeNull(); expect(row(1).querySelector('.mount-btn')?.textContent).toContain('I:');
  });
  it('opens on the evidence and its platform, then its partitions, integrity and details', async () => {
    await mount();
    expect(host.querySelector('.evidence-name')?.textContent).toBe('disk.E01');
    expect(host.querySelector('.evidence-head')?.textContent).toContain('EWF');
    expect(host.querySelector('.evidence-head')?.textContent).toContain('平台结论');
    const actions = host.querySelector('.evidence-actions')!;
    expect([...actions.querySelectorAll('button')].map(item => item.textContent?.trim())).toEqual(['导出原始镜像', '刷新']);
    button(actions, '导出原始镜像')!.click(); await settle();
    expect(imageExport.start).toHaveBeenCalledWith(expect.objectContaining({ imageId: fixture.id, mode: 'unlockedPartitions', defaultFileName: 'disk.E01.dd' }));
    expect(host.querySelector('.evidence-path')?.textContent).toContain('D:/evidence/disk.E01');
    expect(host.querySelector('.evidence-size')?.textContent?.trim()).toBe('1.00 GiB · 实际占用 512.00 MiB（50.0%）');
    for (const name of ['volumes', 'integrity', 'details']) {
      expect(section(name), name).not.toBeNull();
    }
    expect(section('integrity')?.querySelector('[aria-label="哈希校验测试状态"]')).not.toBeNull();

    expect(host.querySelectorAll('.partition-row')).toHaveLength(2);
    expect(row(1).textContent).toContain('系统卷');
    expect(row(1).textContent).toContain('NTFS');
    expect(row(1).querySelector('.col-start')?.textContent).toContain('2,048');
    expect(row(1).querySelector('.col-start')?.getAttribute('title')).toContain('字节偏移 0x100000');
    expect(row(1).querySelector('.col-size')?.textContent).toBe('500.00 MiB');
    expect(row(2).textContent).toContain('解锁 BitLocker');

    const details = section('details')!.textContent;
    expect(details).toContain('1,073,741,824 字节');
    expect(details).toContain('512 B');
    expect(details).toContain('disk-guid');
    // The image id and the image file's own timestamps stay off the page.
    expect(host.textContent).not.toContain('disk-one');
    expect(host.textContent).not.toContain('2023');
  });

  it('leaves the on-disk size out when it equals the logical size', async () => {
    await mount({ ...fixture, on_disk_bytes: fixture.size_bytes });
    expect(host.querySelector('.evidence-size')?.textContent).toBe('1.00 GiB');
  });

  it('locates the selected partition from the disk map', async () => {
    await mount();
    row(2).scrollIntoView = vi.fn();
    host.querySelectorAll<HTMLButtonElement>('.layout-seg')[1].click();
    await settle();
    expect(row(2).classList.contains('active')).toBe(true);
    expect(host.querySelectorAll<HTMLButtonElement>('.layout-seg')[1].classList.contains('active')).toBe(true);
    expect(document.activeElement).toBe(row(2));
  });

  it('mutes partitions without a file system', async () => {
    await mount({ ...fixture, partitions: [...fixture.partitions, { id: 3, kind: 'unknown', label: null, start_lba: 2050048, length_lba: 32768 }] });
    expect(row(3).classList.contains('quiet')).toBe(true);
    expect(row(1).classList.contains('quiet')).toBe(false);
  });

  it('clears the located partition and collapses acquisition details on a new image', async () => {
    const currentInfo = await mount({ ...fixture, ewf: acquisition });
    const facts = section('details')!.textContent;
    for (const value of ['CASE-7', 'E-1', '检验员', '2026-09-14', 'SSD 250GB']) expect(facts).toContain(value);
    row(2).scrollIntoView = vi.fn();
    host.querySelectorAll<HTMLButtonElement>('.layout-seg')[1].click();
    await settle();
    host.querySelector<HTMLDetailsElement>('.acquisition-details')!.open = true;
    currentInfo.value = { ...fixture, id: 'disk-two', ewf: acquisition };
    await settle();
    expect(host.querySelector('.partition-row.active')).toBeNull();
    expect(host.querySelector<HTMLDetailsElement>('.acquisition-details')?.open).toBe(false);
  });

  it('checks the computed hashes against the hashes recorded at acquisition', async () => {
    await mount({ ...fixture, ewf: acquisition });
    expect([...host.querySelectorAll('.stored-hash-row')].map(hashRow => hashRow.className)).toEqual([
      'stored-hash-row verdict-pending', 'stored-hash-row verdict-pending',
    ]);
    expect(host.querySelector('.verdict-badge')).toBeNull();
    app?.unmount();
    host.remove();

    hash.result.value = { md5: '9e107d9d372bb6826bd81d3542a419d6', sha1: '0000000000000000000000000000000000000000' };
    await mount({ ...fixture, ewf: acquisition });
    expect(host.querySelector('.stored-hash-row.verdict-match')?.textContent).toContain('MD5');
    expect(host.querySelector('.stored-hash-row.verdict-mismatch')?.textContent).toContain('SHA1');
    expect(host.querySelector('.verdict-badge')?.textContent).toContain('不一致');
  });

  it('folds shadow copies into their partition rows and opens one in a new tab', async () => {
    const vss = { ...fixture, partitions: [fixture.partitions[0], { id: 3, kind: 'ntfs', label: '数据', start_lba: 2050048, length_lba: 1024000 }] };
    shadow.list.mockResolvedValue([volume(1, [copy(0), copy(1)]), volume(3, [])]);
    shadow.open.mockResolvedValue({ id: 'snapshot', path: 'vss://disk-one/1/store-1/分区 1 卷影副本 2' });
    await mount(vss);
    expect(shadow.list).toHaveBeenCalledWith({ imageId: 'disk-one' });
    expect(row(3).querySelector('.shadow-none')?.getAttribute('title')).toContain('已被删除');

    const toggle = button(row(1), '卷影副本 2')!;
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    toggle.click();
    await settle();
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    const list = host.querySelector('#shadow-list-1')!;
    expect(list.querySelectorAll('.copy-row')).toHaveLength(2);

    button(list.querySelectorAll('.copy-row')[1], '打开')!.click();
    await settle();
    expect(shadow.open).toHaveBeenCalledWith(expect.objectContaining({ imageId: 'disk-one', partitionId: 1, index: 1 }));
    expect(opened).toHaveBeenCalledWith('vss://disk-one/1/store-1/分区 1 卷影副本 2');
    expect(button(list.querySelectorAll('.copy-row')[1], '切换到')).toBeDefined();
  });

  it('shows unreadable VSS metadata in the row and retries a failed listing', async () => {
    shadow.list
      .mockRejectedValueOnce(new Error('image not loaded'))
      .mockResolvedValueOnce([volume(1, [], 'shadow copy evidence contains unreadable sectors')]);
    await mount();
    const alert = section('volumes')!.querySelector('[role=alert]')!;
    expect(alert.textContent).toContain('image not loaded');
    button(alert, '重试')!.click();
    await settle();
    expect(section('volumes')!.querySelector('.shadow-alert')).toBeNull();

    button(row(1), '卷影副本读取失败')!.click();
    await settle();
    expect(host.querySelector('#shadow-list-1 [role=alert]')?.textContent).toContain('unreadable sectors');
  });

  it('unlocks LUKS partitions and encrypted containers from the volume card', async () => {
    await mount({ ...fixture, partitions: [...fixture.partitions, { id: 3, kind: 'luks', label: 'Linux', start_lba: 2050048, length_lba: 1024000 }] });
    expect(host.querySelector('.unlock-dialog')).toBeNull();
    button(row(3), '解锁')!.click();
    await settle();
    expect(host.querySelector('.unlock-dialog')?.getAttribute('data-partition')).toBe('3');
  });

  it('opens the container unlock for the whole image from the card header', async () => {
    await mount();
    button(section('volumes')!.querySelector('.ui-card-header')!, '解锁加密容器')!.click();
    await settle();
    expect(host.querySelector('.unlock-dialog')?.getAttribute('data-partition')).toBe('null');
  });

  it.each(['zip', 'android_backup'])('keeps the %s archive root mount entry when there are no physical partitions', async sourceFormat => {
    await mount({ ...fixture, source_format: sourceFormat, partitions: [] });
    expect(host.querySelector('.synthetic-root-title')?.textContent).toBe('归档根');
    expect(host.querySelector('.synthetic-root-mount .mount-btn')).not.toBeNull();
    expect(host.querySelector('.partition-table')).toBeNull();
    expect(button(host, '解锁加密容器')).toBeUndefined();
    expect(button(host, '导出原始镜像')).toBeUndefined();
    expect(shadow.list).not.toHaveBeenCalled();
    if (sourceFormat === 'android_backup') expect(host.querySelector('.evidence-head .ui-badge')?.textContent).toBe('Android 备份');
  });

  it('treats an AD1 logical image like the other logical evidence', async () => {
    await mount({ ...fixture, path: 'D:/evidence/case.ad1', source_format: 'ad1', partitions: [] });
    expect(host.querySelector('.synthetic-root-title')?.textContent).toBe('逻辑镜像根');
    expect(host.querySelector('.synthetic-root-mount .mount-btn')).not.toBeNull();
    expect(host.querySelector('.evidence-head .ui-badge')?.textContent).toBe('AD1');
  });
});
