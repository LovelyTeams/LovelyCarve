// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, ref, shallowRef, type App } from 'vue';
import type { RaidArray, RaidStack } from '../../../lib/ipc/raid';
import type { ImageInfo } from '../../../lib/ipc/image';
import RaidStackPanel from './RaidStackPanel.vue';

const calls = vi.hoisted(() => ({ detect: vi.fn(), assemble: vi.fn(), info: vi.fn(), export: vi.fn() }));
const detection = shallowRef<RaidStack | null>(null); const loading = ref(false); const error = ref<string | null>(null);
vi.mock('../composables/useRaidStack', () => ({ useRaidStack: () => ({ stack: detection, loading, error, detect: calls.detect, assembleDegraded: calls.assemble }) }));
vi.mock('../../evidence/composables/useImageExport', () => ({ useImageExport: () => ({ startImageExport: calls.export, jobFor: () => undefined }) }));
vi.mock('../../../lib/ipc/image', () => ({ imageInfoById: calls.info }));
vi.mock('../../../composables/useToast', () => ({ notifyError: vi.fn() }));
vi.mock('./RaidManualBuilder.vue', () => ({ default: { render: () => h('div', { class: 'manual-builder' }) } }));
vi.mock('./RaidLayoutDiagram.vue', () => ({ default: { render: () => h('div', { class: 'diagram-content' }) } }));

const array: RaidArray = { set_uuid: 'array-a', set_name: 'array/a', level: 5, layout: 0, chunksize_sectors: 128, raid_disks: 3, data_offset_sectors: 0, assembled_bytes: 1024000, assembled_image_id: null, heuristic: false, metadata_version: '1.2', degraded: false, degradable: false, members: [{ dev_number: 0, slot: 0, device_uuid: 'member-a', data_size_sectors: 1000, reconstructed: false, state: 'active', events: 7, origin: { kind: 'whole', image_id: 'a', partition_id: 1, byte_offset: 0, byte_length: 512000 } }] };
const stack: RaidStack = { images: [{ image_id: 'a', path: 'D:/member-a.dd', size_bytes: 512000, partition_count: 1, lvm_pv_count: 0 }], volume_groups: [], arrays: [array], filesystems: [], diagnostics: ['diagnostic'] };
const info = shallowRef<ImageInfo>({ id: 'a', path: 'D:/member-a.dd', source_format: 'raw', size_bytes: 512000, sector_size: 512, partitions: [{ id: 1, kind: 'ntfs', label: null, start_lba: 0, length_lba: 1000 }], table_scheme: 'gpt', disk_id: null, on_disk_bytes: null, created_unix_ms: null, modified_unix_ms: null, ewf: null });
let app: App; let host: HTMLElement; const opened = vi.fn();
async function settle() { for (let index = 0; index < 8; index++) await nextTick(); }
async function mount() { host = document.createElement('div'); document.body.append(host); app = createApp({ render: () => h(RaidStackPanel, { info: info.value, 'onOpen-image': opened }) }); app.mount(host); await settle(); }
function button(label: string) { return [...host.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent?.trim() === label)!; }
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { resolve, promise }; }
beforeEach(() => { vi.resetAllMocks(); info.value = { ...info.value, id: 'a', source_format: 'raw', table_scheme: 'gpt' }; detection.value = stack; loading.value = false; error.value = null; });
afterEach(() => { app?.unmount(); host?.remove(); });

describe('RAID panel composition', () => {
  it('shows only arrays related to the current evidence and preserves diagnostic disclosure', async () => {
    await mount(); expect(host.querySelector('.array-status')?.textContent).toContain('成员不全 1/3');
    expect(host.querySelector('.chip-name')?.textContent).toBe('member-a.dd'); expect(host.querySelector('.diagnostics')?.textContent).toContain('diagnostic');
    info.value = { ...info.value, id: 'unrelated' }; await settle(); expect(host.querySelector('.raid-card')).toBeNull();
  });
  it('keeps global diagnostics separate when the evidence participates in multiple arrays', async () => {
    detection.value = { ...stack, arrays: [array, { ...array, set_uuid: 'array-b', set_name: 'array/b' }] };
    await mount();
    expect(host.querySelectorAll('.raid-array')).toHaveLength(2);
    expect(host.querySelector('.details-diagnostics')).toBeNull();
    expect(host.querySelector('.panel-content > details.diagnostics')?.textContent).toContain('diagnostic');
  });
  it('retains a manually collapsed diagram across detection snapshots', async () => {
    await mount(); const diagram = host.querySelector<HTMLDetailsElement>('.layout-diagram')!; expect(diagram.open).toBe(true);
    diagram.open = false; diagram.dispatchEvent(new Event('toggle')); await settle();
    detection.value = { ...stack, arrays: [{ ...array, assembled_bytes: 2048000 }] }; await settle();
    expect(host.querySelector<HTMLDetailsElement>('.layout-diagram')?.open).toBe(false);
  });
  it('keeps assembled geometry behind one disclosure and updates headerless members in slot order', async () => {
    const members = ['a', 'b', 'c'].map((imageId, slot) => ({
      ...array.members[0], slot, device_uuid: '',
      origin: { kind: 'whole' as const, image_id: imageId, partition_id: 0, byte_offset: 0, byte_length: 512000 },
    }));
    const assembled = { ...array, set_name: 'auto-raid5', assembled_image_id: 'assembled-a', heuristic: true, members: [members[2], members[0], members[1]] };
    detection.value = { ...stack, images: ['a', 'b', 'c'].map(imageId => ({ ...stack.images[0], image_id: imageId, path: `D:/member-${imageId}.dd` })), arrays: [assembled] };
    await mount();
    expect(host.querySelector('.raid-array')?.querySelectorAll('details')).toHaveLength(1);
    expect(host.querySelector<HTMLDetailsElement>('.array-details')?.open).toBe(false);
    expect(host.querySelector('.array-name')).toBeNull();
    expect([...host.querySelectorAll('.chip-name')].map(element => element.textContent?.trim())).toEqual(['member-a.dd', 'member-b.dd', 'member-c.dd']);
    detection.value = { ...detection.value!, arrays: [{ ...assembled, members: [{ ...members[1], slot: 0 }, { ...members[0], slot: 1 }, members[2]] }] };
    await settle();
    expect([...host.querySelectorAll('.chip-name')].map(element => element.textContent?.trim())).toEqual(['member-b.dd', 'member-a.dd', 'member-c.dd']);
  });
  it('shows missing positions and parity reconstruction without shifting the member order', async () => {
    detection.value = { ...stack, arrays: [{ ...array, members: [array.members[0], { ...array.members[0], slot: 2, reconstructed: true, state: 'stale' }] }] };
    await mount();
    const rows = [...host.querySelectorAll('.member-order-item')];
    expect(rows.map(row => row.querySelector('.slot-number')?.textContent)).toEqual(['0', '1', '2']);
    expect(rows[1].textContent).toContain('缺失成员');
    expect(rows[2].textContent).toContain('校验重建');
    expect(host.querySelector('.member-count')?.textContent).toBe('1 / 3 成员盘');
  });
  it('lists slotless members with their state and offers degraded assembly', async () => {
    const spare = { ...array.members[0], slot: null, state: 'spare' as const, device_uuid: 'spare' };
    detection.value = { ...stack, arrays: [{ ...array, degradable: true, members: [array.members[0], spare] }] };
    await mount();
    expect(host.querySelector('.extra-members')?.textContent).toContain('备用');
    expect(host.querySelector('.array-status')?.textContent).toContain('可降级重组');
    button('降级重组').click(); await settle();
    expect(calls.assemble).toHaveBeenCalledWith('array-a');
  });
  it('rescans when an image the last scan has not seen is shown', async () => {
    info.value = { ...info.value, id: 'fresh' };
    await mount();
    expect(calls.detect).toHaveBeenCalledOnce();
  });
  it('routes open/export intents through the original image services', async () => {
    detection.value = { ...stack, arrays: [{ ...array, assembled_image_id: 'assembled-a' }] }; calls.info.mockResolvedValue({ path: 'assembled://array-a' });
    await mount(); button('打开重组镜像').click(); await settle(); expect(calls.info).toHaveBeenCalledWith({ imageId: 'assembled-a' }); expect(opened).toHaveBeenCalledWith('assembled://array-a');
    button('导出重组镜像').click(); await settle(); expect(calls.export).toHaveBeenCalledWith({ imageId: 'assembled-a', defaultFileName: 'array_a_RAID5.dd' });
  });
  it('discards an open result after the panel is destroyed', async () => {
    const summary = deferred<{ path: string }>(); calls.info.mockReturnValue(summary.promise);
    detection.value = { ...stack, arrays: [{ ...array, assembled_image_id: 'assembled-a' }] }; await mount(); button('打开重组镜像').click(); app.unmount();
    summary.resolve({ path: 'assembled://array-a' }); await settle(); expect(opened).not.toHaveBeenCalled();
  });
  it('keeps current reconstruction details without duplicate header or array actions', async () => {
    detection.value = { ...stack, arrays: [{ ...array, assembled_image_id: 'assembled-a' }], filesystems: [{ set_uuid: array.set_uuid, kind: 'ext4', fsid: null, total_bytes: null, bytes_used: null, num_devices: null, sector_size: null, node_size: null }] };
    info.value = { ...info.value, id: 'assembled-a', source_format: 'assembled' };
    await mount();
    expect(host.querySelector('.array-facts')).toBeNull();
    expect(host.querySelectorAll('details')).toHaveLength(1);
    expect(host.querySelector('.array-details summary')?.textContent).toBe('重组详情');
    expect(host.querySelectorAll('button')).toHaveLength(0);
    expect(calls.info).not.toHaveBeenCalled(); expect(calls.export).not.toHaveBeenCalled();
  });
  it('keeps manual reconstruction reachable for a raw member and auto-detects hinted evidence', async () => {
    info.value = { ...info.value, partitions: [], table_scheme: 'mdadm' }; detection.value = null;
    await mount(); expect(calls.detect).toHaveBeenCalledOnce(); button('手动重组').click(); await settle(); expect(host.querySelector('.manual-builder')).not.toBeNull();
  });
});
