import { invokeCmd } from '../client';
import type { FsNode } from './types';

/** 树图度量：`bytes` 按逻辑大小，`files` 按文件数。 */
export type FsSizeTreeMetric = 'bytes' | 'files';

/** 合并成一块的子项：过小或超出每个目录可显示数量的部分。 */
export interface FsSizeTreeOthers {
  count: number;
  bytes: number;
  files: number;
}

/** 视图里的一个条目，带其下全部内容的合计。镜像 `core::catalog::size_tree::SizeTreeEntry`。 */
export interface FsSizeTreeEntry {
  /** 条目自身的节点；卷根没有索引行，为 null。 */
  node: FsNode | null;
  bytes: number;
  files: number;
  /** 其下的目录数，不含自身。 */
  dirs: number;
  /** 其下最新的文件修改时间（Unix 秒）。 */
  newest: number | null;
  /** 显示的子项，从大到小；叶子与未展开的目录为 null。 */
  children: FsSizeTreeEntry[] | null;
  others: FsSizeTreeOthers | null;
}

export interface FsSizeTreeView {
  root: FsSizeTreeEntry;
  /** 索引来自达到条目上限而截断的遍历，合计可能偏小。 */
  truncated: boolean;
  /** 卷内最新的可信文件修改时间（Unix 秒），即「时间」着色的参照点。 */
  newestWrite: number | null;
  /** 索引所依据的卷快照的构建时间（RFC 3339）。 */
  builtAt: string;
}

/** `ready` 带数据；`unindexed` 表示卷索引尚未覆盖该目录（快照构建中、截断或目录已删除）；
 *  `unsupported` 表示不是 NTFS 目录。 */
export type FsSizeTreeResult =
  | { status: 'ready'; view: FsSizeTreeView }
  | { status: 'unindexed' }
  | { status: 'unsupported' };

/** 当前目录的树图数据：由卷索引一次聚合后缓存在后端，进出目录只做内存遍历。 */
export function fsSizeTree(args: {
  imageId: string;
  nodeId: string;
  depth?: number;
  metric?: FsSizeTreeMetric;
  includeDeleted?: boolean;
  minShare?: number;
}): Promise<FsSizeTreeResult> {
  return invokeCmd<FsSizeTreeResult>('fs_size_tree', args);
}
