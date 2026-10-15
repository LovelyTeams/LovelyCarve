
import { tr } from '../../../composables/useLocale';
import { computed, reactive, ref } from 'vue';
import { formatBytes } from '../model/model';
import type { SizeUnit, SizeFilterState } from '../model/filters';

const UNIT_BYTES: Record<SizeUnit, number> = {
  B: 1,
  KB: 1024,
  MB: 1024 ** 2,
  GB: 1024 ** 3,
  TB: 1024 ** 4,
};

function emptySizeFilter(): SizeFilterState {
  return {
    active: false,
    leEnabled: false,
    leValue: '',
    leUnit: 'MB',
    geEnabled: false,
    geValue: '',
    geUnit: 'KB',
    modEnabled: false,
    modValue: '',
    modRemainder: '',
    appliedLe: null,
    appliedGe: null,
    appliedMod: null,
  };
}

function parsedBound(enabled: boolean, value: string, unit: SizeUnit): number | null {
  if (!enabled || value.trim() === '') return null;
  const parsed = Number(value);
  const bytes = parsed * UNIT_BYTES[unit];
  return Number.isFinite(bytes) && bytes >= 0 && bytes <= Number.MAX_SAFE_INTEGER ? bytes : null;
}

function parsedMod(sizeFilter: SizeFilterState): { n: number; r: number } | null {
  if (!sizeFilter.modEnabled || sizeFilter.modValue.trim() === '') return null;
  const n = Number(sizeFilter.modValue);
  const r = Number(sizeFilter.modRemainder.trim() || '0');
  if (!Number.isSafeInteger(n) || n <= 0 || !Number.isSafeInteger(r) || r < 0 || r >= n) {
    return null;
  }
  return { n, r };
}

export function useSizeFilter() {
  const sizeFilter = reactive<SizeFilterState>(emptySizeFilter());
  const validationAttempted = ref(false);
  const sizeFilterErrors = computed(() => {
    const errors = { le: '', ge: '', mod: '', remainder: '' };
    if (!validationAttempted.value) return errors;
    const upper = parsedBound(sizeFilter.leEnabled, sizeFilter.leValue, sizeFilter.leUnit);
    const lower = parsedBound(sizeFilter.geEnabled, sizeFilter.geValue, sizeFilter.geUnit);
    if (sizeFilter.leEnabled && upper === null) errors.le = tr("请输入有效的非负大小，过大的数值请调小。");
    if (sizeFilter.geEnabled && lower === null) errors.ge = tr("请输入有效的非负大小，过大的数值请调小。");
    if (upper !== null && lower !== null && lower > upper) {
      errors.ge = tr("大小下限不能大于上限，请检查数值和单位。");
      errors.le = errors.ge;
    }
    if (sizeFilter.modEnabled) {
      const divisor = Number(sizeFilter.modValue);
      const remainder = Number(sizeFilter.modRemainder.trim() || '0');
      if (!sizeFilter.modValue.trim() || !Number.isSafeInteger(divisor) || divisor <= 0) errors.mod = tr("除数必须是有效的正整数。");
      if (!Number.isSafeInteger(remainder) || remainder < 0 || (divisor > 0 && remainder >= divisor)) errors.remainder = tr("余数必须是非负整数，并且小于除数。");
    }
    return errors;
  });

  function resetSizeFilter() {
    Object.assign(sizeFilter, emptySizeFilter());
    validationAttempted.value = false;
  }

  function appliedSizePredicate(): ((bytes: number) => boolean) | null {
    const { appliedLe: le, appliedGe: ge, appliedMod: mod } = sizeFilter;
    const conditions: Array<(bytes: number) => boolean> = [];
    if (le != null) conditions.push(bytes => bytes <= le);
    if (ge != null) conditions.push(bytes => bytes >= ge);
    if (mod) conditions.push(bytes => bytes % mod.n === mod.r);
    if (conditions.length === 0) return null;
    return bytes => conditions.every(condition => condition(bytes));
  }

  const sizeFilterEffective = computed(
    () =>
      sizeFilter.active &&
      (sizeFilter.appliedLe != null || sizeFilter.appliedGe != null || sizeFilter.appliedMod != null),
  );

  function applySizeFilter() {
    validationAttempted.value = true;
    if (Object.values(sizeFilterErrors.value).some(Boolean)) return false;
    const le = parsedBound(sizeFilter.leEnabled, sizeFilter.leValue, sizeFilter.leUnit);
    const ge = parsedBound(sizeFilter.geEnabled, sizeFilter.geValue, sizeFilter.geUnit);
    sizeFilter.appliedLe = le;
    sizeFilter.appliedGe = ge;
    sizeFilter.appliedMod = parsedMod(sizeFilter);
    sizeFilter.active = le != null || ge != null || sizeFilter.appliedMod != null;
    validationAttempted.value = false;
    return true;
  }

  function disableSizeFilter() {
    sizeFilter.active = false;
    validationAttempted.value = false;
  }

  function sizeFilterSummary(): string {
    const parts: string[] = [];
    if (sizeFilter.appliedLe != null) parts.push(`≤ ${formatBytes(sizeFilter.appliedLe)}`);
    if (sizeFilter.appliedGe != null) parts.push(`≥ ${formatBytes(sizeFilter.appliedGe)}`);
    if (sizeFilter.appliedMod) parts.push(`mod ${sizeFilter.appliedMod.n} = ${sizeFilter.appliedMod.r}`);
    return parts.join('，');
  }

  return {
    sizeFilter,
    sizeFilterErrors,
    resetSizeFilter,
    appliedSizePredicate,
    sizeFilterEffective,
    applySizeFilter,
    disableSizeFilter,
    sizeFilterSummary,
  };
}
