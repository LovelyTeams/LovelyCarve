
import { tr } from '../../../composables/useLocale';
import { computed, nextTick, onScopeDispose, ref, type Ref } from 'vue';
import { fsNodeLocate, fsSourceLocate } from '../../../lib/ipc/fs/query';
import type { FsLocateResult } from '../../../lib/ipc/fs/types';
import { rowKeyOf, type DiskFsNode as FsNode } from '../model/model';
import type { FileNavigationFrame as StackFrame, FileScrollRestore, FileByteFocus } from '../model/navigation';

/** Breadcrumb/tree navigation and external source resolution have one panel owner. */
export function useFileNavigation(options: {
  imageId: () => string;
  loadGeneration: () => number;
  entries: Ref<FsNode[]>;
  filteredRows: () => FsNode[];
  selectedKey: Ref<string | null>;
  detailNode: Ref<FsNode | null>;
  detailFocus: Ref<FileByteFocus | null>;
  previewVisible: Ref<boolean>;
  error: Ref<string | null>;
  container: () => HTMLElement | null;
  scrollToRow: (index: number) => void;
  rebasePath: () => void;
  loadCurrent: () => Promise<void>;
  prepareLocate: () => void;
}) {
  const { entries, selectedKey, detailNode, detailFocus, previewVisible, error } = options;
  const stack = ref<StackFrame[]>([{ id: 'root', name: '/' }]);
  const currentDirectoryPath = computed(() => {
    const path = stack.value.slice(1).map(frame => frame.name).join('/');
    return path ? `/${path}` : '/';
  });
  const pendingRestore = ref<FileScrollRestore | null>(null);
  const directoryTargetId = ref<string | null>(null);
  let locateGeneration = 0;
  let lastActivateAt = -Infinity;
  onScopeDispose(() => { locateGeneration++; });

  function onTreeNavigate(newStack: StackFrame[]) {
    // The tree emits a fresh stack rooted at "/" plus each ancestor down
    // to the clicked node. Replace the list view's stack and reload —
    // identical to clicking the breadcrumb.
    pendingRestore.value = null;
    stack.value = newStack.slice();
    options.rebasePath();
    options.loadCurrent();
  }

  // 树里单击文件：导航到其所在目录并打开该文件的详情面板，与在右侧列表单击
  // 该文件一致（详情默认「原类型视图」）。
  function onTreeOpenFile(newStack: StackFrame[], fileId: string) {
    // 已在目标目录：直接选中并打开详情，不重载——保留勾选 / 滚动位置。
    const sameDir =
      stack.value.length === newStack.length &&
      stack.value.every((f, i) => f.id === newStack[i].id);
    if (sameDir) {
      // entries 是未过滤列表：命中即打开详情（详情面板独立于列表过滤）；
      // 仅当该行未被过滤隐藏（在 filteredEntries 中可见）时才滚动定位。
      const node = entries.value.find(n => n.id === fileId);
      if (node) {
        selectedKey.value = rowKeyOf(node);
        detailNode.value = node;
        previewVisible.value = true;
        // 滚到「选中的那一行」本身（按 rowKey 配对）：硬链接在全显里有多行，
        // 用 id 另找首个可见同 id 行可能滚到与高亮不同的一行。
        const idx = options.filteredRows().findIndex(n => rowKeyOf(n) === rowKeyOf(node));
        if (idx >= 0) options.scrollToRow(idx);
        return;
      }
    }
    // 切目录：导航到文件所在目录，装载完成后选中该文件并打开其详情面板，
    // 复用 goUp/goTo 的「装载后定位子项」机制（附带 openDetail 标记）。
    pendingRestore.value = { scrollTop: 0, childId: fileId, openDetail: true };
    stack.value = newStack.slice();
    options.rebasePath();
    options.loadCurrent();
  }

  /** Resolve search/timeline sources, reveal the row and optionally focus matched bytes.
   *  Unallocated clusters and disk gaps use the same virtual-node lookup. */
  async function openFileByNodeId(nodeId: string, offset?: number, length = 1) {
    return openLocatedFile(() => fsNodeLocate({ imageId: options.imageId(), nodeId }), offset, length);
  }
  async function openFileBySource(rootId: string, path: string) {
    return openLocatedFile(() => fsSourceLocate({ imageId: options.imageId(), rootId, path }));
  }
  async function openLocatedFile(resolve: () => Promise<FsLocateResult>, offset?: number, length = 1) {
    const request = ++locateGeneration;
    const imageId = options.imageId();
    const initialLoad = options.loadGeneration();
    const current = () => request === locateGeneration && imageId === options.imageId();
    const located = await resolve();
    if (!current() || initialLoad !== options.loadGeneration()) return;
    const frames: StackFrame[] = [
      { id: 'root', name: '/' },
      ...located.frames.filter(frame => frame.id !== 'root').map(frame => ({ id: frame.id, name: frame.name })),
    ];
    // A result jump must not remain hidden by the previous search/category filters.
    options.prepareLocate();
    pendingRestore.value = null;
    stack.value = frames;
    options.rebasePath();
    directoryTargetId.value = located.file_id;
    const loadingRequest = options.loadCurrent();
    const targetLoad = options.loadGeneration();
    await loadingRequest;
    await nextTick();
    if (!current() || targetLoad !== options.loadGeneration()) return;
    if (error.value) throw new Error(error.value);
    if (!located.file_id) return;
    const node = entries.value.find(entry => entry.id === located.file_id);
    if (!node) throw new Error(tr("文件已不在原目录中，请刷新索引后重新定位"));
    selectedKey.value = rowKeyOf(node);
    detailNode.value = node;
    detailFocus.value = offset !== undefined && Number.isSafeInteger(offset) && offset >= 0
      ? { nodeId: node.id, offset, length: Number.isSafeInteger(length) && length > 0 ? length : 1 } : null;
    previewVisible.value = true;
    await nextTick();
    const index = options.filteredRows().findIndex(entry => rowKeyOf(entry) === selectedKey.value);
    if (index >= 0) options.scrollToRow(index);
  }

  function onActivate(node: FsNode) {
    if (node.kind !== 'dir') return;
    detailNode.value = null;
    lastActivateAt = performance.now();
    // 离开本层前记下滚动位置，goUp/goTo 返回时恢复——「钻进去看一眼再退出来
    // 继续扫」是最高频的浏览动作，每次都跳回顶部会让人反复找位置。
    const top = stack.value[stack.value.length - 1];
    if (top) top.scrollTop = options.container()?.scrollTop ?? 0;
    pendingRestore.value = null;
    stack.value.push({ id: node.id, name: node.name });
    options.rebasePath();
    options.loadCurrent();
  }

  function withinActivateCooldown() {
    return performance.now() - lastActivateAt < 300;
  }

  function goUp() {
    if (stack.value.length <= 1) return;
    const popped = stack.value.pop();
    const target = stack.value[stack.value.length - 1];
    pendingRestore.value = { scrollTop: target?.scrollTop ?? 0, childId: popped?.id ?? null };
    options.rebasePath();
    options.loadCurrent();
  }

  function goTo(index: number) {
    if (index < 0 || index >= stack.value.length - 1) return;
    const childId = stack.value[index + 1]?.id ?? null;
    pendingRestore.value = { scrollTop: stack.value[index]?.scrollTop ?? 0, childId };
    stack.value = stack.value.slice(0, index + 1);
    options.rebasePath();
    options.loadCurrent();
  }

  return { stack, currentDirectoryPath, pendingRestore, directoryTargetId,
    onTreeNavigate, onTreeOpenFile, openFileByNodeId, openFileBySource,
    onActivate, withinActivateCooldown, goUp, goTo, invalidateLocate: () => { locateGeneration++; } };
}
