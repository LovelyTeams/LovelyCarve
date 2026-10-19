import type { RecordColumn, RecordPropertyLayout } from '../../../lib/ipc/analysisResults';

export type AnalysisPresentation = 'properties' | 'table';

/** Resolve only complete, distinct, visible roles; malformed or outdated hints fall back to a table. */
export function resolvePropertyLayout(layout: RecordPropertyLayout | null | undefined, columns: RecordColumn[]) {
  if (!layout) return null;
  const visible = columns.filter(column => !column.hidden);
  const byId = new Map(visible.map(column => [column.id, column]));
  const ids = [layout.value, layout.label, layout.secondary].filter((id): id is string => typeof id === 'string');
  if (ids.length !== visible.length || new Set(ids).size !== ids.length || ids.some(id => !byId.has(id))) return null;
  const value = byId.get(layout.value);
  if (!value) return null;
  return { value, label: layout.label ? byId.get(layout.label) ?? null : null, secondary: layout.secondary ? byId.get(layout.secondary) ?? null : null };
}

/** A new installed plugin may improve presentation of an older run only when its visible shape still matches. */
export function compatiblePropertyLayout(columns: RecordColumn[], declared: RecordColumn[], layout: RecordPropertyLayout | null | undefined) {
  const visible = columns.filter(column => !column.hidden);
  const current = declared.filter(column => !column.hidden);
  if (visible.length !== current.length || visible.some(column => !current.some(field => field.id === column.id && field.kind === column.kind))) return null;
  return resolvePropertyLayout(layout, columns) ? layout ?? null : null;
}
