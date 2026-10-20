import { describe, expect, it } from 'vitest';
import { translate, type Translator } from '../../../i18n/translate';
import { activeFilterChips } from './filterChips';

describe('activeFilterChips', () => {
  const active: Record<string, string> = { size: '≥ 1.00 MB', name: '*.jpg', deleted: '是', modified: '自 2026-09-01 00:00:00' };
  const options = {
    isFiltered: (key: string) => key in active,
    summaryOf: (key: string) => active[key] ?? '',
  };

  it('lists active conditions in column order with their summaries', () => {
    expect(activeFilterChips(options)).toEqual([
      { key: 'name', label: '名称', value: '*.jpg' },
      { key: 'size', label: '大小', value: '≥ 1.00 MB' },
      { key: 'modified', label: '修改时间', value: '自 2026-09-01 00:00:00' },
      { key: 'deleted', label: '已删除', value: '是' },
    ]);
  });

  it('translates labels and boolean choices but keeps entered values', () => {
    const english: Translator = (message, params) => translate(message, 'en', params);
    expect(activeFilterChips(options, english).map(chip => [chip.label, chip.value])).toEqual([
      ['Name', '*.jpg'],
      ['Size', '≥ 1.00 MB'],
      ['Modified', '自 2026-09-01 00:00:00'],
      ['Deleted', 'Yes'],
    ]);
  });

  it('skips columns that cannot be filtered', () => {
    expect(activeFilterChips({ isFiltered: () => true, summaryOf: () => 'x', columns: [{ key: 'index', label: '序号', filterMode: 'none' }] })).toEqual([]);
  });
});
