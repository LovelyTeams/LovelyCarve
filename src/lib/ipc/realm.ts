import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';



// ---- Realm 预览（原类型视图）。类型镜像 core/realm_preview.rs 的
// Serialize 输出（camelCase）。解析使用 vendored realm-db-reader 的内存
// reader，不创建临时文件；前端按页浏览表数据。 ---------------------------

export interface RealmColumn {
  index: number;
  name: string | null;
  nullable: boolean;
  indexed: boolean;
}

export interface RealmTable {
  name: string;
  tableNumber: number;
  columns: RealmColumn[];
  rowCount: number | null;
  error: string | null;
}

export interface RealmDigest {
  tables: RealmTable[];
  bytesRead: number;
  truncatedBytes: boolean;
  notes: string[];
}

export interface RealmBinary {
  size: number;
  hex: string;
  truncated: boolean;
}

export interface RealmLink {
  targetTableNumber: number;
  rowNumber: number;
}

export interface RealmBacklink {
  originTableNumber: number;
  originColumnNumber: number;
  rowNumbers: number[];
  truncated: boolean;
}

export type RealmCell =
  | { kind: 'null' }
  | { kind: 'int'; value: number }
  | { kind: 'bool'; value: boolean }
  | { kind: 'string'; value: { text: string; truncated: boolean } }
  | { kind: 'binary'; value: RealmBinary }
  | { kind: 'table'; value: { rows: number } }
  | { kind: 'timestamp'; value: string }
  | { kind: 'float'; value: number }
  | { kind: 'double'; value: number }
  | { kind: 'link'; value: RealmLink }
  | { kind: 'linkList'; value: { links: RealmLink[]; truncated: boolean } }
  | { kind: 'backlink'; value: RealmBacklink }
  | { kind: 'unsupported'; value: string }
  | { kind: 'error'; value: string };

export interface RealmRow {
  index: number;
  cells: RealmCell[];
}

export interface RealmRows {
  columns: RealmColumn[];
  rows: RealmRow[];
  total: number;
  offset: number;
  returned: number;
}

export function fsRealmPreview(args: {
  imageId: ImageId;
  nodeId: NodeId;
}): Promise<RealmDigest> {
  return invokeCmd<RealmDigest>('fs_realm_preview', args);
}

export function fsRealmTableRows(args: {
  imageId: ImageId;
  nodeId: NodeId;
  tableNumber: number;
  offset: number;
  limit: number;
}): Promise<RealmRows> {
  return invokeCmd<RealmRows>('fs_realm_table_rows', args);
}
