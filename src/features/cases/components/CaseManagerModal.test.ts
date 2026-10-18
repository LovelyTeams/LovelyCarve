// @vitest-environment happy-dom
import { createApp, h, nextTick, ref, shallowRef, type App } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import CaseManagerModal from './CaseManagerModal.vue';
import type { CaseInfo, SnapshotProgress } from '../../../lib/ipc/cases';
import type { ImageInfo } from '../../../lib/ipc/image';

const mocked = vi.hoisted(() => ({ session: vi.fn() }));
vi.mock('../composables/useCaseSession', () => ({ useCaseSession: mocked.session }));
vi.mock('../../raid/api', () => ({ useRaidOverview: () => ({ stack: { value: null } }) }));
const caseInfo: CaseInfo = { id: 'case', name: '示例案件', dir: 'D:\\cases\\demo', case_number: 'CASE-001', examiner: '', description: '', locked: false, schema_version: 1, created_at: '', updated_at: '2025-12-31T23:59:00', evidence: [], snapshots: [] };
const imageInfo = { source_format: 'folder', partitions: [] } as unknown as ImageInfo;
let app: App;
let host: HTMLElement;
let session: ReturnType<typeof makeSession>;
function makeSession() {
  return { activeCase: shallowRef<CaseInfo | null>(caseInfo), cases: ref([]), snapshots: ref([]), buildProgress: ref(new Map<string, SnapshotProgress>()), busy: ref(false), registryLoading: ref(false), registryError: ref(''), pickDirectory: vi.fn(), createCase: vi.fn(), openCase: vi.fn(), closeCase: vi.fn(), setLocked: vi.fn(), removeCase: vi.fn(), buildSnapshot: vi.fn(), cancelBuild: vi.fn(), removeEvidence: vi.fn(), refreshActive: vi.fn().mockResolvedValue(undefined), refreshCases: vi.fn().mockResolvedValue(undefined) };
}
async function settle() { for (let index = 0; index < 6; index++) await nextTick(); }
beforeEach(() => { session = makeSession(); mocked.session.mockReturnValue(session); host = document.createElement('div'); document.body.append(host); });
afterEach(() => { app?.unmount(); host.remove(); });
function button(label: string) { return [...host.querySelectorAll<HTMLButtonElement>('button')].find(element => element.textContent?.trim() === label)!; }
async function mount(onClose = vi.fn()) { app = createApp(CaseManagerModal, { visible: true, imageId: 'image', imageInfo, onClose }); app.mount(host); await settle(); }

describe('case manager feedback and focus', () => {
  it('moves focus into the dialog and restores it to the opener on close', async () => {
    const visible = ref(false);
    app = createApp({ render: () => [h('button', { id: 'opener', onClick: () => { visible.value = true; } }, '案件'), h(CaseManagerModal, { visible: visible.value, imageId: '', imageInfo: null, onClose: () => { visible.value = false; } })] });
    app.mount(host); const opener = host.querySelector<HTMLButtonElement>('#opener')!;
    opener.focus(); opener.click(); await settle();
    expect(document.activeElement).toBe(host.querySelector('[role="dialog"]'));
    host.querySelector('[role="dialog"]')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await settle();
    expect(visible.value).toBe(false); expect(document.activeElement).toBe(opener);
  });

  it('prevents dismissal while opening and reports failures inside the dialog', async () => {
    let fail!: (cause: Error) => void;
    session.pickDirectory.mockResolvedValue('D:\\cases\\new');
    session.openCase.mockReturnValue(new Promise((_resolve, reject) => { fail = reject; }));
    const close = vi.fn(); await mount(close); button('切换案件').click(); await settle();
    expect(button('完成').disabled).toBe(true);
    host.querySelector('[role="dialog"]')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(close).not.toHaveBeenCalled();
    fail(new Error('目录不可读')); await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('目录不可读');
    expect(button('完成').disabled).toBe(false);
  });

  it('reports snapshot submission and cancellation failures and lets the user retry', async () => {
    await mount(); host.querySelector<HTMLDetailsElement>('.section-fold')!.open = true;
    session.buildSnapshot.mockRejectedValueOnce(new Error('检材未载入')); button('构建').click(); await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('检材未载入');
    expect(button('构建').disabled).toBe(false);
    session.buildProgress.value = new Map([['image:root', { entries: 20, phase: 'walking' } as SnapshotProgress]]); await settle();
    session.cancelBuild.mockRejectedValueOnce(new Error('任务已结束')); button('取消').click(); await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('任务已结束');
  });

  it('shows a registry error with retry and labels case timestamps as local time', async () => {
    await mount(); host.querySelectorAll<HTMLDetailsElement>('.section-fold')[1].open = true;
    expect(host.querySelector('.case-properties')?.textContent).toContain('2025-12-31 23:59:00（本地）');
    session.activeCase.value = null; session.registryError.value = '读取案件列表失败'; await settle();
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('读取案件列表失败');
    button('重试').click(); await settle(); expect(session.refreshCases).toHaveBeenCalledTimes(2);
  });

  it('keeps a different image usable while an older snapshot submission is pending', async () => {
    let fail!: (cause: Error) => void;
    session.buildSnapshot.mockReturnValueOnce(new Promise((_resolve, reject) => { fail = reject; }));
    const imageId = ref('image');
    app = createApp({ render: () => h(CaseManagerModal, { visible: true, imageId: imageId.value, imageInfo }) });
    app.mount(host); await settle(); button('构建').click(); await settle();
    imageId.value = 'other'; await settle(); expect(button('构建').disabled).toBe(false);
    fail(new Error('旧检材不可用')); await settle(); expect(host.querySelector('[role="alert"]')).toBeNull();
  });
});
