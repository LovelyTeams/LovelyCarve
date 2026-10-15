import { sourceText, translate, type Translator } from '../../../i18n/translate';
import type { FsTypeIdent, StoredDigest } from '../../../lib/ipc/fs/derived';
import type { IconCategory } from '../../../utils/fileCategory';
import { shouldShowDirectoryWithEmptyFilter } from './emptyDirectoryFilter';
import { extOf, kindLabel, nameQueryMatcher, isHostDockerContainerRootId, sortValueOf, type DiskFsNode as FsNode, type DiskFileSortContext } from './model';
import type { TimeColumnKey } from './filters';

export interface ClientFileRows {
  entries: FsNode[];
  deepsearch: boolean;
  directoryClientFilter: boolean;
  effectiveFilters: Readonly<Record<string, string>>;
  digestCache: ReadonlyMap<string, StoredDigest>;
  ftypeCache: ReadonlyMap<string, FsTypeIdent>;
  tagOfNode: (node: FsNode) => string | undefined;
  fullPathOf: (node: FsNode) => string;
  selectedCategories: ReadonlySet<IconCategory>;
  categoryOfNode: (node: FsNode) => IconCategory;
  sizeFilter: { active: boolean };
  appliedSizePredicate: () => ((bytes: number) => boolean) | null;
  activeTimeFilterKeys: readonly TimeColumnKey[];
  matchesTimeFilter: (node: FsNode, key: TimeColumnKey) => boolean;
  hideEmptyDirs: boolean;
  stackDepth: number;
  emptySubtree: Readonly<Record<string, boolean>>;
  dirCounts: Readonly<Record<string, number>>;
  sortKey: string | null;
  sortDir: 'asc' | 'desc';
  sortContext: DiskFileSortContext;
}

/** Full-directory projection only; server-paged queries retain their authoritative order. */
export function projectFileRows(input: ClientFileRows, tr: Translator = sourceText): FsNode[] {
  const { entries, deepsearch, directoryClientFilter, effectiveFilters, digestCache, ftypeCache,
    tagOfNode, fullPathOf, selectedCategories, categoryOfNode, sizeFilter, appliedSizePredicate,
    activeTimeFilterKeys, matchesTimeFilter, hideEmptyDirs, stackDepth, emptySubtree, dirCounts,
    sortKey, sortDir, sortContext } = input;
  // 全显：过滤与排序都已在服务端完成（同一查询决定行序与总数），已加载页
  // 原样展示——在客户端再过滤只会让「无匹配」与「尚未加载」混淆。
  if (deepsearch || !directoryClientFilter) return entries;
  let out = entries.slice();

  // text filters（读 effectiveFilters：大列表下与输入解耦、按 200ms 去抖）
  // 通配符查询不 trim：锚定整名匹配下 "*.exe " 的尾空格是有意为之（追查尾空
  // 格规避名），trim 会悄悄改写语义；纯关键字仍 trim 保持旧行为。
  const rawNameQ = effectiveFilters.name ?? '';
  const nameQ = (/[*?]/.test(rawNameQ) ? rawNameQ : rawNameQ.trim()).toLowerCase();
  if (nameQ) {
    const nameMatches = nameQueryMatcher(nameQ);
    out = out.filter(n => nameMatches(n.name.toLowerCase()));
  }
  const extQ = effectiveFilters.ext?.trim().toLowerCase();
  if (extQ) out = out.filter(n => extOf(n).toLowerCase().includes(extQ));
  const kindQ = effectiveFilters.kind?.trim().toLowerCase();
  if (kindQ) out = out.filter(node => {
    const source = kindLabel(node);
    return source.includes(kindQ) || translate(source, 'en').toLowerCase().includes(kindQ);
  });
  const inodeQ = effectiveFilters.inode?.trim();
  if (inodeQ) out = out.filter(n => String(n.inode ?? '').includes(inodeQ));
  const linkQ = effectiveFilters.link?.trim().toLowerCase();
  if (linkQ) out = out.filter(n => (n.link_target ?? '').toLowerCase().includes(linkQ));
  const md5Q = effectiveFilters.md5?.trim().toLowerCase();
  if (md5Q) out = out.filter(n => digestCache.get(n.id)?.md5?.toLowerCase().includes(md5Q) ?? false);
  const sha1Q = effectiveFilters.sha1?.trim().toLowerCase();
  if (sha1Q) out = out.filter(n => digestCache.get(n.id)?.sha1?.toLowerCase().includes(sha1Q) ?? false);
  const sha256Q = effectiveFilters.sha256?.trim().toLowerCase();
  if (sha256Q) {
    out = out.filter(n => digestCache.get(n.id)?.sha256?.toLowerCase().includes(sha256Q) ?? false);
  }
  const sm3Q = effectiveFilters.sm3?.trim().toLowerCase();
  if (sm3Q) out = out.filter(n => digestCache.get(n.id)?.sm3?.toLowerCase().includes(sm3Q) ?? false);
  const tagQ = effectiveFilters.tag?.trim();
  if (tagQ) out = out.filter(node => {
    const tag = tagOfNode(node);
    return !!tag && (tag.includes(tagQ) || tr(tag).includes(tagQ) || translate(tag, 'en').includes(tagQ));
  });
  const ftypeQ = effectiveFilters.ftype?.trim().toLowerCase();
  if (ftypeQ) {
    out = out.filter(n => {
      const t = ftypeCache.get(n.id);
      if (!t) return false;
      return t.label.toLowerCase().includes(ftypeQ) || t.mime_type.toLowerCase().includes(ftypeQ);
    });
  }
  const pathQ = effectiveFilters.path?.trim().toLowerCase();
  if (pathQ) {
    if (pathQ.startsWith('/')) {
      // 路径形态：锚定前缀匹配，"/etc/" 不再误中 "/backup/etc/"。统一补一个
      // 尾 "/" 再比，查询带尾 "/" 时目录自身行（路径无尾斜杠）也算命中。
      out = out.filter(n => `${fullPathOf(n).toLowerCase()}/`.startsWith(pathQ));
    } else {
      out = out.filter(n => fullPathOf(n).toLowerCase().includes(pathQ));
    }
  }
  const deletedQ = effectiveFilters.deleted?.trim();
  if (deletedQ) {
    const want = deletedQ === '是' || deletedQ.toLowerCase() === 'true' || deletedQ === '1';
    out = out.filter(n => n.deleted === want);
  }
  // 文件分类（多选取并集）：只对文件生效，目录/符号链接始终保留，便于在筛出
  // 某类文件后仍能逐级进出目录。空集不过滤。
  if (selectedCategories.size > 0) {
    const sel = selectedCategories;
    out = out.filter(n => n.kind !== 'file' || sel.has(categoryOfNode(n)));
  }
  if (sizeFilter.active) {
    const pred = appliedSizePredicate();
    // 目录的 size 是目录块大小，对取证筛选没有意义，大小过滤一律排除目录。
    if (pred) out = out.filter(n => n.kind !== 'dir' && pred(n.size));
  }

  // 时间范围过滤：读「激活」时的边界快照，与展示格式做字典序比较（该格式
  // 字典序即时间序）。缺时间戳或格式异常的条目无法落入范围，一律排除。
  for (const key of activeTimeFilterKeys) {
    out = out.filter(node => matchesTimeFilter(node, key));
  }

  // 隐藏空目录：开关开启、且不在根（根列出的是分区，不该按空隐藏整卷）时，
  // 丢掉「整棵子树无文件」的目录行。优先用快照子树判定（emptySubtree，含递归
  // 只有空目录的情形，如 sbin / usr）；尚未判定时保留导航入口，
  // 待可见行计数返回再隐藏确认为空的目录。Docker 容器根是必须可见的导航/
  // 挂载入口，不参与此过滤；文件 / 符号链接也一律保留。
  if (hideEmptyDirs && stackDepth > 1) {
    out = out.filter(n => {
      if (n.kind !== 'dir' || isHostDockerContainerRootId(n.id)) return true;
      return (emptySubtree[n.id] == null && dirCounts[n.id] == null)
        || shouldShowDirectoryWithEmptyFilter(emptySubtree[n.id], dirCounts[n.id]);
    });
  }

  // sort: dirs always first, then sort key
  const sk = sortKey;
  if (!sk) {
    out.sort((a, b) => {
      const ad = a.kind === 'dir' ? 0 : 1;
      const bd = b.kind === 'dir' ? 0 : 1;
      return ad - bd || a.name.localeCompare(b.name);
    });
    return out;
  }
  // Schwartzian：先一次性算出每行的排序键，比较器只比键。比较器内逐次
  // sortValue（按路径排序时每次比较重拼 fullPathOf）在百万行上是数量级差距。
  const dir = sortDir === 'asc' ? 1 : -1;
  const keyed: Array<{ d: number; k: string | number; n: FsNode }> = new Array(out.length);
  for (let i = 0; i < out.length; i++) {
    const n = out[i];
    keyed[i] = { d: n.kind === 'dir' ? 0 : 1, k: sortValueOf(n, sk, sortContext), n };
  }
  keyed.sort((a, b) => a.d - b.d || (a.k < b.k ? -1 : a.k > b.k ? 1 : 0) * dir);
  for (let i = 0; i < keyed.length; i++) out[i] = keyed[i].n;
  return out;
}
