import { sourceText, type Translator } from '../../../i18n/translate';
import { COLUMNS, type ColumnDef } from './model';

/** One active column condition on the filter bar. */
export interface FilterChip {
  key: string;
  /** Translated column label. */
  label: string;
  /** The condition as entered; a boolean choice is translated. */
  value: string;
}

/** Active column conditions in column order. Every condition that narrows the
 *  list gets a chip, so none of them stays hidden behind a column header. */
export function activeFilterChips(
  options: {
    isFiltered: (key: string) => boolean;
    summaryOf: (key: string) => string;
    columns?: readonly ColumnDef[];
  },
  tr: Translator = sourceText,
): FilterChip[] {
  return (options.columns ?? COLUMNS)
    .filter(column => column.filterMode !== 'none' && options.isFiltered(column.key))
    .map(column => {
      const summary = options.summaryOf(column.key);
      return { key: column.key, label: tr(column.label), value: column.filterMode === 'bool' ? tr(summary) : summary };
    });
}
