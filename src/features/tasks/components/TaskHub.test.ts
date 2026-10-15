// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue';
import type { EvidenceRef } from '../../../lib/ipc/cases';
import TaskHub from './TaskHub.vue';
import { type TaskHubTab } from '../model/types';

const calls = vi.hoisted(() => ({ resetFilters: vi.fn(), openPanel: vi.fn(), openCaseManager: vi.fn(), close: vi.fn() }));
// `__esModule` lets defineAsyncComponent unwrap the stub's default export.
vi.mock('./TaskCenterPanel.vue', () => ({
  __esModule: true,
  default: defineComponent({
    props: { imageId: String },
    emits: ['openResult', 'quickAnalysis'],
    setup(props, { emit, expose }) {
      expose({ resetFilters: calls.resetFilters });
      return () => h('button', { class: 'stub-open', 'data-image-id': props.imageId, onClick: () => emit('openResult', 'analysis', 'browser') }, '任务列表');
    },
  }),
}));
vi.mock('./QuickAnalysisPanel.vue', () => ({
  __esModule: true,
  default: defineComponent({
    props: { imageId: String },
    emits: ['submitted'],
    setup(props, { emit }) {
      return () => h('button', { class: 'stub-submit', onClick: () => emit('submitted') }, `快速分析 ${props.imageId}`);
    },
  }),
}));

const evidence: EvidenceRef = { image_id: 'disk-a', path: 'D:\\Evidence\\WIN10-PC.E01', source_format: 'ewf', size_bytes: 1, added_at: '', platform: 'windows', evidence_type: 'disk' };
const tab = ref<TaskHubTab>('tasks');
const current = ref<EvidenceRef | undefined>(evidence);
const hasCaseWithoutImage = ref(false);
const sessionImage = ref<{ id: string; path: string } | null>(null);
let app: App | undefined;
let host: HTMLDivElement;

async function settle() {
  for (let index = 0; index < 6; index++) {
    await nextTick();
    await new Promise(resolve => setTimeout(resolve, 0));
  }
}
async function mount() {
  app = createApp({
    render: () => h(TaskHub, {
      open: true, tab: tab.value, evidence: current.value, sessionImage: sessionImage.value,
      evidenceName: current.value ? 'WIN10-PC.E01' : '', hasCase: Boolean(current.value) || hasCaseWithoutImage.value, activeCount: 2,
      'onUpdate:tab': (value: TaskHubTab) => { tab.value = value; },
      onOpenPanel: calls.openPanel, onOpenCaseManager: calls.openCaseManager, onClose: calls.close,
    }),
  });
  app.mount(host);
  await settle();
}
const panelVisible = (id: string) => (host.querySelector<HTMLElement>(`#${id}`)?.style.display ?? 'none') !== 'none';

beforeEach(() => {
  vi.clearAllMocks();
  tab.value = 'tasks';
  current.value = evidence;
  hasCaseWithoutImage.value = false;
  sessionImage.value = null;
  host = document.createElement('div');
  document.body.append(host);
});
afterEach(() => { app?.unmount(); app = undefined; host.remove(); });

describe('task hub', () => {
  it('opens the task list while the first evidence is still extracting', async () => {
    current.value = undefined;
    hasCaseWithoutImage.value = true;
    await mount();
    expect(panelVisible('task-list-panel')).toBe(true);
    expect(host.querySelector('.stub-open')).not.toBeNull();
  });
  it('names the current evidence and counts active work on the task tab', async () => {
    await mount();
    expect(host.querySelector('.hub-evidence')?.textContent).toBe('WIN10-PC.E01');
    expect(host.querySelector('.hub-evidence')?.getAttribute('title')).toContain('D:\\Evidence\\WIN10-PC.E01');
    expect(host.querySelector('#task-list-tab')?.textContent).toBe('任务2');
    expect(panelVisible('task-list-panel')).toBe(true);
    expect(panelVisible('quick-analysis-panel')).toBe(false);
  });

  it('moves between tabs with the arrow keys and returns to the list after a submission', async () => {
    await mount();
    host.querySelector('#task-list-tab')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    await settle();
    expect(tab.value).toBe('quick');
    expect(host.querySelector('#quick-analysis-tab')?.getAttribute('aria-selected')).toBe('true');
    expect(panelVisible('quick-analysis-panel')).toBe(true);
    (host.querySelector('.stub-submit') as HTMLButtonElement).click();
    await settle();
    expect(calls.resetFilters).toHaveBeenCalledOnce();
    expect(tab.value).toBe('tasks');
    host.querySelector('#task-list-tab')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true }));
    await settle();
    expect(tab.value).toBe('quick');
  });

  it('forwards result links and closes on Escape', async () => {
    await mount();
    (host.querySelector('.stub-open') as HTMLButtonElement).click();
    expect(calls.openPanel).toHaveBeenCalledWith('analysis', 'browser');
    host.querySelector('#task-hub')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(calls.close).toHaveBeenCalledOnce();
  });

  it('offers the case manager when no evidence is open', async () => {
    current.value = undefined;
    await mount();
    expect(host.querySelector('.hub-evidence')).toBeNull();
    expect(host.querySelector('#task-list-panel')).toBeNull();
    expect(host.textContent).toContain('打开案件，开始分析');
    (host.querySelector('.hub-empty .ui-btn') as HTMLButtonElement).click();
    expect(calls.openCaseManager).toHaveBeenCalledOnce();
  });

  it('shows tasks for a loaded image without a case and keeps quick analysis gated', async () => {
    current.value = undefined;
    sessionImage.value = { id: 'loose-image', path: 'D:/Evidence/server.E01' };
    await mount();
    expect(host.querySelector('.hub-evidence')?.textContent).toBe('server.E01');
    expect(host.querySelector('.stub-open')?.getAttribute('data-image-id')).toBe('loose-image');
    expect(panelVisible('task-list-panel')).toBe(true);
    tab.value = 'quick';
    await settle();
    expect(host.querySelector('.stub-submit')).toBeNull();
    expect(host.querySelector('#quick-analysis-panel')?.textContent).toContain('选择案件检材');
  });
});
