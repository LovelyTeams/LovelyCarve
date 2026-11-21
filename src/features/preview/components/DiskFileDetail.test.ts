// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, ref, type App } from 'vue';
import DiskFileDetail from './DiskFileDetail.vue';
import type { FsNode } from '../../../lib/ipc/fs/types';
import type { FilePreviewReadingState } from '../../../lib/models/filePreviewReadingState';

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('../../../lib/ipc/client', () => ({ invokeCmd: ipc.invoke }));
vi.mock('../../../lib/ipc/events', () => ({ onEvent: async () => () => {} }));
vi.mock('../../../composables/useTheme', () => ({ useTheme: () => ({ currentTheme: ref('light') }) }));
const node = ref<FsNode>({ id: 'markdown', name: 'notes.md', kind: 'file', parent_id: 'root', size: 60, deleted: false,
  inode: 42, timestamps: { created: null, modified: null, accessed: null, mft_changed: null, birth: null } });
const content = '**needle** needle <img src=x onerror=alert(1)>';
let app: App;
let host: HTMLDivElement;
async function settle() { for (let index = 0; index < 15; index++) await nextTick(); }
beforeEach(() => {
  vi.resetAllMocks(); node.value = { ...node.value, id: 'markdown', name: 'notes.md', kind: 'file' };
  ipc.invoke.mockImplementation(async command => {
    if (command === 'fs_preview') return new TextEncoder().encode(content).buffer;
    if (command === 'fs_preview_text_encoding') return { encoding: 'UTF-8', from_bom: false, had_errors: false, continues: false };
    return {};
  });
  host = document.createElement('div'); document.body.append(host);
});
afterEach(() => { app?.unmount(); host.remove(); vi.unstubAllGlobals(); });
async function mount(readingState?: FilePreviewReadingState, indexOnly = false) { app = createApp({ render: () => h(DiskFileDetail, { indexOnly, imageId: 'image-a', node: node.value, fullPath: '/notes.md', digest: { md5: 'known-md5' }, readingState }) }); app.mount(host); await settle(); }
function tab(label: string) { [...host.querySelectorAll<HTMLButtonElement>('[role="tab"]')].find(button => button.textContent === label)!.click(); }
async function openFind() { host.querySelector<HTMLButtonElement>('[aria-label="查找"]')!.click(); await settle(); }

describe('preview content composition', () => {
  it('preserves the offline attribute reading position when docking or detaching', async () => {
    await mount({ imageId: 'image-a', nodeId: 'markdown', tab: 'detail', searchQuery: '', encoding: '', page: 1, pageSize: 2048, currentMatch: -1, scrollTop: 90, scrollLeft: 0, codeScrollTop: 0, codeScrollLeft: 0 }, true);
    expect(host.querySelector('.preview-body')?.scrollTop).toBe(90); expect(ipc.invoke).not.toHaveBeenCalled();
  });
  it('shows saved attributes offline and disables source views even with an old text reading state', async () => {
    await mount({ imageId: 'image-a', nodeId: 'markdown', tab: 'text', searchQuery: 'needle', encoding: '', page: 1, pageSize: 2048, currentMatch: -1, scrollTop: 0, scrollLeft: 0, codeScrollTop: 0, codeScrollLeft: 0 }, true);
    expect(host.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe('详细信息');
    expect(host.textContent).toContain('/notes.md'); expect(host.textContent).toContain('known-md5');
    expect([...host.querySelectorAll<HTMLButtonElement>('[role="tab"]')].filter(button => button.disabled)).toHaveLength(3);
    expect(host.querySelector('[aria-label="查找"]')).toBeNull(); expect(host.textContent).toContain('离线索引');
    expect(ipc.invoke).not.toHaveBeenCalled();
    node.value = { ...node.value, id: 'next-file' }; await settle();
    expect(host.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe('详细信息'); expect(ipc.invoke).not.toHaveBeenCalled();
  });
  it('restores the saved text view, query and reading position after decoding', async () => {
    await mount({ imageId: 'image-a', nodeId: 'markdown', tab: 'text', searchQuery: 'needle', encoding: '', page: 1, pageSize: 2048, currentMatch: -1, scrollTop: 90, scrollLeft: 12, codeScrollTop: 0, codeScrollLeft: 0 });
    expect(host.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe('文本');
    expect(host.querySelector<HTMLInputElement>('[aria-label="搜索预览内容"]')?.value).toBe('needle');
    expect(host.querySelectorAll('.text-view mark')).toHaveLength(2);
    expect([host.querySelector('.preview-body')?.scrollTop, host.querySelector('.preview-body')?.scrollLeft]).toEqual([90, 12]);
  });
  it('pages hexadecimal bytes through the status bar', async () => {
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => command === 'fs_preview' ? new Uint8Array(8192).buffer : original(command, args));
    node.value = { ...node.value, name: 'bytes.bin', size: 8192 }; await mount(); tab('十六进制'); await settle();
    expect(host.querySelector('.preview-statusbar .page-total')?.textContent).toContain('/ 4');
    expect(host.querySelector<HTMLButtonElement>('[aria-label="上一页"]')?.disabled).toBe(true);
    host.querySelector<HTMLButtonElement>('[aria-label="下一页"]')!.click(); await settle();
    expect(host.querySelector<HTMLInputElement>('[aria-label="当前页"]')?.value).toBe('2');
    expect(host.querySelector('.hex-row:not(.hex-header-row) .hex-offset')?.textContent).toBe('0800h');
    expect(host.querySelector<HTMLButtonElement>('[aria-label="上一页"]')?.disabled).toBe(false);
  });
  it('binds the media child image/OCR elements to their recognition owner', async () => {
    vi.stubGlobal('URL', class extends URL {
      static createObjectURL = vi.fn(() => 'blob:preview');
      static revokeObjectURL = vi.fn();
    });
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => {
      if (command === 'fs_read') return new Uint8Array([1, 2]).buffer;
      if (command === 'ocr_scan') return { regions: [{ text: 'image text', confidence: 1, points: [[5, 5], [15, 5], [15, 15]] }], width: 100, height: 50, count: 1 };
      return original(command, args);
    });
    node.value = { ...node.value, name: 'picture.png' }; await mount();
    const image = host.querySelector<HTMLImageElement>('.media-view')!;
    for (const [key, value] of Object.entries({ naturalWidth: 100, naturalHeight: 50, clientWidth: 200, clientHeight: 100 })) Object.defineProperty(image, key, { value });
    image.dispatchEvent(new Event('load')); await settle();
    [...host.querySelectorAll<HTMLButtonElement>('.preview-viewbar button')].find(button => button.textContent?.includes('识别文字'))!.click(); await settle();
    expect(host.querySelector('.preview-statusbar')?.textContent).toContain('100 × 50 px');
    expect(host.querySelector('.ocr-overlay')?.getAttribute('viewBox')).toBe('0 0 100 50');
    expect(host.querySelector<HTMLElement>('.ocr-text-span')?.style.left).toBe('10px');
    expect(host.querySelector('.ocr-text-block')?.textContent).toBe('image text');
    app.unmount(); expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:preview');
  });

  it('renders safe Markdown and navigates its hits through the find bar', async () => {
    await mount();
    expect(host.querySelector('[aria-label="搜索预览内容"]')).toBeNull();
    await openFind();
    const input = host.querySelector<HTMLInputElement>('[aria-label="搜索预览内容"]')!;
    input.value = 'needle'; input.dispatchEvent(new Event('input', { bubbles: true })); await settle();
    const marks = host.querySelectorAll<HTMLElement>('.md-body mark'); expect(marks).toHaveLength(2);
    marks.forEach(mark => { mark.scrollIntoView = vi.fn(); });
    host.querySelector<HTMLButtonElement>('[aria-label="下一个匹配"]')!.click(); await settle();
    expect(marks[0].classList.contains('current')).toBe(true);
    expect(host.querySelector('.match-count')?.textContent).toContain('1/2'); expect(host.querySelector('.md-body img')).toBeNull();
  });

  it('finds and navigates SQLite cell text and resets the selected hit on page changes', async () => {
    const columns = [{ name: 'body', type_name: 'TEXT', notnull: false, default_value: null, pk: 0 }];
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => {
      if (command === 'fs_sqlite_preview') return {
        tables: [{ name: 'messages', kind: 'table', columns, row_count: 101 }], sidecars: [],
      };
      if (command === 'fs_sqlite_table_rows') return {
        columns, total: 101, offset: args.offset,
        rows: [[{ kind: 'text', value: args.offset ? '每天、每天' : '每天看 <img src=x>，每天写' }]],
      };
      return original(command, args);
    });
    node.value = { ...node.value, id: 'database', name: 'messages.db' };
    await mount();
    tab('原类型');
    await vi.dynamicImportSettled();
    await settle();
    expect(host.querySelector('.preview-statusbar')?.textContent).toContain('1–1 / 101');

    await openFind();
    const input = host.querySelector<HTMLInputElement>('[aria-label="搜索当前页"]')!;
    input.value = '每天';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await settle();
    const marks = [...host.querySelectorAll<HTMLElement>('.db-table mark')];
    expect(marks.map(mark => mark.textContent)).toEqual(['每天', '每天']);
    expect(host.querySelector('.db-table img')).toBeNull();
    marks.forEach(mark => { mark.scrollIntoView = vi.fn(); });
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', shiftKey: true, bubbles: true }));
    await settle();
    expect(host.querySelector('mark.current')).toBe(marks[1]);
    expect(marks[1].scrollIntoView).toHaveBeenCalledOnce();
    expect(host.querySelector('.match-count')?.textContent).toContain('2/2');

    host.querySelector<HTMLButtonElement>('[aria-label="下一页"]')!.click();
    await settle();
    expect(input.value).toBe('每天');
    expect(host.querySelector('.match-count')?.textContent).toContain('0/2');
    expect(host.querySelector('mark.current')).toBeNull();
    expect(host.querySelector('.db-table tbody td')?.textContent).toBe('101');

    input.value = 'missing';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await settle();
    expect(host.querySelector('.match-count')?.textContent).toContain('本页无匹配');
    expect(host.querySelector<HTMLButtonElement>('[aria-label="下一个匹配"]')?.disabled).toBe(true);
  });
  it('renders metadata with available hashes while keeping window actions hidden in standalone mode', async () => {
    await mount(); tab('详细信息'); await settle();
    expect(host.textContent).toContain('/notes.md'); expect(host.textContent).toContain('known-md5');
    expect(host.textContent).not.toContain('SHA256'); expect(host.querySelector('[aria-label="在独立窗口打开"]')).toBeNull();
  });
  it('runs bound SQL after a schema preview failure and preserves large integer results', async () => {
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => {
      if (command === 'fs_sqlite_preview') throw new Error('SQLite preview exceeded its execution budget');
      if (command === 'fs_sqlite_query') return {
        columns: ['id'], rows: [[{ kind: 'integer', value: '9223372036854775807' }]],
        offset: args.offset, nextOffset: args.offset === 0 ? 50 : null, shortenedCells: false,
        sidecars: ['messages.db-wal'],
      };
      return original(command, args);
    });
    node.value = { ...node.value, id: 'database', name: 'messages.db' };
    await mount();
    tab('原类型');
    await vi.dynamicImportSettled();
    await settle();
    expect(host.textContent).toContain('SQLite 解析失败');
    host.querySelector<HTMLButtonElement>('.table-item[aria-pressed]')!.click();
    await settle();
    const panel = host.querySelector<HTMLElement>('[aria-label="SQL 查询"]')!;
    const sql = 'SELECT id FROM messages WHERE chat = ?1 ORDER BY id';
    const editor = panel.querySelector('textarea')!;
    editor.value = sql;
    editor.dispatchEvent(new Event('input', { bubbles: true }));
    const parameters = panel.querySelector('input')!;
    parameters.value = '["work"]';
    parameters.dispatchEvent(new Event('input', { bubbles: true }));
    await settle();
    panel.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await settle();
    expect(ipc.invoke).toHaveBeenCalledWith('fs_sqlite_query', expect.objectContaining({
      imageId: 'image-a', nodeId: 'database', parentId: 'root', sql, parameters: ['work'], offset: 0, limit: 50,
    }));
    expect(panel.querySelector('tbody')?.textContent).toContain('9223372036854775807');
    expect(panel.textContent).toContain('messages.db-wal');
    host.querySelector<HTMLButtonElement>('.preview-statusbar [aria-label="下一页"]')!.click();
    await settle();
    expect(ipc.invoke).toHaveBeenLastCalledWith('fs_sqlite_query', expect.objectContaining({ sql, parameters: ['work'], offset: 50 }));
  });
  it('binds text search refs and clears the previous match when selecting another file', async () => {
    await mount(); tab('文本'); await settle(); await openFind();
    const input = host.querySelector<HTMLInputElement>('[aria-label="搜索预览内容"]')!;
    input.value = 'needle'; input.dispatchEvent(new Event('input', { bubbles: true })); await settle();
    host.querySelectorAll<HTMLElement>('.text-view mark').forEach(mark => { mark.scrollIntoView = vi.fn(); });
    host.querySelector<HTMLButtonElement>('[aria-label="下一个匹配"]')!.click(); await settle();
    expect(host.querySelector('.text-view mark.current')).not.toBeNull();
    node.value = { ...node.value, id: 'next', name: 'next.md' }; await settle();
    expect(host.querySelector('.match-count')?.textContent).toContain('0/2'); expect(host.querySelector('mark.current')).toBeNull();
  });
  it('opens the find bar with Ctrl+F and clears it with Escape', async () => {
    await mount(); tab('文本'); await settle();
    host.querySelector('.preview-body')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true }));
    await settle();
    const input = host.querySelector<HTMLInputElement>('[aria-label="搜索预览内容"]')!;
    input.value = 'needle'; input.dispatchEvent(new Event('input', { bubbles: true })); await settle();
    expect(host.querySelectorAll('.text-view mark')).toHaveLength(2);
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await settle();
    expect(host.querySelector('[aria-label="搜索预览内容"]')).toBeNull();
    expect(host.querySelectorAll('.text-view mark')).toHaveLength(0);
  });
  it('offers the text and hex views when a file has no native view', async () => {
    const original = ipc.invoke.getMockImplementation()!;
    ipc.invoke.mockImplementation(async (command, args) => command === 'fs_preview' ? new Uint8Array([0, 1, 2, 0, 0, 255]).buffer : original(command, args));
    node.value = { ...node.value, id: 'blob', name: 'blob.dat', size: 6 }; await mount(); tab('原类型'); await settle();
    expect(host.textContent).toContain('无法按原类型显示');
    [...host.querySelectorAll<HTMLButtonElement>('.preview-body button')].find(button => button.textContent?.includes('查看十六进制'))!.click(); await settle();
    expect(host.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe('十六进制');
  });
});
