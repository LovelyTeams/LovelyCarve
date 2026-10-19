// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, shallowRef, type App } from 'vue';
import type { AutoDetectReport } from '../../../lib/ipc/autodetect';

const detect = vi.hoisted(() => ({ run: vi.fn() }));
const report = shallowRef<AutoDetectReport | undefined>();
const pending = shallowRef(false);
const failure = shallowRef<string | null>(null);
vi.mock('../composables/useAutoDetect', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../composables/useAutoDetect')>()),
  runAutoDetect: detect.run,
  useAutoDetectReport: () => report,
  useAutoDetectPending: () => pending,
  useAutoDetectError: () => failure,
}));
vi.mock('../../tasks/api', () => ({ useTaskEntries: () => ({ entries: shallowRef([]) }) }));
import AutoDetectBadge from './AutoDetectBadge.vue';

let app: App;
let host: HTMLDivElement;

function mount(action?: boolean) {
  host = document.createElement('div');
  document.body.append(host);
  app = createApp({ render: () => h(AutoDetectBadge, { imageId: 'disk-one' }, action ? { default: () => h('button', '导出源镜像') } : undefined) });
  app.mount(host);
}

beforeEach(() => {
  vi.clearAllMocks();
  report.value = undefined;
  pending.value = false;
  failure.value = null;
});
afterEach(() => { app.unmount(); host.remove(); });

describe('platform badge', () => {
  it.each(['backup', 'XiaomiBackup'])('shows one backup conclusion for %s without repeating the manufacturer or matching rule', evidenceType => {
    const description = 'Android · Xiaomi · 关键词匹配备份';
    report.value = { platform: 'android', platforms: ['android'], display_platform_name: description,
      evidence_type: evidenceType, is_backup: true, manufactor: 'xiaomi' } as AutoDetectReport;
    mount();
    const chip = host.querySelector('.platform-chip')!;
    expect(chip.querySelector('.platform-name')?.textContent).toBe('Android');
    expect(chip.querySelector('.platform-type')?.textContent).toBe('· Xiaomi · 备份');
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
