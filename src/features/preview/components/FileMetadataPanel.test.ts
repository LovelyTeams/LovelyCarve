// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, reactive, type App } from 'vue';
import FileMetadataPanel from './FileMetadataPanel.vue';
import type { FsNode } from '../../../lib/ipc/fs/types';

const mocks = vi.hoisted(() => ({
  fsStreams: vi.fn(),
  fsStreamPreview: vi.fn(),
  fsStreamExport: vi.fn(),
  saveDialog: vi.fn(),
  notifySuccess: vi.fn(),
  notifyError: vi.fn(),
}));

vi.mock('../../../lib/ipc/fs/streams', () => ({
  fsStreams: mocks.fsStreams,
  fsStreamPreview: mocks.fsStreamPreview,
  fsStreamExport: mocks.fsStreamExport,
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  save: mocks.saveDialog,
}));
vi.mock('../../../composables/useToast', () => ({
  notifySuccess: mocks.notifySuccess,
  notifyError: mocks.notifyError,
}));

let app: App;
let host: HTMLDivElement;
let props: {
  imageId?: string;
  indexOnly?: boolean;
  node: FsNode;
  fullPath: string;
  extension: string;
  ident: null;
  digest: null;
  extMismatch: boolean;
};

async function settle() {
  for (let i = 0; i < 8; i++) await nextTick();
}

async function mount() {
  app = createApp({ render: () => h(FileMetadataPanel, props) });
  app.mount(host);
  await settle();
}

beforeEach(() => {
  vi.resetAllMocks();
  props = reactive({
    imageId: 'img-ntfs',
    indexOnly: false,
    node: {
      id: 'ntfs:42',
      name: 'payload.exe',
      kind: 'file',
      parent_id: 'root',
      size: 1024,
      deleted: false,
      inode: 42,
      timestamps: { created: null, modified: null, accessed: null, mft_changed: null, birth: null },
    },
    fullPath: '/Users/alice/payload.exe',
    extension: 'exe',
    ident: null,
    digest: null,
    extMismatch: false,
  });
  host = document.createElement('div');
  document.body.append(host);
});

afterEach(() => {
  app?.unmount();
  host.remove();
});

describe('FileMetadataPanel NTFS ADS', () => {
  it('lists alternate data streams, previews UTF-8 text and binary hex fallback, and exports to selected path', async () => {
    mocks.fsStreams.mockResolvedValue([
      { name: 'Zone.Identifier', size: 32, resident: true },
      { name: 'hidden.bin', size: 4, resident: false },
    ]);
    mocks.fsStreamPreview
      .mockResolvedValueOnce(Array.from(new TextEncoder().encode('[ZoneTransfer]\nZoneId=3')))
      .mockResolvedValueOnce([0x00, 0xfe, 0xed, 0xfa]);
    mocks.saveDialog.mockResolvedValue('C:\\export\\payload.exe_Zone.Identifier.bin');
    mocks.fsStreamExport.mockResolvedValue(32);

    await mount();

    expect(mocks.fsStreams).toHaveBeenCalledExactlyOnceWith({ imageId: 'img-ntfs', nodeId: 'ntfs:42' });
    const items = host.querySelectorAll('.ads-item');
    expect(items).toHaveLength(2);
    expect(items[0].querySelector('.ads-stream-name')?.textContent).toBe(':Zone.Identifier');

    const firstButtons = items[0].querySelectorAll<HTMLButtonElement>('button');
    firstButtons[0].click();
    await settle();
    expect(mocks.fsStreamPreview).toHaveBeenCalledWith({
      imageId: 'img-ntfs',
      nodeId: 'ntfs:42',
      streamName: 'Zone.Identifier',
      maxBytes: 65536,
    });
    expect(items[0].querySelector('.ads-preview')?.textContent).toContain('ZoneId=3');

    const secondButtons = items[1].querySelectorAll<HTMLButtonElement>('button');
    secondButtons[0].click();
    await settle();
    expect(items[1].querySelector('.ads-preview')?.textContent).toBe('00 fe ed fa');

    firstButtons[1].click();
    await settle();
    expect(mocks.fsStreamExport).toHaveBeenCalledWith({
      imageId: 'img-ntfs',
      nodeId: 'ntfs:42',
      streamName: 'Zone.Identifier',
      destination: 'C:\\export\\payload.exe_Zone.Identifier.bin',
    });
    expect(mocks.notifySuccess).toHaveBeenCalled();
  });

  it('skips fsStreams in index-only mode', async () => {
    props.indexOnly = true;
    await mount();
    expect(mocks.fsStreams).not.toHaveBeenCalled();
    expect(host.querySelectorAll('.ads-item')).toHaveLength(0);
  });
});
