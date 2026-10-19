import type { RecordColumn, RecordFieldKind, RecordPropertyLayout } from '../../../lib/ipc/analysisResults';

export type AnalysisPresentation = 'properties' | 'table';

/** Narrowest share of a flexible (free text) table column. */
export const FLEXIBLE_COLUMN_WIDTH = 200;

/** Table column width (px) by field kind, never by record contents: counts and
 *  flags stay narrow, a time with its zone (`… UTC+08:00`) fits whole and
 *  paths get room. Free text, lists and JSON return null: they share whatever
 *  width is left, at least {@link FLEXIBLE_COLUMN_WIDTH} each. */
export function columnWidth(kind: RecordFieldKind): number | null {
  switch (kind) {
    case 'bool': return 88;
    case 'int':
    case 'float':
    case 'duration': return 112;
    case 'size': return 120;
    case 'enum': return 140;
    case 'ip': return 152;
    case 'timestamp':
    case 'hash': return 240;
    case 'path': return 280;
    case 'text':
    case 'list':
    case 'json': return null;
  }
}

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
