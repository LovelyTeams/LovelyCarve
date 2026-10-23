import { invokeCmd } from '../client';
import { type DockerIndexStatus, type FsNode, type FlatFileFilter, type FlatSortColumn, type Page, type FsLocateResult, type FsFlatPage } from './types';


/** Prepare only this container; success includes its persisted directory index. */
export function fsDockerPrepare(args: { imageId: string; rootId: string; caseDir: string | null; mode: 'status' | 'build' }): Promise<DockerIndexStatus> {
  return invokeCmd('fs_docker_prepare', args);
}

export function fsChildren(args: { imageId: string; parentId: string | null }): Promise<FsNode[]> {
  return invokeCmd<FsNode[]>('fs_children', args);
}

/** Cursor-paged directory listing. The first page uses `cursor: null`;
 *  subsequent pages pass the returned `nextCursor`. Results are sorted by
 *  stable `(name, id)` keys, so huge single directories can be consumed
 *  without one large IPC response. The legacy `fsChildren` remains available
 *  for existing full-directory callers. */
export function fsChildrenPage(args: {
  imageId: string;
  parentId: string | null;
  cursor?: string | null;
  pageSize?: number | null;
  query?: { filter?: FlatFileFilter; sort?: FlatSortColumn | null; descending?: boolean };
}): Promise<Page<FsNode>> {
  return invokeCmd<Page<FsNode>>('fs_children_page', args);
}

/** 把一个节点解析为可导航的祖先帧链（全局搜索跳转定位）。文件走案件
 *  filedb 的 pid 链；分区/未分配/磁盘间隙虚拟节点按 id 前缀合成。 */
export function fsNodeLocate(args: {
  imageId: string;
  nodeId: string;
}): Promise<FsLocateResult> {
  return invokeCmd<FsLocateResult>('fs_node_locate', args);
}

/** Resolve plugin provenance directly in its evidence volume, without filedb. */
export function fsSourceLocate(args: { imageId: string; rootId: string; path: string }): Promise<FsLocateResult> {
  return invokeCmd<FsLocateResult>('fs_source_locate', args);
}

/** 文件全显的一页。与逐级浏览(`fsChildren`)的差别只有一个布尔:前端叫
 *  `deepsearch`,后端叫 `allowAllFiles`;起点 `parentId` 仍传当前节点,置位
 *  后语义从「列一层」变成「以该节点为根递归平铺文件」。`skip`/`limit` 分页,
 *  `keyword` 命中名称或路径,`sort`/`sortdesc` 服务端排序,`filter` 是列漏斗
 *  的表达式过滤;`refresh` 让后端丢掉该起点的实时遍历缓存重走。 */
export function fsFlatPage(args: {
  imageId: string;
  parentId: string;
  deepsearch: boolean;
  keyword?: string | null;
  sort?: FlatSortColumn | null;
  sortdesc?: boolean;
  skip: number;
  limit: number;
  filter?: FlatFileFilter;
  refresh?: boolean;
}): Promise<FsFlatPage> {
  const { deepsearch, ...rest } = args;
  return invokeCmd<FsFlatPage>('fs_flat_page', { ...rest, allowAllFiles: deepsearch });
}

/** Batch directory child-counts served from the active case's volume snapshot.
 *  Returns a map of `parentId → count` containing ONLY the ids a snapshot
 *  covers; uncovered ids (no case, still building, truncated) are omitted, so
 *  the caller falls back to a bounded live probe just for those. Replaces the
 *  per-subdir `fsChildren` count fan-out whenever a snapshot exists. */
export function fsChildCounts(
  args: { imageId: string; parentIds: string[] },
): Promise<Record<string, number>> {
  return invokeCmd<Record<string, number>>('fs_child_counts', args);
}

/** Batch "is this directory's subtree empty of files?" served from the volume
 *  snapshot. Returns a map of `parentId → isEmpty` for ONLY the ids a snapshot
 *  can decide: `true` = the subtree holds nothing but (recursively) empty
 *  directories, `false` = a real file exists somewhere beneath it. Uncovered
 *  ids (no case, still building, truncated) are omitted, so the caller falls
 *  back to its direct child-count signal for those. Drives the directory tree's
 *  "hide empty directories" filter, which hides a dir when its whole subtree is
 *  file-less (not just when it is directly empty). */
export function fsSubtreeEmptyDirs(
  args: { imageId: string; parentIds: string[] },
): Promise<Record<string, boolean>> {
  return invokeCmd<Record<string, boolean>>('fs_subtree_empty_dirs', args);
}
