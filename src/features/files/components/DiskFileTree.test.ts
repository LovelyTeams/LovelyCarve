// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, ref, type App } from 'vue';
import DiskFileTree from './DiskFileTree.vue';
import type { TreeRow } from '../model/fileTree';

const tree = vi.hoisted(() => ({ use: vi.fn() }));
vi.mock('../composables/useFileTree', () => ({ useFileTree: tree.use }));
vi.mock('../../cases/api', () => ({ useCaseContext: () => ({ activeCase: { value: null } }) }));
vi.mock('../composables/useEmptyDirFilter', () => ({ useEmptyDirFilter: () => ({ hideEmptyDirs: { value: false } }) }));
vi.mock('../../../components/display/FileNodeIcon.vue', () => ({ default: { render: () => null } }));

let app: App | undefined;
let host: HTMLDivElement;
afterEach(() => { app?.unmount(); host?.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it('renders a bounded window of a 23,536-directory tree and reveals a navigated row', async () => {
  const rows = ref<TreeRow[]>(Array.from({ length: 23_536 }, (_, index) => ({
    key: `dir:${index}`, depth: 1,
    node: { id: `dir:${index}`, name: `Directory ${index}`, parentId: 'part:0', kind: 'dir', expanded: false,
      loaded: false, loading: false, childCount: null, driveLetter: null },
  })));
  const activeId = ref<string | null>(null);
  const hydrateRows = vi.fn();
  tree.use.mockReturnValue({ visibleRows: rows, activeId, selectedLeafId: ref(null), rootNodes: ref(['part:0']),
    rootLoaded: ref(true), dockerFilesystemLoading: ref(false), hydrateRows, onRowClick: vi.fn(), toggleExpand: vi.fn() });
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(260);
  let frame: FrameRequestCallback | undefined;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frame = callback; return 1; });
  vi.stubGlobal('cancelAnimationFrame', vi.fn());
  host = document.createElement('div'); document.body.append(host);
  app = createApp({ render: () => h(DiskFileTree, { imageId: 'image', pathStack: [] }) });
  app.mount(host); await nextTick(); await nextTick();
  expect(host.querySelectorAll('.tree-row').length).toBeLessThanOrEqual(26);
  const scroll = host.querySelector<HTMLElement>('.tree-list')!;
  scroll.scrollTop = 13_000; scroll.dispatchEvent(new Event('scroll')); frame?.(0); await nextTick();
  expect(host.querySelector('.tree-name')?.textContent).toBe('Directory 492');
  expect(host.querySelectorAll('.tree-row')).toHaveLength(26);
  activeId.value = 'dir:22000'; await nextTick(); await nextTick();
  expect(host.querySelector('.tree-row.active .tree-name')?.textContent).toBe('Directory 22000');
  expect(hydrateRows.mock.calls.at(-1)?.[0].length).toBeLessThanOrEqual(26);
  rows.value = rows.value.slice(0, 3); await nextTick();
  expect(scroll.scrollTop).toBe(0);
  expect(host.querySelectorAll('.tree-row')).toHaveLength(3);
});
