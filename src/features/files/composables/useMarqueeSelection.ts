import { onUnmounted, ref, type Ref } from 'vue';

export interface MarqueeRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

const MARQUEE_THRESHOLD = 5; // 拖动超过该像素数才算框选，普通单击不受影响
const MARQUEE_EDGE = 28; // 指针距上下边缘该像素数内触发自动滚动
const MARQUEE_MAX_STEP = 18; // 自动滚动每帧最大像素数
const MARQUEE_ROW_SLOP = 6; // 行命中容差：选择框只擦过一行边缘几个像素不算框中
// 框选结束后这段时间内派生的 click / dblclick 视为框选残留，调用方据此忽略。
const MARQUEE_CLICK_GUARD_MS = 200;

/**
 * 左键框选勾选。在表体按住左键拖出选择框，框到的行按拖动起始时的勾选状态逐行
 * 翻转：未勾的框到打勾，已勾的框到取消（再框一次即撤销上一次框选）。从行复选框
 * 上按下拖动同样能起框（顺着复选框列往下扫一列就全翻转），原地单击仍是普通勾选。
 * Alt+拖动放行浏览器原生文本选取，留作复制单元格文本的逃生口。
 *
 * 行高固定（`rowHeight`）且虚拟滚动的占位行同样按行高撑开，所以框到哪些行用
 * 纵向区间除以行高即可精确算出，不依赖框内行是否真的渲染在 DOM 里。
 */
export function useMarqueeSelection(options: {
  /** 表格滚动容器（含 sticky 表头）。 */
  container: Ref<HTMLElement | null>;
  /** 当前可见（已过滤、排序）的行，顺序与表格一致。 */
  rows: Readonly<Ref<readonly { id: string }[]>>;
  rowHeight: number;
  checkedIds: Ref<Set<string>>;
  /** 返回 true 时不起框（例如装载中：表格只剩状态行，行数据仍是旧目录的）。 */
  isBlocked: () => boolean;
}) {
  const { container, rows, rowHeight, checkedIds } = options;

  const marqueeRect = ref<MarqueeRect | null>(null);
  let anchor = { x: 0, y: 0 }; // 起点，容器内容坐标（含滚动偏移）
  let pointer = { x: 0, y: 0 }; // 指针当前位置，视口坐标
  let base = new Set<string>();
  let tracking = false;
  let headerHeight = 0;
  let rafId: number | null = null;
  let endedAt = -Infinity;
  // 上一次应用过的命中行区间：行区间和列表都没变就不必重建勾选 Set——
  // 全显模式下列表可达数十万行，逐像素重建是肉眼可见的卡顿。
  let applied = { first: 0, last: -1, listLen: -1 };

  function onMarqueeMouseDown(e: MouseEvent) {
    if (e.button !== 0 || e.altKey) return;
    if (options.isBlocked()) return;
    const wrap = container.value;
    if (!wrap) return;
    // thead 覆盖全选复选框、漏斗与排序；行复选框（input）不排除，从它上面
    // 拖动也能起框，原地单击的勾选语义由阈值保证。
    if ((e.target as HTMLElement).closest('button, a, thead')) return;
    const rect = wrap.getBoundingClientRect();
    const px = e.clientX - rect.left;
    const py = e.clientY - rect.top;
    // 点在滚动条上不框选
    if (px >= wrap.clientWidth || py >= wrap.clientHeight) return;
    // 抑制按下后的原生文本选取/拖拽；click（含复选框翻转）不受 mousedown
    // preventDefault 影响。preventDefault 同时挡掉了点击聚焦，表格快捷键
    // （Ctrl+A 等）依赖容器持焦，这里显式补上。
    e.preventDefault();
    wrap.focus({ preventScroll: true });
    anchor = { x: px + wrap.scrollLeft, y: py + wrap.scrollTop };
    pointer = { x: e.clientX, y: e.clientY };
    // 框选是翻转语义：以拖动起始的勾选为基底，框中的行逐行取反——未勾的
    // 并进去，已勾的取消（对已勾选区域再框一次即撤销）。基底固定在按下
    // 一刻，选择框缩回去的行随帧恢复基底状态，不会越翻越乱；不框到的
    // 勾选不受影响。整体清空走 Escape / 状态栏「清空勾选」/ 表头全选框。
    base = new Set(checkedIds.value);
    headerHeight = wrap.querySelector('thead')?.offsetHeight ?? 0;
    applied = { first: 0, last: -1, listLen: -1 };
    tracking = true;
    window.addEventListener('mousemove', onMouseMove);
    window.addEventListener('mouseup', onMouseUp);
    // 按住期间滚轮滚动也要跟着重算选择框与命中行
    wrap.addEventListener('scroll', update);
  }

  function onMouseMove(e: MouseEvent) {
    // 错过了 mouseup（Alt-Tab、原生对话框抢走焦点）：左键已松就收尾
    if ((e.buttons & 1) === 0) {
      onMouseUp();
      return;
    }
    pointer = { x: e.clientX, y: e.clientY };
    update();
  }

  function update() {
    const wrap = container.value;
    if (!wrap || !tracking) return;
    const rect = wrap.getBoundingClientRect();
    const cx = Math.min(Math.max(pointer.x - rect.left, 0), wrap.clientWidth) + wrap.scrollLeft;
    const cy = Math.min(Math.max(pointer.y - rect.top, 0), wrap.clientHeight) + wrap.scrollTop;
    if (
      !marqueeRect.value &&
      Math.abs(cx - anchor.x) < MARQUEE_THRESHOLD &&
      Math.abs(cy - anchor.y) < MARQUEE_THRESHOLD
    ) {
      return; // 还没拖出阈值，保持普通单击语义
    }
    const top = Math.min(anchor.y, cy);
    const bottom = Math.max(anchor.y, cy);
    marqueeRect.value = {
      left: Math.min(anchor.x, cx),
      top,
      width: Math.abs(cx - anchor.x),
      height: bottom - top,
    };
    const list = rows.value;
    // 带容差的命中：框沿只擦过一行最后几个像素时不算框中——肉眼看起来
    // 从第 2 行开始的框不该把第 1 行也勾上（连带表头全选框跟着亮）。
    let first = Math.max(0, Math.floor((top - headerHeight + MARQUEE_ROW_SLOP) / rowHeight));
    let last = Math.min(list.length - 1, Math.floor((bottom - headerHeight - 1 - MARQUEE_ROW_SLOP) / rowHeight));
    if (first > last) {
      // 框比容差还小（行内微拖）：退回任意相交语义，按下那一行仍可框中。
      first = Math.max(0, Math.floor((top - headerHeight) / rowHeight));
      last = Math.min(list.length - 1, Math.floor((bottom - headerHeight - 1) / rowHeight));
    }
    if (first !== applied.first || last !== applied.last || list.length !== applied.listLen) {
      applied = { first, last, listLen: list.length };
      const next = new Set(base);
      for (let i = first; i <= last; i++) {
        const id = list[i].id;
        if (base.has(id)) next.delete(id);
        else next.add(id);
      }
      checkedIds.value = next;
    }
    if (rafId === null) rafId = requestAnimationFrame(autoScroll);
  }

  // 框选中把指针拖到表格上下边缘附近时自动滚动，让选择框越过当前视口。
  function autoScroll() {
    const wrap = container.value;
    if (!wrap || !marqueeRect.value) {
      rafId = null;
      return;
    }
    // 先续订下一帧再调 update：它看到 rafId 非空才不会重复排帧，
    // 否则每帧翻倍，自动滚动会指数级失控。
    rafId = requestAnimationFrame(autoScroll);
    const rect = wrap.getBoundingClientRect();
    const py = pointer.y - rect.top;
    let dy = 0;
    if (py < MARQUEE_EDGE) {
      dy = -Math.ceil(Math.min(1, (MARQUEE_EDGE - py) / MARQUEE_EDGE) * MARQUEE_MAX_STEP);
    } else if (py > wrap.clientHeight - MARQUEE_EDGE) {
      dy = Math.ceil(Math.min(1, (py - (wrap.clientHeight - MARQUEE_EDGE)) / MARQUEE_EDGE) * MARQUEE_MAX_STEP);
    }
    if (dy !== 0) {
      const before = wrap.scrollTop;
      wrap.scrollTop = before + dy;
      if (wrap.scrollTop !== before) update();
    }
  }

  function onMouseUp(e?: MouseEvent) {
    // 框选期间松开右/中键（如呼出行右键菜单）不结束左键的框选
    if (e && e.button !== 0) return;
    window.removeEventListener('mousemove', onMouseMove);
    window.removeEventListener('mouseup', onMouseUp);
    container.value?.removeEventListener('scroll', update);
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    // 框选刚结束时 mouseup 会在落点元素上派生一次 click，
    // 调用方经 marqueeJustEnded() 据此忽略
    if (marqueeRect.value) endedAt = performance.now();
    tracking = false;
    marqueeRect.value = null;
  }

  onUnmounted(() => onMouseUp());

  return {
    marqueeRect,
    onMarqueeMouseDown,
    /** 中止进行中的框选（装载会清空勾选并换掉列表时调用）。 */
    cancelMarquee: () => onMouseUp(),
    /** 刚结束一次框选：此刻落到行 / 复选框上的 click、dblclick 是框选残留。 */
    marqueeJustEnded: () => performance.now() - endedAt < MARQUEE_CLICK_GUARD_MS,
  };
}
