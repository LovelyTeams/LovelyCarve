import { computed, nextTick, onScopeDispose, reactive, ref, watch, type Ref } from 'vue';
import type { IconCategory } from '../../../utils/fileCategory';
import { COLUMNS, type ColumnDef, type DiskFsNode } from '../model/model';
import type { FileNavigationFrame } from '../model/navigation';
import { SIZE_UNITS, LARGE_FILE_LIST } from '../model/filters';
import { useSizeFilter } from './useSizeFilter';
import { useTimeFilters } from './useTimeFilters';
import { usePathFilter } from './usePathFilter';

/** Query filter state and its floating editor share a single panel-scoped owner. */
export function useFileFilters(options: {
  imageId: () => string;
  entries: Ref<DiskFsNode[]>;
  stack: Ref<FileNavigationFrame[]>;
  flatMode: Ref<boolean>;
  selectedCategories: Ref<Set<IconCategory>>;
  loading: Ref<boolean>;
  clearCategories: () => void;
  container: () => HTMLElement | null;
  fullPathOf: (node: DiskFsNode) => string;
  loadCurrent: () => Promise<void>;
  closeRowMenu: () => void;
}) {
  const { entries, stack, flatMode: deepsearch, selectedCategories, loading, clearCategories, fullPathOf, loadCurrent } = options;
  const pathSuggestEl = ref<HTMLElement | null>(null);
  let disposed = false;
  let popoverGeneration = 0;
  const filters = reactive<Record<string, string>>(
    Object.fromEntries(
      COLUMNS.filter(c => c.filterMode === 'text' || c.filterMode === 'bool').map(c => [c.key, '']),
    ),
  );

  // 过滤实际生效值。小列表与输入同步；条目超过 LARGE_FILE_LIST 时按 200ms 去抖——
  // 全显百万行下每击键全量重过滤会让输入框体感「坏了」。输入框/漏斗高亮/
  // 路径跟随读 filters（即时），filteredEntries 读这里。
  const effectiveFilters = reactive<Record<string, string>>({ ...filters });
  let filterDebounceTimer: number | null = null;

  watch(filters, () => {
    const apply = () => {
      for (const k of Object.keys(filters)) effectiveFilters[k] = filters[k];
    };
    if (entries.value.length <= LARGE_FILE_LIST) {
      if (filterDebounceTimer !== null) {
        window.clearTimeout(filterDebounceTimer);
        filterDebounceTimer = null;
      }
      apply();
      return;
    }
    if (filterDebounceTimer !== null) window.clearTimeout(filterDebounceTimer);
    filterDebounceTimer = window.setTimeout(() => {
      filterDebounceTimer = null;
      apply();
    }, 200);
  });

  const {
    sizeFilter,
    sizeFilterErrors,
    resetSizeFilter,
    appliedSizePredicate,
    sizeFilterEffective,
    applySizeFilter: applySizeFilterState,
    disableSizeFilter: disableSizeFilterState,
    sizeFilterSummary,
  } = useSizeFilter();
  const SIZE_UNIT_OPTIONS = SIZE_UNITS.map(unit => ({ value: unit, label: unit }));

  function applySizeFilter() {
    if (applySizeFilterState()) hideFilterPopover();
    else nextTick(() => filterPopoverEl.value?.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus());
  }

  function disableSizeFilter() {
    disableSizeFilterState();
    hideFilterPopover();
  }

  // Enter/Esc 需避开两类误触发：IME 组合提交/取消（keydown.key 同样是 Enter/Escape，
  // 但 isComposing 为 true），以及焦点落在按钮上的 Enter（应执行按钮自身的动作，
  // 否则「禁用」按钮上按 Enter 会反过来激活过滤器）。
  function onSizeFormEnter(e: KeyboardEvent) {
    if (e.isComposing || e.keyCode === 229) return;
    if (e.target instanceof HTMLElement && e.target.tagName === 'BUTTON') return;
    e.preventDefault();
    applySizeFilter();
  }

  function onFilterCloseKey(e: KeyboardEvent) {
    if (e.isComposing || e.keyCode === 229) return;
    e.preventDefault();
    // 文档级 Escape 链（onRowMenuKeydown）不能再收到这次按键，
    // 否则关气泡的同时还会顺手关掉详情面板/清空勾选。
    e.stopPropagation();
    hideFilterPopover();
  }

  const {
    timeFilters,
    timeFilterErrors,
    activeTimeFilterKeys,
    popoverTimeFilter,
    timeCalOpen,
    timeCalCursor,
    timeCalTime,
    timeCalCells,
    isTimeColumn,
    timeFilterEffectiveOf,
    applyTimeFilterForKey,
    disableTimeFilterForKey,
    resetTimeFilter,
    resetAllTimeFilters,
    timeFilterSummary,
    matchesTimeFilter,
    toggleTimeCal,
    moveTimeCal,
    pickCalTime,
    pickTimeCalDay,
    pickTimeCalToday,
    clearTimeCalRow,
  } = useTimeFilters({
    filterKey: () => filterPopover.key,
    getPopoverElement: () => filterPopoverEl.value,
  });

  function applyPopoverTimeFilter() {
    if (applyTimeFilterForKey(filterPopover.key)) hideFilterPopover();
    else nextTick(() => filterPopoverEl.value?.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus());
  }

  function disablePopoverTimeFilter() {
    disableTimeFilterForKey(filterPopover.key);
    hideFilterPopover();
  }

  function onTimeFormEnter(e: KeyboardEvent) {
    if (e.isComposing || e.keyCode === 229) return;
    if (e.target instanceof HTMLElement && e.target.tagName === 'BUTTON') return;
    e.preventDefault();
    applyPopoverTimeFilter();
  }

  const activeFilterCount = computed(
    () =>
      Object.values(filters).filter(v => v.trim() !== '').length +
      (sizeFilterEffective.value ? 1 : 0) +
      activeTimeFilterKeys.value.length +
      (selectedCategories.value.size > 0 ? 1 : 0),
  );

  function isColumnFiltered(key: string): boolean {
    if (key === 'size') return sizeFilterEffective.value;
    if (isTimeColumn(key)) return timeFilterEffectiveOf(key);
    return !!filters[key]?.trim();
  }

  // 漏斗悬停提示里的条件摘要——读「激活」时的快照，罗列的就是真正生效的条件。
  function filterSummaryOf(key: string): string {
    if (key === 'size') {
      return sizeFilterSummary();
    }
    if (isTimeColumn(key)) {
      return timeFilterSummary(key);
    }
    return filters[key] ?? '';
  }

  function clearAllFilters() {
    for (const k of Object.keys(filters)) filters[k] = '';
    // 清除要即时生效，不等大列表的去抖窗口。
    for (const k of Object.keys(effectiveFilters)) effectiveFilters[k] = '';
    resetSizeFilter();
    resetAllTimeFilters();
    clearCategories();
  }

  /** 移除单列条件（筛选条上的条件标签）；与「清除全部」一样即时生效。 */
  function clearColumnFilter(key: string) {
    if (key === 'size') resetSizeFilter();
    else if (isTimeColumn(key)) resetTimeFilter(key);
    else if (key in filters) {
      filters[key] = '';
      effectiveFilters[key] = '';
    }
    if (filterPopover.visible && filterPopover.key === key) hideFilterPopover();
  }

  // ── 列头漏斗过滤气泡 ──
  // 漏斗常驻列标题，点击在标题下方弹出输入框；输入即过滤，
  // Esc / Enter / 点击外部 / 滚动表格 时关闭。气泡 Teleport 到 body 以避免
  // 被表格滚动容器裁剪，定位沿用右键菜单的 fixed + 视口收拢方案。
  const filterPopover = reactive({ visible: false, key: '', x: 0, y: 0, width: 232 });
  const filterPopoverEl = ref<HTMLElement | null>(null);
  const filterPopoverColumn = computed(
    () => COLUMNS.find(c => c.key === filterPopover.key) ?? null,
  );
  // 打开气泡时表格的水平滚动位置——纵向滚动不移动 sticky 表头里的漏斗锚点，
  // 只有横向滚动才会让气泡错位、需要关闭。
  let popoverAnchorScrollLeft = 0;

  function focusPopoverInput() {
    const first = filterPopoverEl.value?.querySelector<HTMLInputElement>('input[type="text"]');
    first?.focus();
    first?.select();
  }

  function toggleFilterPopover(e: MouseEvent, c: ColumnDef) {
    options.closeRowMenu();
    if (filterPopover.visible && filterPopover.key === c.key) {
      hideFilterPopover();
      return;
    }
    // 大小列是三行结构化表单、时间列是带日历 + 时间轮面板的范围表单，
    // 都需要比单行关键字输入更宽的面板。时间列宽度按固定格尺寸精确推算：
    // 日历 7×34+12 + 时间轮 3×34+4 + 面板 padding/border/gap + 气泡 padding。
    // 全路径列要容纳整条路径的候选列表，也放宽。
    const request = ++popoverGeneration;
    const width =
      c.key === 'path' ? 384 : c.filterMode === 'size' ? 300 : c.filterMode === 'time' ? 404 : 232;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    filterPopover.key = c.key;
    filterPopover.width = width;
    filterPopover.x = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8));
    filterPopover.y = rect.bottom + 6;
    filterPopover.visible = true;
    timeCalOpen.value = null;
    closePathSuggest();
    if (c.key === 'path') ensureCurrentPathLevel(); // 预取当前层，输入 "/" 即刻有候选
    popoverAnchorScrollLeft = options.container()?.scrollLeft ?? 0;
    nextTick(() => {
      if (disposed || request !== popoverGeneration || !filterPopover.visible) return;
      focusPopoverInput();
      document.addEventListener('pointerdown', onFilterOutsidePointerDown);
      document.addEventListener('scroll', onFilterScrollAway, { capture: true, passive: true });
    });
  }

  // 外点关闭用 pointerdown + 包含判断，而不是 click + stopPropagation：在输入框里
  // 拖选文本、松手落在气泡外时，click 会派发到共同祖先（body）上，误关气泡。
  function onFilterOutsidePointerDown(e: PointerEvent) {
    const t = e.target;
    if (!(t instanceof Element)) return;
    if (filterPopoverEl.value?.contains(t)) return;
    // 单位下拉框（UiSelect）的列表 Teleport 在 body 下，同属气泡交互。
    if (t.closest('.ui-select-list')) return;
    // 漏斗按钮与筛选条上的条件标签自身的开/关/换列由 click 阶段的
    // toggleFilterPopover 决定；非左键（右键/中键）不会产生 click，不能豁免，
    // 否则气泡留着不关。
    if (e.button === 0 && t.closest('.head-funnel, [data-filter-anchor]')) return;
    hideFilterPopover();
  }

  // capture 阶段会收到文档里所有元素的 scroll——包括气泡输入框自己的横向滚动
  // （输入长哈希/路径时光标越过右缘就会触发），所以必须排除气泡内部的事件。
  function onFilterScrollAway(e: Event) {
    const t = e.target;
    if (t instanceof Node && filterPopoverEl.value?.contains(t)) return;
    if (t instanceof Element && t.closest('.ui-select-list')) return;
    // 输入过滤会缩短虚拟列表、触发 table-wrap 的 scrollTop 钳制滚动；表头是
    // sticky 的，纵向滚动不影响锚点，只在横向滚动（漏斗真的移位）时才关闭。
    const wrap = options.container();
    if (wrap && t === wrap) {
      if (loading.value) {
        // 装载中表体塌缩成单行状态行，表格宽度收窄会把 scrollLeft 钳到 0——
        // 这是布局引发的，不是用户滚动；重设锚点而不是关掉正在输入的气泡
        // （路径跟随导航每次击键都可能触发装载）。
        popoverAnchorScrollLeft = wrap.scrollLeft;
        return;
      }
      if (wrap.scrollLeft === popoverAnchorScrollLeft) return;
    }
    hideFilterPopover();
  }

  function hideFilterPopover() {
    popoverGeneration++;
    filterPopover.visible = false;
    timeCalOpen.value = null;
    closePathSuggest();
    document.removeEventListener('pointerdown', onFilterOutsidePointerDown);
    document.removeEventListener('scroll', onFilterScrollAway, { capture: true });
  }

  function clearPopoverFilter() {
    if (filterPopover.key) filters[filterPopover.key] = '';
    focusPopoverInput();
  }

  const {
    pathSuggestIndex,
    pathSuggestMatches,
    pathSuggestVisible,
    pathSuggestLoadingVisible,
    rebasePathFilterToStack,
    resolveAbsoluteDirStack,
    invalidatePathLevels,
    invalidatePathFollow,
    ensureCurrentPathLevel,
    closePathSuggest,
    onPathSuggestClick,
    onTextFilterInput,
    onTextFilterFocus,
    onTextFilterKeydown,
  } = usePathFilter({
    imageId: () => options.imageId(),
    stack,
    filters,
    entries,
    flatMode: deepsearch,
    filterPopover,
    filterPopoverEl,
    fullPathOf,
    loadCurrent,
    hideFilterPopover,
    suggestionElement: pathSuggestEl,
  });

  onScopeDispose(() => {
    disposed = true;
    if (filterDebounceTimer !== null) window.clearTimeout(filterDebounceTimer);
    hideFilterPopover();
  });
  return {
    filters, effectiveFilters, sizeFilter, sizeFilterErrors, timeFilters, timeFilterErrors, activeTimeFilterKeys, appliedSizePredicate, matchesTimeFilter,
    resetSizeFilter, resetTimeFilter, activeFilterCount, isColumnFiltered, filterSummaryOf, clearAllFilters, clearColumnFilter,
    filterPopover, filterPopoverEl, filterPopoverColumn, toggleFilterPopover, hideFilterPopover,
    rebasePathFilterToStack, resolveAbsoluteDirStack, invalidatePathLevels, invalidatePathFollow,
    onSizeFormEnter, onFilterCloseKey, applySizeFilter, disableSizeFilter, SIZE_UNIT_OPTIONS,
    popoverTimeFilter, onTimeFormEnter, timeCalOpen, timeCalCursor, timeCalTime, timeCalCells,
    toggleTimeCal, moveTimeCal, pickCalTime, pickTimeCalDay, pickTimeCalToday, clearTimeCalRow,
    applyPopoverTimeFilter, disablePopoverTimeFilter, onTextFilterInput, onTextFilterFocus, onTextFilterKeydown,
    pathSuggestVisible, pathSuggestMatches, pathSuggestLoadingVisible, pathSuggestIndex, pathSuggestEl,
    onPathSuggestClick, clearPopoverFilter,
  };
}
