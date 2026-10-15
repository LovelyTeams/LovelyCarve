
import { tr } from '../../../composables/useLocale';
import { computed, nextTick, reactive, ref } from 'vue';
import { formatTimestamp } from '../../../utils/time';
import type { DiskFsNode } from '../model/model';
import { TIME_COLUMNS, type TimeColumnKey, type TimeFilterEdge, type TimeFilterState } from '../model/filters';

interface TimeCalCell {
  day: number;
  iso: string;
  inMonth: boolean;
  isToday: boolean;
  isSelected: boolean;
}

interface UseTimeFiltersOptions {
  filterKey: () => string;
  getPopoverElement: () => HTMLElement | null;
}

export const CAL_HOURS = Array.from({ length: 24 }, (_, i) => String(i).padStart(2, '0'));
export const CAL_MINSEC = Array.from({ length: 60 }, (_, i) => String(i).padStart(2, '0'));

function emptyTimeFilter(): TimeFilterState {
  return {
    active: false,
    fromEnabled: false,
    fromValue: '',
    toEnabled: false,
    toValue: '',
    appliedFrom: null,
    appliedTo: null,
  };
}

function normalizedTimeBound(enabled: boolean, value: string, edge: TimeFilterEdge): string | null {
  if (!enabled) return null;
  const match = value.trim().match(/^(\d{4}-\d{2}-\d{2})(?:[T ](\d{2}:\d{2})(:\d{2})?)?$/);
  if (!match) return null;
  const [year, month, day] = match[1].split('-').map(Number);
  const date = new Date(0);
  date.setUTCFullYear(year, month - 1, day);
  if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) return null;
  if (match[2]) {
    const [hour, minute] = match[2].split(':').map(Number);
    const second = Number(match[3]?.slice(1) ?? 0);
    if (hour > 23 || minute > 59 || second > 59) return null;
  }
  if (!match[2]) return `${match[1]} ${edge === 'from' ? '00:00:00' : '23:59:59'}`;
  return `${match[1]} ${match[2]}${match[3] ?? (edge === 'from' ? ':00' : ':59')}`;
}

function toIsoDate(date: Date): string {
  const year = String(date.getFullYear()).padStart(4, '0');
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

function timestampForColumn(node: DiskFsNode, key: TimeColumnKey): string | null | undefined {
  if (key === 'created') return node.timestamps?.created || node.timestamps?.birth;
  if (key === 'accessed') return node.timestamps?.accessed;
  return node.timestamps?.modified;
}

export function useTimeFilters(options: UseTimeFiltersOptions) {
  const timeFilters = reactive<Record<TimeColumnKey, TimeFilterState>>({
    created: emptyTimeFilter(),
    accessed: emptyTimeFilter(),
    modified: emptyTimeFilter(),
  });
  const validationAttempted = reactive<Record<TimeColumnKey, boolean>>({ created: false, accessed: false, modified: false });
  const timeFilterErrors = computed(() => {
    const key = options.filterKey();
    const errors = { from: '', to: '' };
    if (!isTimeColumn(key) || !validationAttempted[key]) return errors;
    const filter = timeFilters[key];
    const { from, to } = timeFilterBounds(key);
    if (filter.fromEnabled && from === null) errors.from = tr("请输入有效日期或时间，例如 2026-10-11 09:30。");
    if (filter.toEnabled && to === null) errors.to = tr("请输入有效日期或时间，例如 2026-10-11 18:00。");
    if (from !== null && to !== null && from > to) {
      errors.from = tr("起始时间不能晚于截止时间。");
      errors.to = errors.from;
    }
    return errors;
  });

  const timeCalOpen = ref<TimeFilterEdge | null>(null);
  const timeCalCursor = reactive({ year: 2000, month: 0 });
  const timeCalTime = reactive({ h: '', m: '', s: '' });

  function isTimeColumn(key: string): key is TimeColumnKey {
    return (TIME_COLUMNS as readonly string[]).includes(key);
  }

  function resetTimeFilter(key: string) {
    if (isTimeColumn(key)) {
      Object.assign(timeFilters[key], emptyTimeFilter());
      validationAttempted[key] = false;
    }
  }

  function resetAllTimeFilters() {
    for (const key of TIME_COLUMNS) resetTimeFilter(key);
  }

  function timeFilterBounds(key: TimeColumnKey): { from: string | null; to: string | null } {
    const filter = timeFilters[key];
    return {
      from: normalizedTimeBound(filter.fromEnabled, filter.fromValue, 'from'),
      to: normalizedTimeBound(filter.toEnabled, filter.toValue, 'to'),
    };
  }

  function timeFilterEffectiveOf(key: TimeColumnKey): boolean {
    const filter = timeFilters[key];
    return filter.active && (filter.appliedFrom !== null || filter.appliedTo !== null);
  }

  const activeTimeFilterKeys = computed(() => TIME_COLUMNS.filter(key => timeFilterEffectiveOf(key)));

  const popoverTimeFilter = computed<TimeFilterState | null>(() => {
    const key = options.filterKey();
    return isTimeColumn(key) ? timeFilters[key] : null;
  });

  function applyTimeFilterForKey(key: string) {
    if (!isTimeColumn(key)) return false;
    const filter = timeFilters[key];
    const { from, to } = timeFilterBounds(key);
    validationAttempted[key] = true;
    if ((filter.fromEnabled && from === null) || (filter.toEnabled && to === null) || (from !== null && to !== null && from > to)) return false;
    filter.appliedFrom = from;
    filter.appliedTo = to;
    filter.active = from !== null || to !== null;
    validationAttempted[key] = false;
    return true;
  }

  function disableTimeFilterForKey(key: string) {
    if (isTimeColumn(key)) {
      timeFilters[key].active = false;
      validationAttempted[key] = false;
    }
  }

  function timeFilterSummary(key: TimeColumnKey): string {
    const filter = timeFilters[key];
    const parts: string[] = [];
    if (filter.appliedFrom !== null) parts.push(tr("自 {0}", { 0: filter.appliedFrom }));
    if (filter.appliedTo !== null) parts.push(tr("至 {0}", { 0: filter.appliedTo }));
    return parts.join('，');
  }

  function matchesTimeFilter(node: DiskFsNode, key: TimeColumnKey): boolean {
    const { appliedFrom: from, appliedTo: to } = timeFilters[key];
    if (from === null && to === null) return true;
    const displayValue = formatTimestamp(timestampForColumn(node, key));
    if (!/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(displayValue)) return false;
    if (from !== null && displayValue < from) return false;
    if (to !== null && displayValue > to) return false;
    return true;
  }

  function clampCalCursor(year: number, month: number) {
    if (year < 1601) {
      timeCalCursor.year = 1601;
      timeCalCursor.month = 0;
    } else if (year > 9999) {
      timeCalCursor.year = 9999;
      timeCalCursor.month = 11;
    } else {
      timeCalCursor.year = year;
      timeCalCursor.month = month;
    }
  }

  function scrollCalTimeColumns() {
    nextTick(() => {
      const columns = options.getPopoverElement()?.querySelectorAll<HTMLElement>('.cal-time-col');
      columns?.forEach(column => {
        const selected = column.querySelector<HTMLElement>('.cal-time-item.selected');
        if (selected) {
          column.scrollTop = Math.max(
            0,
            selected.offsetTop - column.clientHeight / 2 + selected.clientHeight / 2,
          );
        }
      });
    });
  }

  function toggleTimeCal(which: TimeFilterEdge) {
    if (timeCalOpen.value === which) {
      timeCalOpen.value = null;
      return;
    }
    const filter = popoverTimeFilter.value;
    const currentValue = ((which === 'from' ? filter?.fromValue : filter?.toValue) ?? '').trim();
    const dateMatch = currentValue.match(/^(\d{4})-(\d{2})-(\d{2})/);
    const base = dateMatch
      ? { year: Number(dateMatch[1]), month: Number(dateMatch[2]) - 1 }
      : { year: new Date().getFullYear(), month: new Date().getMonth() };
    clampCalCursor(base.year, base.month);
    const timeMatch = currentValue.match(/[T ](\d{2}):(\d{2})(?::(\d{2}))?$/);
    timeCalTime.h = timeMatch ? timeMatch[1] : '';
    timeCalTime.m = timeMatch ? timeMatch[2] : '';
    timeCalTime.s = timeMatch ? (timeMatch[3] ?? '00') : '';
    timeCalOpen.value = which;
    scrollCalTimeColumns();
  }

  function moveTimeCal(deltaMonths: number) {
    const date = new Date(timeCalCursor.year, timeCalCursor.month + deltaMonths, 1);
    clampCalCursor(date.getFullYear(), date.getMonth());
  }

  const timeCalSelected = computed<string | null>(() => {
    const which = timeCalOpen.value;
    const filter = popoverTimeFilter.value;
    if (!which || !filter) return null;
    const value = which === 'from' ? filter.fromValue : filter.toValue;
    const match = value.trim().match(/^(\d{4}-\d{2}-\d{2})/);
    return match ? match[1] : null;
  });

  const timeCalCells = computed<TimeCalCell[]>(() => {
    const { year, month } = timeCalCursor;
    const leadingDays = (new Date(year, month, 1).getDay() + 6) % 7;
    const today = toIsoDate(new Date());
    const selected = timeCalSelected.value;
    return Array.from({ length: 42 }, (_, index) => {
      const date = new Date(year, month, 1 - leadingDays + index);
      const iso = toIsoDate(date);
      return {
        day: date.getDate(),
        iso,
        inMonth: date.getMonth() === month,
        isToday: iso === today,
        isSelected: iso === selected,
      };
    });
  });

  function calTimePart(): string | null {
    const { h, m, s } = timeCalTime;
    if (h === '' && m === '' && s === '') return null;
    return `${h || '00'}:${m || '00'}:${s || '00'}`;
  }

  function syncCalTimeToText() {
    const which = timeCalOpen.value;
    const filter = popoverTimeFilter.value;
    if (!which || !filter) return;
    const currentValue = which === 'from' ? filter.fromValue : filter.toValue;
    const dateMatch = currentValue.trim().match(/^(\d{4}-\d{2}-\d{2})/);
    if (!dateMatch) return;
    const time = calTimePart();
    const text = time ? `${dateMatch[1]} ${time}` : dateMatch[1];
    if (which === 'from') {
      filter.fromValue = text;
      filter.fromEnabled = true;
    } else {
      filter.toValue = text;
      filter.toEnabled = true;
    }
  }

  function pickCalTime(segment: 'h' | 'm' | 's', value: string) {
    timeCalTime[segment] = value;
    if (timeCalTime.h === '') timeCalTime.h = '00';
    if (timeCalTime.m === '') timeCalTime.m = '00';
    if (timeCalTime.s === '') timeCalTime.s = '00';
    syncCalTimeToText();
  }

  function focusTimeRowInput(which: TimeFilterEdge) {
    const inputs = options.getPopoverElement()?.querySelectorAll<HTMLInputElement>('.disk-time-input');
    inputs?.[which === 'from' ? 0 : 1]?.focus();
  }

  function pickTimeCalDay(iso: string) {
    const which = timeCalOpen.value;
    const filter = popoverTimeFilter.value;
    if (!which || !filter) return;
    const time = calTimePart();
    const text = time ? `${iso} ${time}` : iso;
    if (which === 'from') {
      filter.fromValue = text;
      filter.fromEnabled = true;
    } else {
      filter.toValue = text;
      filter.toEnabled = true;
    }
    focusTimeRowInput(which);
    timeCalOpen.value = null;
  }

  function pickTimeCalToday() {
    pickTimeCalDay(toIsoDate(new Date()));
  }

  function clearTimeCalRow() {
    const which = timeCalOpen.value;
    const filter = popoverTimeFilter.value;
    if (!which || !filter) return;
    if (which === 'from') {
      filter.fromValue = '';
      filter.fromEnabled = false;
    } else {
      filter.toValue = '';
      filter.toEnabled = false;
    }
    timeCalTime.h = '';
    timeCalTime.m = '';
    timeCalTime.s = '';
    focusTimeRowInput(which);
    timeCalOpen.value = null;
  }

  return {
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
  };
}
