// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, shallowRef, type App } from 'vue';
import type { AutoDetectReport } from '../../../lib/ipc/autodetect';
import type { CaseInfo, EvidenceRef } from '../../../lib/ipc/cases';

const detect = vi.hoisted(() => ({ run: vi.fn() }));
const caseMock = vi.hoisted(() => ({ setPlatform: vi.fn(), refreshActive: vi.fn(), notifyError: vi.fn() }));
const report = shallowRef<AutoDetectReport | undefined>();
const pending = shallowRef(false);
const failure = shallowRef<string | null>(null);
const activeCase = shallowRef<CaseInfo | null>(null);
vi.mock('../composables/useAutoDetect', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../composables/useAutoDetect')>()),
  runAutoDetect: detect.run,
  useAutoDetectReport: () => report,
  useAutoDetectPending: () => pending,
  useAutoDetectError: () => failure,
}));
vi.mock('../../tasks/api', () => ({ useTaskEntries: () => ({ entries: shallowRef([]) }) }));
vi.mock('../../cases/api', () => ({ useCaseSession: () => ({ activeCase, refreshActive: caseMock.refreshActive }) }));
vi.mock('../../../lib/ipc/cases', () => ({ caseEvidencePlatformSet: caseMock.setPlatform }));
vi.mock('../../../composables/useToast', () => ({ notifyError: caseMock.notifyError }));
import AutoDetectBadge from './AutoDetectBadge.vue';

let app: App;
let host: HTMLDivElement;

function mount(action?: boolean) {
  host = document.createElement('div');
  document.body.append(host);
  app = createApp({ render: () => h(AutoDetectBadge, { imageId: 'disk-one' }, action ? { default: () => h('button', '导出源镜像') } : undefined) });
  app.mount(host);
}

function caseWith(evidence: Partial<EvidenceRef>, locked = false): CaseInfo {
  return {
    locked,
    evidence: [{ image_id: 'disk-one', path: 'D:\\evidence\\disk-one.E01', source_format: 'ewf', size_bytes: 1, added_at: '', ...evidence }],
  } as CaseInfo;
}

/** The picker's visible label (the trigger also holds hidden sizer labels). */
function selectedChoice() {
  return host.querySelector('.platform-choice .ui-select-value > span:not(.ui-select-sizer)')?.textContent;
}

function choose(label: string) {
  host.querySelector<HTMLButtonElement>('.platform-choice .ui-select')!.click();
  return nextTick().then(() => {
    const option = [...document.querySelectorAll<HTMLElement>('.ui-select-option')].find(item => item.textContent === label);
    option!.click();
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  report.value = undefined;
  pending.value = false;
  failure.value = null;
  activeCase.value = null;
  caseMock.setPlatform.mockResolvedValue({});
  caseMock.refreshActive.mockResolvedValue(undefined);
});
afterEach(() => { app.unmount(); host.remove(); });

describe('platform badge', () => {
  it('shows the drone platform with the DJI manufacturer', () => {
    report.value = { platform: 'drone', platforms: ['drone'], display_platform_name: '无人机 DJI',
      evidence_type: 'disk', is_backup: false, manufactor: 'dji' } as AutoDetectReport;
    mount();
    expect(host.querySelector('.platform-name')?.textContent).toBe('无人机');
    expect(host.querySelector('.platform-type')?.textContent).toBe('DJI 磁盘镜像');
    expect(host.querySelector('svg.lucide-drone')).not.toBeNull();
    expect(detect.run).not.toHaveBeenCalled();
  });

  it.each(['backup', 'XiaomiBackup'])('shows one backup conclusion for %s without repeating the manufacturer or matching rule', evidenceType => {
    const description = 'Android Xiaomi 关键词匹配备份';
    report.value = { platform: 'android', platforms: ['android'], display_platform_name: description,
      evidence_type: evidenceType, is_backup: true, manufactor: 'xiaomi' } as AutoDetectReport;
    mount();
    const chip = host.querySelector('.platform-chip')!;
    expect(chip.querySelector('.platform-name')?.textContent).toBe('Android');
    expect(chip.querySelector('.platform-type')?.textContent).toBe('Xiaomi 备份');
    expect(chip.textContent?.match(/备份/g)).toHaveLength(1);
    expect(chip.textContent?.match(/Xiaomi/g)).toHaveLength(1);
    expect(host.querySelectorAll('.ui-badge')).toHaveLength(0);
    expect(chip.getAttribute('title')).toBe(description);
    expect(detect.run).not.toHaveBeenCalled();
  });

  it('runs the detection once when the page opens without a conclusion', () => {
    mount();
    expect(detect.run).toHaveBeenCalledWith('disk-one');
    expect(host.textContent).toContain('尚未识别平台');
  });

  it('shows the conclusion and runs it again on request', async () => {
    report.value = { platform: 'windows', platforms: ['windows', 'linux'], display_platform_name: null, evidence_type: 'disk', is_backup: false, manufactor: null } as unknown as AutoDetectReport;
    mount(true);
    expect(detect.run).not.toHaveBeenCalled();
    expect(host.querySelector('.platform-chip')?.textContent).toContain('Windows');
    expect(host.querySelector('.platform-chip')?.textContent).toContain('磁盘镜像');
    expect(host.textContent).toContain('Linux');
    expect([...host.querySelectorAll('button')].map(item => item.textContent?.trim())).toEqual(['导出源镜像', '']);
    host.querySelector<HTMLButtonElement>('[aria-label="重新识别平台"]')!.click();
    expect(detect.run).toHaveBeenCalledWith('disk-one');

    pending.value = true;
    await nextTick();
    expect(host.querySelector<HTMLButtonElement>('[aria-label="重新识别平台"]')!.disabled).toBe(true);
  });

  it('names a failed detection', () => {
    failure.value = 'partition table unreadable';
    pending.value = true;
    mount();
    expect(detect.run).not.toHaveBeenCalled();
    pending.value = false;
    return nextTick().then(() => {
      expect(host.querySelector('.platform-state.failed')?.getAttribute('title')).toBe('partition table unreadable');
    });
  });
  it('distinguishes an unknown platform from unidentified RAID geometry', () => {
    report.value = { platform: null, platforms: [], display_platform_name: null, evidence_type: 'disk', is_backup: false, manufactor: null } as unknown as AutoDetectReport;
    mount();
    expect(host.querySelector('.platform-name')?.textContent).toBe('平台未识别');
    expect(detect.run).not.toHaveBeenCalled();
  });
});

describe('manual platform', () => {
  it('offers no picker for evidence outside the active case', () => {
    report.value = { platform: 'linux', platforms: ['linux'], display_platform_name: 'Linux', evidence_type: 'compressfile', is_backup: false, manufactor: null } as AutoDetectReport;
    activeCase.value = { locked: false, evidence: [] } as unknown as CaseInfo;
    mount();
    expect(host.querySelector('.platform-choice')).toBeNull();
  });

  it('puts the manual platform ahead of the detected one and keeps the detection in the tooltip', () => {
    report.value = { platform: 'android', platforms: ['android', 'linux'], display_platform_name: 'Android Xiaomi 关键词匹配备份',
      evidence_type: 'backup', is_backup: true, manufactor: 'xiaomi' } as AutoDetectReport;
    activeCase.value = caseWith({ platform: 'linux', manual_platform: 'linux' });
    mount();
    const chip = host.querySelector('.platform-chip')!;
    expect(chip.querySelector('.platform-name')?.textContent).toBe('Linux');
    expect(chip.querySelector('.platform-type')?.textContent).toBe('手动指定');
    expect(chip.textContent).not.toContain('·');
    expect(chip.getAttribute('title')).toBe('自动识别：Android Xiaomi 关键词匹配备份');
    expect(chip.querySelector('svg.lucide-archive')).toBeNull();
    expect(host.querySelectorAll('.ui-badge')).toHaveLength(0);
    expect(selectedChoice()).toBe('Linux');
  });

  it('shows a manual platform before the first detection finishes', () => {
    pending.value = true;
    activeCase.value = caseWith({ platform: 'windows', manual_platform: 'windows' });
    mount();
    expect(host.querySelector('.platform-name')?.textContent).toBe('Windows');
    expect(host.querySelector('.platform-chip')?.getAttribute('title')).toBeNull();
  });

  it('sets and clears the manual platform through the case', async () => {
    report.value = { platform: 'android', platforms: ['android'], display_platform_name: 'Android', evidence_type: 'compressfile', is_backup: false, manufactor: null } as AutoDetectReport;
    activeCase.value = caseWith({ platform: 'android' });
    mount();
    expect(selectedChoice()).toBe('自动识别');
    await choose('Linux');
    await vi.waitFor(() => expect(caseMock.refreshActive).toHaveBeenCalledTimes(1));
    expect(caseMock.setPlatform).toHaveBeenCalledWith({ imageId: 'disk-one', platform: 'linux' });

    activeCase.value = caseWith({ platform: 'linux', manual_platform: 'linux' });
    await nextTick();
    await choose('自动识别');
    await vi.waitFor(() => expect(caseMock.refreshActive).toHaveBeenCalledTimes(2));
    expect(caseMock.setPlatform).toHaveBeenLastCalledWith({ imageId: 'disk-one', platform: 'auto' });
    expect(caseMock.notifyError).not.toHaveBeenCalled();
  });

  it('reports a refused choice and keeps the stored platform', async () => {
    caseMock.setPlatform.mockRejectedValue(new Error('case is locked; unlock it before editing'));
    report.value = { platform: 'android', platforms: ['android'], display_platform_name: 'Android', evidence_type: 'disk', is_backup: false, manufactor: null } as AutoDetectReport;
    activeCase.value = caseWith({ platform: 'android' });
    mount();
    await choose('iOS');
    await vi.waitFor(() => expect(caseMock.notifyError).toHaveBeenCalledWith('手动指定平台失败', 'Error: case is locked; unlock it before editing'));
    expect(caseMock.refreshActive).not.toHaveBeenCalled();
    await nextTick();
    expect(selectedChoice()).toBe('自动识别');
  });

  it('locks the picker with the case', () => {
    report.value = { platform: 'linux', platforms: ['linux'], display_platform_name: 'Linux', evidence_type: 'disk', is_backup: false, manufactor: null } as AutoDetectReport;
    activeCase.value = caseWith({ platform: 'linux' }, true);
    mount();
    expect(host.querySelector<HTMLButtonElement>('.platform-choice .ui-select')?.disabled).toBe(true);
    expect(host.querySelector('.platform-choice')?.getAttribute('title')).toBe('案件已锁定，解锁后才能修改平台');
  });
});
