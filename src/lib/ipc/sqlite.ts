import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';



// ---- SQLite 预览（原类型视图）。类型镜像 core/sqlite_preview.rs 的
// Serialize 输出。 -------------------------------------------------------

/** 单元格值。BLOB 不下发内容，只给字节数，UI 显示占位符。 */
export type SqliteCell =
  | { kind: 'null' }
  | { kind: 'integer'; value: number }
  | { kind: 'real'; value: number }
  | { kind: 'text'; value: string }
  | { kind: 'blob'; value: { size: number } };

export interface SqliteColumn {
  name: string;
  type_name: string;
  notnull: boolean;
  default_value: string | null;
  /** 主键序号（PRAGMA table_info 的 pk 列）；0 = 非主键。 */
  pk: number;
}

export interface SqliteTable {
  name: string;
  /** `"table"` / `"view"`，镜像 sqlite_master.type。 */
  kind: string;
  columns: SqliteColumn[];
  /** 视图不计数，为 null。 */
  row_count: number | null;
  /** 头部采样行（后端 LIMIT 50），按 columns 顺序。 */
  sample_rows: SqliteCell[][];
  /** 该表单独解析失败时的错误（坏页/虚表等），不影响其他表。 */
  error: string | null;
  create_sql: string | null;
}

export interface SqliteIndex {
  name: string;
  table: string;
  unique: boolean;
}

export interface SqliteHeader {
  page_size: number;
  user_version: number;
  application_id: number;
  schema_version: number;
  encoding: string;
  journal_mode: string | null;
}

export interface SqliteDigest {
  header: SqliteHeader;
  tables: SqliteTable[];
  indexes: SqliteIndex[];
  /** true 表示库是加密的（SQLCipher），本次已用密钥成功解密——UI 据此显示
   *  「已解密」徽章。 */
  encrypted: boolean;
  /** 一并导入预览 VFS 的 -wal 文件名；非空表示预览是 SQLite 回放 WAL 后的
   *  状态，而非最后一次 checkpoint。 */
  sidecars: string[];
  bytes_copied: number;
  truncated: boolean;
}

/** 加密（SQLCipher）库的解密参数。字段全可选；passphrase 与 rawHexKey 二选一
 *  （rawHexKey 优先）。compatibility/pageSize/kdfIter 用于非默认或旧版库
 *  （如微信：raw key + 1024 页 + 低 kdf_iter）。 */
export interface SqliteKey {
  /** 文本口令，按 PBKDF2 派生。 */
  passphrase?: string | null;
  /** 256-bit raw key 的 hex（64 字符），可再带 16 字节 salt（共 96 字符）。 */
  rawHexKey?: string | null;
  /** SQLCipher 兼容版本 1~4（cipher_compatibility）。 */
  compatibility?: number | null;
  /** 页大小（cipher_page_size，字节）。 */
  pageSize?: number | null;
  /** KDF 迭代次数（kdf_iter）。 */
  kdfIter?: number | null;
}

/** 后端返回的「需要密钥」错误前缀：前端据此弹出密钥输入表单。 */
export const SQLITE_LOCKED = 'SQLITE_LOCKED';

/** 把镜像内的 SQLite 库（连同 WAL sidecar）导入后端预览 VFS 后只读打开，
 *  返回全库摘要：表/视图的列定义、行数与采样行。`parentId` 是库所在目录的
 *  节点，用来查找同目录的 -wal。加密库需传 `key`；密钥/参数不对时 reject
 *  且消息以 `SQLITE_LOCKED` 开头。非 SQLite / 损坏的库也 reject，错误信息
 *  可直接展示。 */
export function fsSqlitePreview(args: {
  imageId: ImageId;
  nodeId: NodeId;
  parentId?: NodeId | null;
  key?: SqliteKey;
}): Promise<SqliteDigest> {
  return invokeCmd<SqliteDigest>('fs_sqlite_preview', args);
}

/** 删除记录的表归属，镜像 core/sqlite_recovery.rs 的 `TableLink`：
 *  known = 页仍属于该表的 B 树；inferred = 空闲页，行结构只匹配一张表；
 *  ambiguous = 匹配多张表（给出的是最佳猜测）；unattributed = 无法归属。 */
export type SqliteTableLink = 'known' | 'inferred' | 'ambiguous' | 'unattributed';

/** 删除记录的来源，镜像 `RecordOrigin`。 */
export type SqliteRecordOrigin =
  | 'freelist_page'
  | 'in_page'
  | 'freeblock'
  | 'prior_version'
  | 'dropped_table'
  | 'wal_frame'
  | 'wal_commit'
  | 'journal_deleted'
  | 'journal_prior';

export interface SqliteRecoveredRecord {
  /** 所属或最匹配的表；无法归属时为 null。 */
  table: string | null;
  link: SqliteTableLink;
  origin: SqliteRecordOrigin;
  /** 记录所在的页（从 1 起；回滚日志记录为日志里还原的页）。 */
  page: number | null;
  /** 页内字节偏移。 */
  offset: number | null;
  /** 空闲块重建会覆盖 rowid，此时为 null。 */
  rowid: number | null;
  /** 雕刻记录的启发式置信度，(0, 1]。 */
  confidence: number | null;
  /** WAL 记录所在的帧号（从 0 起）。 */
  wal_frame: number | null;
  /** 跨溢出页重组时用到的溢出页。 */
  overflow_pages: number[];
  values: SqliteCell[];
  /** 回滚日志的修改前版本：该行当前的值。 */
  current_values: SqliteCell[] | null;
}

/** 从空闲区恢复的、已删除或被替换的 sqlite_master 行。 */
export interface SqliteRecoveredSchema {
  object_type: string;
  name: string;
  table_name: string;
  root_page: number | null;
  sql: string;
}

/** 一个库的删除记录恢复结果，镜像 `SqliteRecovery`。 */
export interface SqliteRecovery {
  page_size: number;
  page_count: number;
  freelist_pages: number;
  /** 加密库按页解密所用的 SQLCipher 参数说明，如「SQLCipher 4 · 页大小 4096」。 */
  cipher: string | null;
  /** 参与恢复的 sidecar（-wal 或 -journal）文件名。 */
  sidecars: string[];
  /** 需要和结果一起展示的说明。 */
  notes: string[];
  records: SqliteRecoveredRecord[];
  /** 超出上限（20,000 条）未返回的记录数。 */
  omitted: number;
  schemas: SqliteRecoveredSchema[];
}

/** 从 SQLite 库的空闲页、页内空闲区、溢出链、未检查点的 -wal 与回滚
 *  -journal 中恢复已删除的行，不含现存行。加密库传浏览器解密时用的同一
 *  `key`，后端会按页重新解密；缺少或密钥不对时 reject 且消息以
 *  `SQLITE_LOCKED` 开头。 */
export function fsSqliteDeletedRecords(args: {
  imageId: ImageId;
  nodeId: NodeId;
  parentId?: NodeId | null;
  key?: SqliteKey;
}): Promise<SqliteRecovery> {
  return invokeCmd<SqliteRecovery>('fs_sqlite_deleted_records', args);
}

/** 单张表的一页行数据（后端复用 `fsSqlitePreview` 导入好的预览 VFS 会话）。 */
export interface SqliteRows {
  columns: SqliteColumn[];
  rows: SqliteCell[][];
  /** 全表 COUNT(*)，驱动前端分页器。 */
  total: number;
  /** 回显的（已 clamp）偏移量。 */
  offset: number;
  returned: number;
}

/** 取某张表的一页行。`orderBy` 须为该表真实列名（否则后端忽略排序）；
 *  `limit` 后端硬上限 1000；加密库传同一 `key`。复用 fsSqlitePreview 的
 *  VFS 会话缓存，翻页/排序不重新读取整库。 */
export function fsSqliteTableRows(args: {
  imageId: ImageId;
  nodeId: NodeId;
  parentId?: NodeId | null;
  table: string;
  offset: number;
  limit: number;
  orderBy?: string | null;
  descending: boolean;
  key?: SqliteKey;
}): Promise<SqliteRows> {
  return invokeCmd<SqliteRows>('fs_sqlite_table_rows', args);
}

export type SqliteQueryParameter = string | number | boolean | null;
export type SqliteQueryCell = Exclude<SqliteCell, { kind: 'integer' }> | { kind: 'integer'; value: string };

export interface SqliteQueryPage {
  columns: string[];
  rows: SqliteQueryCell[][];
  offset: number;
  nextOffset: number | null;
  shortenedCells: boolean;
  sidecars: string[];
}

export function fsSqliteQuery(args: {
  imageId: ImageId;
  nodeId: NodeId;
  parentId?: NodeId | null;
  sql: string;
  parameters?: SqliteQueryParameter[];
  offset: number;
  limit: number;
  key?: SqliteKey;
}): Promise<SqliteQueryPage> {
  return invokeCmd<SqliteQueryPage>('fs_sqlite_query', args);
}
