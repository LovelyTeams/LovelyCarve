
import { tr } from '../../../composables/useLocale';
import { computed, onScopeDispose, reactive, ref, shallowRef, watch } from 'vue';
import { timelineTypes, type TimelineBucket, type TimelineQuery, type TimelineTypeGroup, type TimelineTypePair } from '../../../lib/ipc/timeline';
import { notifyError, notifyInfo } from '../../../composables/useToast';
import { intersectBucketRange } from '../model/timelineHeatmap';
import { TIMELINE_UNITS, bucketRange, drillDown, type TimelineUnitValue } from '../model/timelineView';
export function useTimelineFilters(options: { imageId: () => string; tzHours: number }) {
  const tzHours = options.tzHours;
  let disposed = false;
  const unit = ref<TimelineUnitValue>('year');
  const UNIT_OPTIONS = TIMELINE_UNITS.map(candidate => ({ value: candidate, get label() { return tr("按{0}查看", { 0: tr(`timeUnits.${candidate}`) }); } }));
  const heatmapYear = ref(new Date().getFullYear());

  interface DrillFrame {
    start: number;
    end: number;
    /** 进入该层后的粒度（撤销时恢复）。 */
    unitAfter: TimelineUnitValue;
  }

  const drillStack = ref<DrillFrame[]>([]);
  const activeDrill = computed(() =>
    drillStack.value.length > 0 ? drillStack.value[drillStack.value.length - 1] : null,
  );

  function drillInto(bucket: TimelineBucket) {
    if (unit.value === 'second') {
      notifyInfo(tr("已到最细粒度"), tr("秒粒度下无法继续下钻。"));
      return;
    }
    const { start, end } = intersectBucketRange(bucket.date_str, unit.value, tzHours, activeDrill.value);
    const nextUnit = drillDown(unit.value);
    drillStack.value.push({ start, end, unitAfter: nextUnit });
    unit.value = nextUnit;
  }

  function undoDrill() {
    drillStack.value.pop();
    unit.value = drillStack.value.length > 0
      ? drillStack.value[drillStack.value.length - 1].unitAfter
      : 'year';
  }

  // ── 过滤（当前检材的应用树两级 / 文本 / 显示已删除）────────────────────
  const showDelete = ref(false);
  const filterTextDraft = ref('');
  const filterText = ref('');
  /** 勾选的 (from_app, type) 对；空 = 全部。 */
  const selectedPairs = ref<TimelineTypePair[]>([]);
  const typeGroups = shallowRef<TimelineTypeGroup[]>([]);
  const typeFilterActive = computed(() => selectedPairs.value.length > 0);
  const hasFilters = computed(() => Boolean(filterText.value || showDelete.value || typeFilterActive.value || activeDrill.value));

  // ── 查询构造（count/datas/export 共用参数族）────────────────────────
  function buildQuery(): TimelineQuery {
    return {
      start: activeDrill.value?.start,
      end: activeDrill.value?.end,
      time_zone_hours: tzHours,
      time_unit: unit.value,
      from_apps: [],
      data_types: [],
      pairs: selectedPairs.value.map((pair) => ({ ...pair })),
      text: filterText.value.trim() || undefined,
      show_delete: showDelete.value,
    };
  }

  function selectDay(date: string) {
    const range = bucketRange(date, 'day', tzHours);
    drillStack.value = [{ ...range, unitAfter: 'hour' }];
    unit.value = 'hour';
  }

  function changeHeatmapYear(value: number) {
    if (Number.isInteger(value) && value >= 100 && value <= 9998) heatmapYear.value = value;
  }
  function inputHeatmapYear(event: Event) {
    const input = event.target as HTMLInputElement;
    changeHeatmapYear(Number(input.value));
    input.value = String(heatmapYear.value);
  }

  function resetFilters() {
    filterTextDraft.value = '';
    filterText.value = '';
    selectedPairs.value = [];
    showDelete.value = false;
    drillStack.value = [];
    unit.value = 'year';
  }

  const dialog = reactive({
    open: false,
    pairs: [] as TimelineTypePair[],
  });

  function openConfigDialog() {
    dialog.pairs = selectedPairs.value.map((pair) => ({ ...pair }));
    dialog.open = true;
    void refreshTypeGroups();
  }

  let typeGroupsGeneration = 0;
  async function refreshTypeGroups() {
    const generation = ++typeGroupsGeneration;
    const imageId = options.imageId();
    try {
      const groups = await timelineTypes({ imageId });
      if (generation !== typeGroupsGeneration || imageId !== options.imageId() || disposed) return;
      typeGroups.value = groups;
    } catch (error) {
      if (generation !== typeGroupsGeneration || imageId !== options.imageId() || disposed) return;
      notifyError(tr("读取时间线维度失败"), String(error));
      typeGroups.value = [];
    }
  }

  function dialogPairChecked(fromApp: string, dataType: number): boolean {
    return dialog.pairs.some(
      (pair) => pair.from_app === fromApp && pair.data_type === dataType,
    );
  }

  function toggleDialogPair(fromApp: string, dataType: number) {
    const index = dialog.pairs.findIndex(
      (pair) => pair.from_app === fromApp && pair.data_type === dataType,
    );
    if (index >= 0) {
      dialog.pairs.splice(index, 1);
    } else {
      dialog.pairs.push({ from_app: fromApp, data_type: dataType });
    }
  }

  function groupFullyChecked(group: TimelineTypeGroup): boolean {
    return group.types.every((info) => dialogPairChecked(group.from_app, info.data_type));
  }

  function toggleDialogGroup(group: TimelineTypeGroup) {
    const fully = groupFullyChecked(group);
    for (const info of group.types) {
      const checked = dialogPairChecked(group.from_app, info.data_type);
      if (checked === fully) {
        toggleDialogPair(group.from_app, info.data_type);
      }
    }
  }

  function applyConfigDialog() {
    selectedPairs.value = dialog.pairs.map((pair) => ({ ...pair }));
    dialog.open = false;
  }

  function applyFilters() { filterText.value = filterTextDraft.value; }
  function resetContext() {
    typeGroupsGeneration++; resetFilters(); dialog.open = false; dialog.pairs = []; typeGroups.value = [];
  }
  watch(options.imageId, resetContext, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; typeGroupsGeneration++; });
  return {
    unit, UNIT_OPTIONS, heatmapYear, drillStack, activeDrill, showDelete, filterTextDraft, filterText,
    selectedPairs, typeGroups, typeFilterActive, hasFilters, dialog, buildQuery, drillInto, undoDrill,
    selectDay, changeHeatmapYear, inputHeatmapYear, resetFilters, resetContext, openConfigDialog,
    dialogPairChecked, toggleDialogPair, groupFullyChecked, toggleDialogGroup, applyConfigDialog, applyFilters
  };
}
