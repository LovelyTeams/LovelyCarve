// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, reactive, type App } from 'vue';
import VimSwapViewer from './VimSwapViewer.vue';
import type { VimSwapDigest } from '../../../../lib/ipc/forensicPreview';

const mocks = vi.hoisted(() => ({
  preview: vi.fn(),
  copyText: vi.fn(),
}));

vi.mock('../../../../lib/ipc/forensicPreview', () => ({
  fsVimSwapPreview: mocks.preview,
}));
vi.mock('../../../../composables/clipboard', () => ({
  copyText: mocks.copyText,
}));

let app: App;
let host: HTMLDivElement;
let props: { imageId: string; nodeId: string; query?: string };

async function settle() {
  for (let i = 0; i < 8; i++) await nextTick();
}

async function mount() {
  app = createApp({ render: () => h(VimSwapViewer, props) });
  app.mount(host);
  await settle();
}

function sampleDigest(overrides: Partial<VimSwapDigest> = {}): VimSwapDigest {
  return {
    meta: {
      version: 'VIM 9.1',
      page_size: 4096,
      mtime: 1_700_000_000,
      mtime_iso: '2023-11-14T22:13:20Z',
      inode: 42,
      pid: 1234,
      user: 'alice',
      host: 'workstation',
      fname: '/etc/hosts',
    },
    lines: ['127.0.0.1 localhost', 'secret-token=hunter2', '::1 localhost'],
    bytes_read: 8192,
    truncated: false,
    data_block_count: 1,
    pointer_block_count: 1,
    unknown_block_count: 0,
    encrypted: false,
    notes: [],
    ...overrides,
  };
}

beforeEach(() => {
  vi.resetAllMocks();
  props = reactive({ imageId: 'img-1', nodeId: 'node-swp', query: '' });
  host = document.createElement('div');
  document.body.append(host);
});

afterEach(() => {
  app?.unmount();
  host.remove();
});

describe('VimSwapViewer', () => {
  it('loads digest via fsVimSwapPreview, renders metadata, filters highlights, and copies recovered lines', async () => {
    mocks.preview.mockResolvedValue(sampleDigest());
    await mount();

    expect(mocks.preview).toHaveBeenCalledExactlyOnceWith({ imageId: 'img-1', nodeId: 'node-swp' });
    expect(host.querySelector('.vsw-fname')?.textContent).toBe('/etc/hosts');
    expect(host.querySelectorAll('.vsw-line')).toHaveLength(3);

    props.query = 'hunter2';
    await settle();
    expect(host.querySelectorAll('.vsw-line')).toHaveLength(1);
    expect(host.querySelector('.vsw-mark')?.textContent).toBe('hunter2');

    host.querySelector<HTMLButtonElement>('.vsw-copy')!.click();
    await settle();
    expect(mocks.copyText).toHaveBeenCalledWith(
      '127.0.0.1 localhost\nsecret-token=hunter2\n::1 localhost',
      expect.any(String),
    );
  });

  it('shows encryption warning badge and alert when swap blocks are encrypted', async () => {
    mocks.preview.mockResolvedValue(sampleDigest({ lines: [], encrypted: true }));
    await mount();

    expect(host.querySelector('.vsw-alert')).not.toBeNull();
    expect(host.querySelector<HTMLButtonElement>('.vsw-copy')?.disabled).toBe(true);
  });
});
