import { sourceText, type Translator } from '../../../i18n/translate';
import { categoryOfName, iconCategoryOf, type IconCategory } from '../../../utils/fileCategory';
import type { FsSizeTreeEntry, FsSizeTreeExtension } from '../../../lib/ipc/fs/sizeTree';

/** 树图的配色类别：颜色回答「这是什么数据」。文件类别合成九组，每组一个色相，
 *  只有「其他」不带颜色。 */
export type TreemapKind = 'document' | 'image' | 'media' | 'code' | 'archive' | 'executable' | 'database' | 'system' | 'other';

/** 图例顺序。 */
export const TREEMAP_KINDS: readonly TreemapKind[] = ['document', 'image', 'media', 'code', 'archive', 'executable', 'database', 'system', 'other'];

const KIND_OF_CATEGORY: Record<IconCategory, TreemapKind> = {
  image: 'image',
  video: 'media',
  audio: 'media',
  pdf: 'document',
  document: 'document',
  spreadsheet: 'document',
  email: 'document',
  code: 'code',
  archive: 'archive',
  executable: 'executable',
  database: 'database',
  other: 'other',
};

export function treemapKindLabel(kind: TreemapKind, tr: Translator = sourceText): string {
  switch (kind) {
    case 'document': return tr('文档');
    case 'image': return tr('图片');
    case 'media': return tr('音视频');
    case 'code': return tr('代码/文本');
    case 'archive': return tr('压缩包');
    case 'executable': return tr('可执行');
    case 'database': return tr('数据库');
    case 'system': return tr('系统');
    case 'other': return tr('其他');
  }
}

/** 卷根下由名字决定的条目：分页、休眠与交换文件和卷影副本区。 */
const SYSTEM_ROOT_NAMES = new Set(['pagefile.sys', 'hiberfil.sys', 'swapfile.sys', 'system volume information']);

/** 卷根下按名字判为系统的条目：NTFS 元文件（`$MFT`、`$Extend` 等）与 Windows 的安装残留
 *  （`$WINDOWS.~BT`），以及分页、休眠文件和卷影副本区。`$Recycle.Bin` 装的是用户文件，
 *  按内容判定。`.sys` 扩展名本会把 `pagefile.sys` 算作可执行文件。 */
export function isSystemRootName(name: string): boolean {
  const lower = name.toLowerCase();
  return SYSTEM_ROOT_NAMES.has(lower) || (lower.startsWith('$') && lower !== '$recycle.bin');
}

/** 分区或逻辑卷的根节点（`part:N` / `lvm:N:M`）。 */
export function isVolumeRootId(id: string): boolean {
  return /^part:\d+$/.test(id) || /^lvm:\d+:\d+$/.test(id);
}

/** 树图目前只服务 NTFS：NTFS 目录，或列出了 NTFS 条目的卷根。卷根是否 NTFS 由后端最终判定。 */
export function treemapAvailableAt(directoryId: string, entries: readonly { id: string }[]): boolean {
  if (directoryId.startsWith('ntfs:')) return true;
  return isVolumeRootId(directoryId) && entries.some(entry => entry.id.startsWith('ntfs:') || entry.id.startsWith('ntfs-ads:'));
}

/** 当前目录是否在卷根下的系统区（如 `$Extend`、`System Volume Information`）之内：
 *  面包屑里卷根之后的第一级按名字判定。 */
export function inSystemArea(stack: readonly { id: string; name: string }[]): boolean {
  const first = stack.findIndex(frame => frame.id.startsWith('ntfs:'));
  return first >= 1 && isVolumeRootId(stack[first - 1].id) && isSystemRootName(stack[first].name);
}

export function kindOfExtension(extension: string): TreemapKind {
  return KIND_OF_CATEGORY[iconCategoryOf(extension.toLowerCase())];
}

/** 目录的类别：其下字节最多的那一类。 */
export function dominantKind(extensions: readonly FsSizeTreeExtension[]): TreemapKind {
  const totals = new Map<TreemapKind, number>();
  for (const { extension, bytes } of extensions) {
    const kind = kindOfExtension(extension);
    totals.set(kind, (totals.get(kind) ?? 0) + bytes);
  }
  let best: TreemapKind = 'other';
  let bestBytes = 0;
  for (const [kind, bytes] of totals) {
    if (bytes > bestBytes) {
      best = kind;
      bestBytes = bytes;
    }
  }
  return best;
}

/** 条目在树图里的位置：其祖先已判为系统时它也是系统；视图根是卷根时，
 *  顶层条目按名字判定。 */
export interface TreemapKindContext {
  system: boolean;
  volumeRoot: boolean;
  depth: number;
}

/** 条目的配色类别：系统区按名字，文件按扩展名，目录按其下的主要内容。
 *  NTFS 命名数据流的「扩展名」是名称的一部分，归为其他。 */
export function treemapKindOf(entry: FsSizeTreeEntry, context: TreemapKindContext): TreemapKind {
  if (context.system) return 'system';
  const node = entry.node;
  if (node && context.volumeRoot && context.depth === 0 && isSystemRootName(node.name)) return 'system';
  if (!node || node.kind === 'dir') return dominantKind(entry.extensions);
  if (node.id.startsWith('ntfs-ads:')) return 'other';
  return KIND_OF_CATEGORY[categoryOfName(node.name)];
}

/** 「时间」着色的分档：距卷内最新写入多久。 */
export const AGE_BUCKETS: readonly number[] = [7, 30, 182, 365, Number.POSITIVE_INFINITY];

export function ageBucketLabel(bucket: number, tr: Translator = sourceText): string {
  switch (bucket) {
    case 0: return tr('一周内');
    case 1: return tr('一月内');
    case 2: return tr('半年内');
    case 3: return tr('一年内');
    default: return tr('更早');
  }
}

/** `newest`（Unix 秒）落在哪一档，以卷内最新写入 `reference` 为「现在」；任一未知时为 null。 */
export function ageBucketOf(newest: number | null, reference: number | null): number | null {
  if (newest === null || reference === null) return null;
  const days = (reference - newest) / 86_400;
  const bucket = AGE_BUCKETS.findIndex(limit => days <= limit);
  return bucket < 0 ? AGE_BUCKETS.length - 1 : bucket;
}

/** Unix 秒 → 与列表一致的 `YYYY-MM-DD HH:MM:SS`（UTC）。 */
export function formatUnixSeconds(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return '—';
  const date = new Date(seconds * 1000);
  if (Number.isNaN(date.getTime())) return '—';
  return date.toISOString().replace('T', ' ').slice(0, 19);
}
