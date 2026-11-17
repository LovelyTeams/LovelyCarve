// SQLite 单元格（后端 SqliteCell）的显示文本与样式类：SQLite 浏览器的表格
// 和「已删除记录」共用。BLOB 不下发内容，只显示字节数。
import type { SqliteCell, SqliteQueryCell } from '../../../lib/ipc/sqlite';
import { formatBytes } from '../../../utils/format';

export function sqliteCellText(cell: SqliteCell | SqliteQueryCell): string {
  switch (cell.kind) {
    case 'null': return 'NULL';
    case 'integer': return String(cell.value);
    case 'real': return String(cell.value);
    case 'text': return cell.value;
    case 'blob': return `<BLOB ${formatBytes(cell.value.size)}>`;
  }
}

export function sqliteCellClass(cell: SqliteCell | SqliteQueryCell): string {
  if (cell.kind === 'null') return 'cell-null';
  if (cell.kind === 'blob') return 'cell-blob';
  return '';
}
