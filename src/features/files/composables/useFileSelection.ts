import { computed, ref } from 'vue';
import type { DiskFsNode as FsNode } from '../model/model';

/** Stable cursor rows and node-deduplicated visible selection are distinct identities. */
export function useFileSelection(options: {
  rows: () => FsNode[];
  marqueeJustEnded: () => boolean;
}) {
  const selectedKey = ref<string | null>(null);
  const checkedIds = ref<Set<string>>(new Set());

  const allSelected = computed(() => {
    // 勾选集比可见去重行数还小就不可能全选——先短路掉 O(n) 的 every，
    // 否则全显大列表上每次勾选变化（框选每帧）都全表扫一遍。
    const distinct = filteredById.value.size;
    if (distinct === 0 || checkedIds.value.size < distinct) return false;
    return options.rows().every(n => checkedIds.value.has(n.id));
  });

  // 表头全选框按标准约定反映勾选状态，与选中方式（点框/框选/Shift 范围/
  // Ctrl+A）无关：全部可见行已勾 = 勾选态，选了但没选全 = 半选横杠，
  // 没有勾选 = 空。点击在「全选可见行」与「清空可见行」之间切换。
  const headerChecked = computed(() => allSelected.value);
  const headerIndeterminate = computed(
    () => !allSelected.value && checkedStats.value.count > 0,
  );

  function toggleAll() {
    const next = new Set(checkedIds.value);
    if (allSelected.value) {
      options.rows().forEach(n => next.delete(n.id));
    } else {
      options.rows().forEach(n => next.add(n.id));
    }
    checkedIds.value = next;
  }

  // Shift+点击范围勾选的锚点：上一次单独点过的复选框所在行 id。存 id 而非
  // 下标——过滤/排序变化后下标会漂移，id 始终指向同一行。
  let lastCheckAnchorId: string | null = null;

  function onCheckClick(e: MouseEvent, id: string) {
    const box = e.target as HTMLInputElement;
    // 框选拖动的起止都落在同一个复选框上时，浏览器在 mouseup 后补发的 click
    // 会原生翻转复选框——不能让它把刚框好的勾选又改掉；把 DOM 勾选恢复成
    // 真实状态。
    if (options.marqueeJustEnded()) {
      box.checked = checkedIds.value.has(id);
      return;
    }
    // 不 preventDefault：原生激活在 click 派发前已翻转复选框，这里读到的
    // checked 就是用户意图的新状态，状态跟着 DOM 走（prevent + 自管状态
    // 依赖「取消激活回滚」与 Vue 补丁的时序，在 WebView2 上不可靠）。
    const willCheck = box.checked;
    const next = new Set(checkedIds.value);
    if (e.shiftKey && lastCheckAnchorId && lastCheckAnchorId !== id) {
      const list = options.rows();
      let ai = -1;
      let ci = -1;
      for (let i = 0; i < list.length && (ai < 0 || ci < 0); i++) {
        if (ai < 0 && list[i].id === lastCheckAnchorId) ai = i;
        if (ci < 0 && list[i].id === id) ci = i;
      }
      if (ai >= 0 && ci >= 0) {
        const [from, to] = ai <= ci ? [ai, ci] : [ci, ai];
        for (let i = from; i <= to; i++) {
          if (willCheck) next.add(list[i].id);
          else next.delete(list[i].id);
        }
        checkedIds.value = next;
        lastCheckAnchorId = id;
        return;
      }
    }
    if (willCheck) next.add(id);
    else next.delete(id);
    checkedIds.value = next;
    lastCheckAnchorId = id;
  }
  /** Ctrl+click on a row: flips its check and makes it the range anchor. */
  function toggleChecked(id: string) {
    const next = new Set(checkedIds.value);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    checkedIds.value = next;
    lastCheckAnchorId = id;
  }

  /** Shift+click on a row: checks every visible row from the anchor (the row
   *  last checked by hand, else `fallbackAnchorId`) through `id`, keeping the
   *  other checks. Without an anchor in the list only `id` is checked. */
  function checkRange(id: string, fallbackAnchorId: string | null) {
    const anchorId = lastCheckAnchorId ?? fallbackAnchorId;
    const list = options.rows();
    let anchorIndex = -1;
    let targetIndex = -1;
    for (let i = 0; i < list.length && (anchorIndex < 0 || targetIndex < 0); i++) {
      if (anchorIndex < 0 && list[i].id === anchorId) anchorIndex = i;
      if (targetIndex < 0 && list[i].id === id) targetIndex = i;
    }
    const next = new Set(checkedIds.value);
    if (anchorIndex < 0 || targetIndex < 0) {
      next.add(id);
    } else {
      const [from, to] = anchorIndex <= targetIndex ? [anchorIndex, targetIndex] : [targetIndex, anchorIndex];
      for (let i = from; i <= to; i++) next.add(list[i].id);
    }
    checkedIds.value = next;
    lastCheckAnchorId = id;
  }

  function checkedOfKind(kind: 'file' | 'dir'): FsNode[] {
    const out: FsNode[] = [];
    const seen = new Set<string>();
    for (const n of options.rows()) {
      if (n.kind !== kind || !checkedIds.value.has(n.id) || seen.has(n.id)) continue;
      seen.add(n.id);
      out.push(n);
    }
    return out;
  }
  const checkedFiles = computed(() => checkedOfKind('file'));
  const checkedDirs = computed(() => checkedOfKind('dir'));

  // ── 底部状态栏 ──
  // 勾选统计走 id → 节点 的索引：索引随过滤结果重建一次（O(n)），之后每次
  // 勾选变化只遍历勾选集（O(k)）——框选每帧都改 checkedIds，绝不能每帧 O(n)。
  const filteredById = computed(() => {
    const m = new Map<string, FsNode>();
    for (const n of options.rows()) if (!m.has(n.id)) m.set(n.id, n);
    return m;
  });

  const checkedStats = computed(() => {
    let count = 0;
    let bytes = 0;
    const m = filteredById.value;
    for (const id of checkedIds.value) {
      const n = m.get(id);
      if (!n) continue; // 勾选后被过滤隐藏的行不计入——与导出口径一致
      count += 1;
      if (n.kind !== 'dir') bytes += n.size;
    }
    return { count, bytes };
  });

  function clearChecked() {
    checkedIds.value = new Set();
    lastCheckAnchorId = null;
  }

  function reset() {
    selectedKey.value = null;
    clearChecked();
  }
  return { selectedKey, checkedIds, headerChecked, headerIndeterminate, toggleAll, onCheckClick, toggleChecked, checkRange,
    checkedFiles, checkedDirs, checkedStats, clearChecked, reset, setAnchor: (id: string) => { lastCheckAnchorId = id; } };
}
