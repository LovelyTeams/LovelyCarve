import { onScopeDispose, type Ref } from 'vue';
import { isContextMenuKey } from '../../../composables/useContextMenu';
import { rowKeyOf, type DiskFsNode as FsNode } from '../model/model';

/** Table-local keyboard and scroll interaction; never installs document listeners. */
export function useFileTableKeyboard(options: {
  rows: () => FsNode[];
  selectedKey: Ref<string | null>;
  checkedIds: Ref<Set<string>>;
  container: Ref<HTMLElement | null>;
  rowHeight: number;
  menuOpen: () => boolean;
  loading: () => boolean;
  toggleAll: () => void;
  goUp: () => void;
  activate: (node: FsNode) => void;
  preview: (node: FsNode) => void;
  setAnchor: (id: string) => void;
  /** The ContextMenu key or Shift+F10: the cursor row's menu, or the list's when there is none. */
  openMenu: (node: FsNode | null) => void;
}) {
  const { selectedKey, checkedIds, container: tableWrapRef, rowHeight: ROW_HEIGHT, toggleAll } = options;
  let typeaheadBuf = '';
  let typeaheadTimer: number | null = null;
  // 选中行下标缓存：百万行上每次按键 findIndex 全表扫太慢，自动重复的方向键
  // 会肉眼可见地顿。先用缓存校验，失配才回退全扫。
  let selectedIdxCache = -1;

  function selectedIndexIn(list: FsNode[]): number {
    const key = selectedKey.value;
    if (key === null) return -1;
    if (selectedIdxCache >= 0 && selectedIdxCache < list.length && rowKeyOf(list[selectedIdxCache]) === key) {
      return selectedIdxCache;
    }
    return list.findIndex(n => rowKeyOf(n) === key);
  }

  function selectRowAt(list: FsNode[], idx: number) {
    const i = Math.max(0, Math.min(idx, list.length - 1));
    selectedKey.value = rowKeyOf(list[i]);
    selectedIdxCache = i;
    scrollRowIntoView(i);
  }

  function scrollRowIntoView(i: number) {
    const wrap = tableWrapRef.value;
    if (!wrap) return;
    const headerH = wrap.querySelector('thead')?.offsetHeight ?? 0;
    const top = headerH + i * ROW_HEIGHT;
    const bottom = top + ROW_HEIGHT;
    // sticky 表头悬浮在内容上方，行顶必须越过表头下缘才算可见。
    if (top < wrap.scrollTop + headerH) {
      wrap.scrollTop = top - headerH;
    } else if (bottom > wrap.scrollTop + wrap.clientHeight) {
      wrap.scrollTop = bottom - wrap.clientHeight;
    }
  }

  function typeaheadJump(list: FsNode[], ch: string) {
    if (typeaheadTimer !== null) window.clearTimeout(typeaheadTimer);
    typeaheadTimer = window.setTimeout(() => {
      typeaheadBuf = '';
      typeaheadTimer = null;
    }, 800);
    typeaheadBuf += ch.toLowerCase();
    // 缓冲只有一个字符时从下一行开始找（连按同字母在同前缀项间轮转），
    // 多字符时含当前行（继续输入是在收窄，不该跳走）。
    const cur = selectedIndexIn(list);
    const from = typeaheadBuf.length === 1 ? cur + 1 : Math.max(cur, 0);
    for (let k = 0; k < list.length; k++) {
      const i = (from + k) % list.length;
      if (list[i].name.toLowerCase().startsWith(typeaheadBuf)) {
        selectRowAt(list, i);
        return;
      }
    }
  }

  function onTableKeydown(e: KeyboardEvent) {
    if (e.isComposing || e.keyCode === 229) return;
    // 右键菜单打开时键盘归文档级的菜单处理器（↑/↓ 移焦点、Esc 关闭）独占，
    // 否则第一下方向键会同时移动表格光标，Enter 会在菜单底下导航走。
    if (options.menuOpen()) return;
    // 来自容器内可聚焦控件（全选框/漏斗按钮/行复选框）的按键放行——
    // preventDefault 会吞掉它们的原生 Space/Enter 激活。
    const target = e.target as HTMLElement | null;
    if (target && target !== tableWrapRef.value && target.closest('input, button, select, textarea, a[href]')) {
      return;
    }
    if ((e.ctrlKey || e.metaKey) && (e.key === 'a' || e.key === 'A')) {
      e.preventDefault();
      toggleAll();
      return;
    }
    if (isContextMenuKey(e)) {
      e.preventDefault();
      const list = options.rows();
      const cur = selectedIndexIn(list);
      if (cur >= 0) scrollRowIntoView(cur);
      options.openMenu(cur >= 0 ? list[cur] : null);
      return;
    }
    if (e.key === 'Backspace' || (e.altKey && e.key === 'ArrowLeft')) {
      e.preventDefault();
      if (!options.loading()) options.goUp();
      return;
    }
    const list = options.rows();
    if (list.length === 0) return;
    const wrap = tableWrapRef.value;
    const pageRows = wrap
      ? Math.max(1, Math.floor((wrap.clientHeight - (wrap.querySelector('thead')?.offsetHeight ?? 0)) / ROW_HEIGHT))
      : 20;
    const cur = selectedIndexIn(list);
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        selectRowAt(list, cur < 0 ? 0 : cur + 1);
        return;
      case 'ArrowUp':
        e.preventDefault();
        selectRowAt(list, cur < 0 ? list.length - 1 : cur - 1);
        return;
      case 'PageDown':
        e.preventDefault();
        selectRowAt(list, (cur < 0 ? 0 : cur) + pageRows);
        return;
      case 'PageUp':
        e.preventDefault();
        selectRowAt(list, (cur < 0 ? list.length - 1 : cur) - pageRows);
        return;
      case 'Home':
        e.preventDefault();
        selectRowAt(list, 0);
        return;
      case 'End':
        e.preventDefault();
        selectRowAt(list, list.length - 1);
        return;
      case 'Enter': {
        e.preventDefault();
        const node = cur >= 0 ? list[cur] : null;
        if (!node) return;
        if (node.kind === 'dir') options.activate(node);
        else {
          options.preview(node);
        }
        return;
      }
      case ' ': {
        e.preventDefault();
        // 正在 type-ahead 输入时空格是名字的一部分（'New Folder'），
        // 不能既翻复选框又把缓冲串搞脏。
        if (typeaheadBuf !== '') {
          typeaheadJump(list, ' ');
          return;
        }
        const node = cur >= 0 ? list[cur] : null;
        if (!node) return;
        const next = new Set(checkedIds.value);
        if (next.has(node.id)) next.delete(node.id);
        else next.add(node.id);
        checkedIds.value = next;
        options.setAnchor(node.id);
        return;
      }
    }
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      typeaheadJump(list, e.key);
    }
  }

  onScopeDispose(() => { if (typeaheadTimer !== null) window.clearTimeout(typeaheadTimer); });
  return { selectedIndexIn, scrollRowIntoView, onTableKeydown };
}
