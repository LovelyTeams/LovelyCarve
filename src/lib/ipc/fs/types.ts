


export interface DockerIndexStatus {
  ready: boolean;
  persisted: boolean;
  entryCount: number;
}

export interface FsNode {
  id: string;
  name: string;
  parent_id: string | null;
  kind: 'dir' | 'file' | 'symlink' | 'unallocated';
  size: number;
  deleted: boolean;
  inode: number | null;
  timestamps: {
    modified: string | null;
    accessed: string | null;
    created: string | null;
    mft_changed: string | null;
    birth: string | null;
  };
  /** 符号链接指向的目标路径（链接内容原样，不解析）；非符号链接为 null */
  link_target?: string | null;
  /** 该文件是否拥有独立展示的命名流：ReFS 文件快照（refsutil streamsnapshot）
   *  或 NTFS 命名数据流（ADS）。伪文件行本身为 false。 */
  has_snapshot?: boolean;
}

/** 祖先帧：与文件列表 dirStack 帧同构（jump2Page 跳转定位用）。 */
export interface FsLocateNode {
  id: string;
  name: string;
  kind: string;
}

/** `fs_node_locate` 的返回：frames 为根→目标所在目录链（不含目标本身），
 *  fileId 是目标文件节点（目录定位时为 null）。 */
export interface FsLocateResult {
  node_id: string;
  volume_key: string;
  frames: FsLocateNode[];
  file_id: string | null;
}

export interface FsWalkEntry {
  node: FsNode;
  /** Path relative to the listing's start node, with a leading `/`. */
  path: string;
}

export interface Page<T> {
  items: T[];
  nextCursor: string | null;
  total?: number;
  truncated: boolean;
}

// ---- 文件全显(前端 deepsearch / 后端 allowAllFiles)。类型镜像
// core/flat_query.rs 的 FlatFileFilter / FsFlatPage(camelCase)。 ----------

/** 单个时间列的区间,边界为表格显示口径 `YYYY-MM-DD HH:MM:SS`(两端均含)。 */
export interface FlatTimeRange {
  from: string | null;
  to: string | null;
}

/** 字节数 `mod divisor = remainder`。 */
export interface FlatSizeMod {
  divisor: number;
  remainder: number;
}

/** 全显的服务端表达式过滤——各列漏斗的结构化状态,由后端在索引 / 快照 /
 *  实时遍历三条数据源上以同一语义应用。名称支持 `*` `?` 通配;路径以 `/`
 *  开头为「相对当前目录」的锚定前缀,否则子串。`extensions` 只保留所列扩展名,
 *  `otherCategoryExts` 另外保留无扩展名与不在其中的扩展名(两者取并集);文件列表
 *  不再发送这两项,CLI 的 `--filter` 仍可使用。 */
export interface FlatFileFilter {
  keyword?: string | null;
  nameQuery?: string | null;
  pathQuery?: string | null;
  extQuery?: string | null;
  kindQuery?: string | null;
  linkQuery?: string | null;
  inodeQuery?: string | null;
  deleted?: boolean | null;
  sizeGe?: number | null;
  sizeLe?: number | null;
  sizeMod?: FlatSizeMod | null;
  timeCreated?: FlatTimeRange | null;
  timeAccessed?: FlatTimeRange | null;
  timeModified?: FlatTimeRange | null;
  extensions?: string[];
  otherCategoryExts?: string[];
}

/** 全显可服务端排序的列(与后端 `FlatSortKey` 一致)。 */
export type FlatSortColumn =
  | 'kind'
  | 'name'
  | 'ext'
  | 'size'
  | 'created'
  | 'accessed'
  | 'modified'
  | 'link'
  | 'inode'
  | 'path'
  | 'deleted';

/** 全显的一页:`items` 路径相对起点;`total` 是过滤后的总命中数,与行同一次
 *  查询返回(窗口函数 count);`truncated` 仅实时遍历撞上安全上限时为 true。 */
export interface FsFlatPage {
  items: FsWalkEntry[];
  total: number;
  truncated: boolean;
}
