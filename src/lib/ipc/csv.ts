import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';



// ---- CSV / TSV 预览（原类型视图）。类型镜像 core/csv_table.rs 的 Serialize
// 输出（camelCase）。解析在 Rust，前端只负责渲染 + 搜索/排序交互。 ----------

/** 后端解析的 CSV / TSV 表格。 */
export interface CsvTable {
  /** 实际使用的分隔符（单字符："," / ";" / "\t" / "|"）。 */
  delimiter: string;
  /** 分隔符是否为后端自动嗅探（true）还是调用方明确指定（false）。 */
  sniffed: boolean;
  /** 所有返回行（含可能的表头行），按文件序；ragged（行可不等长，按 colCount 补齐）。 */
  rows: string[][];
  /** 最大列宽（跨返回行），渲染时补齐成矩形。 */
  colCount: number;
  /** 读取窗口内解析到的总行数（用于「共 N 行」提示）。 */
  totalRows: number;
  /** 行数超上限，rows 只含前若干行。 */
  truncatedRows: boolean;
  /** 文件大于读取上限，仅解析了前缀字节。 */
  truncatedBytes: boolean;
}

/** 解析镜像内 CSV / TSV 文件为表格。`delimiter` 省略 / 'auto' 时后端自动嗅探；
 *  传 ',' / ';' / '\t' / '|' 则按该分隔符解析（前端切换分隔符时重新调用）。 */
export function fsCsvPreview(args: {
  imageId: ImageId;
  nodeId: NodeId;
  delimiter?: string;
}): Promise<CsvTable> {
  return invokeCmd<CsvTable>('fs_csv_preview', args);
}
