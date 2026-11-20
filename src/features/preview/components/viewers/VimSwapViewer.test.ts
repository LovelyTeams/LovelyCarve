// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, type App } from 'vue';
import type { VimSwapDigest } from '../../../../lib/ipc/vimSwap';
import VimSwapViewer from './VimSwapViewer.vue';

const mocks = vi.hoisted(() => ({ preview: vi.fn(), copyText: vi.fn() }));
vi.mock('../../../../lib/ipc/vimSwap', () => ({ fsVimSwapPreview: mocks.preview }));
vi.mock('../../../../composables/clipboard', () => ({ copyText: mocks.copyText }));
// CodeMirror stays out of the test: the stub shows what the viewer hands it.
vi.mock('./CodeViewer.vue', async () => {
  const { defineComponent, h: render } = await import('vue');
  return {
    __esModule: true,
    default: defineComponent({
      props: { code: { type: String, default: '' }, filename: { type: String, default: '' }, dark: Boolean, query: { type: String, default: '' } },
      setup: props => () => render('pre', { class: 'code-stub', 'data-filename': props.filename, 'data-query': props.query }, props.code),
    }),
  };
});

let app: App | null = null;
let host: HTMLDivElement;

async function settle() {
  for (let i = 0; i < 10; i++) await nextTick();
  await new Promise(resolve => setTimeout(resolve, 0));
  for (let i = 0; i < 4; i++) await nextTick();
}

async function mount() {
  app = createApp({ render: () => h(VimSwapViewer, { imageId: 'img-1', nodeId: 'node-swp' }) });
  app.mount(host);
  await settle();
}

function digest(overrides: Partial<VimSwapDigest> = {}): VimSwapDigest {
  return {
    meta: {
      version: 'VIM 9.1', page_size: 4096, mtime: 1_700_000_000, mtime_iso: '2023-11-14T22:13:20+00:00',
      inode: 42, pid: 1234, user: 'alice', host: 'workstation', fname: '/root/.env',
    },
    lines: ['DB_HOST=127.0.0.1', 'DB_PASSWORD=hunter2'],
    bytes_read: 8192, truncated: false, data_block_count: 1, pointer_block_count: 1, unknown_block_count: 0,
    encrypted: false, notes: [],
    ...overrides,
  };
}

const fields = () => Object.fromEntries([...host.querySelectorAll('.report-fields > div')]
  .map(row => [row.querySelector('dt')?.textContent, row.querySelector('dd')?.textContent]));

beforeEach(() => {
  vi.resetAllMocks();
  host = document.createElement('div');
  document.body.append(host);
});

afterEach(() => {
  app?.unmount();
  app = null;
  host.remove();
});

describe('VimSwapViewer', () => {
  it('shows the editing session and hands the recovered lines to the code view', async () => {
    mocks.preview.mockResolvedValue(digest({ notes: ['已提取 20000 行，后续 data block 不再展开'] }));
    await mount();

    expect(mocks.preview).toHaveBeenCalledExactlyOnceWith({ imageId: 'img-1', nodeId: 'node-swp' });
    expect(host.querySelector('.report-name')?.textContent).toBe('/root/.env');
    expect([...host.querySelectorAll('.report-badges .ui-badge')].map(badge => badge.textContent)).toEqual(['VIM 9.1']);
    expect(fields()).toMatchObject({ 用户: 'alice', 主机: 'workstation', PID: '1234', inode: '42', 修改时间: '2023-11-14 22:13:20 UTC' });
    expect(host.querySelector('.report-warnings .warn-line')?.textContent).toBe('已提取 20000 行，后续 data block 不再展开');
    const code = host.querySelector<HTMLElement>('.code-stub')!;
    expect(code.textContent).toBe('DB_HOST=127.0.0.1\nDB_PASSWORD=hunter2');
    expect(code.dataset.filename).toBe('/root/.env');

    const search = host.querySelector<HTMLInputElement>('.section-filter input')!;
    search.value = 'hunter2';
    search.dispatchEvent(new Event('input'));
    await settle();
    expect(host.querySelector<HTMLElement>('.code-stub')?.dataset.query).toBe('hunter2');

    host.querySelector<HTMLButtonElement>('.section-head .ui-icon-btn')!.click();
    expect(mocks.copyText).toHaveBeenCalledWith('DB_HOST=127.0.0.1\nDB_PASSWORD=hunter2', '恢复的行');
  });

  it('marks a swap file whose data blocks are missing and offers nothing to copy', async () => {
    mocks.preview.mockResolvedValue(digest({ lines: [], data_block_count: 0, encrypted: true, truncated: true, meta: { ...digest().meta, fname: '', mtime: 0, mtime_iso: null } }));
    await mount();

    expect(host.querySelector('.report-name')?.textContent).toBe('未记录原文件名');
    expect([...host.querySelectorAll('.report-badges .ui-badge')].map(badge => badge.textContent)).toEqual(['VIM 9.1', '疑似加密', '已截断']);
    expect(fields()['修改时间']).toBe('—');
    expect(host.querySelector('.code-stub')).toBeNull();
    expect(host.querySelector('.section-head .ui-icon-btn')).toBeNull();
    expect(host.querySelector('.lines .empty-row')?.textContent).toBe('没有恢复出文本行');
  });

  it('reports a file the backend cannot parse', async () => {
    mocks.preview.mockRejectedValue('not a vim swap file (missing b0VIM magic)');
    await mount();

    expect(host.querySelector('.preview-state.error .state-title')?.textContent).toBe('无法解析 Vim 交换文件');
    expect(host.querySelector('.preview-state.error .state-desc')?.textContent).toBe('not a vim swap file (missing b0VIM magic)');
  });
});
