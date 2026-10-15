import { sourceText, type Translator } from '../../../i18n/translate';
import type { GlobalSearchOptions, SearchCodePage } from '../../../lib/ipc/search';
import { parseSearchPatterns } from './searchResults';

export const CODE_PAGES: SearchCodePage[] = ['ASCII', 'UTF-8', 'UTF-16LE', 'UTF-16BE', 'GBK'];
/** 默认搜索编码页。 */
export const DEFAULT_CODE_PAGES: SearchCodePage[] = ['UTF-8', 'GBK', 'UTF-16LE', 'UTF-16BE'];
export const SCOPE_TABS = [
  { id: 'partitions', label: '按分区' },
  { id: 'tags', label: '按标签' },
  { id: 'tree_nodes', label: '按目录' },
] as const;
export type ScopeMode = (typeof SCOPE_TABS)[number]['id'];

export function createSearchDraft() {
  return {
    keysText: '',
    regexesText: '',
    codepages: [...DEFAULT_CODE_PAGES] as SearchCodePage[],
    scopeMode: 'partitions' as ScopeMode,
    partitions: [] as string[],
    tags: [] as string[],
    treeNodesText: '',
    matchCase: false,
    searchDeleted: true,
    searchEmptySpace: false,
    calcHash: false,
    stopAfterFirst: false,
    perFileCap: 100,
    globalCap: 200_000,
  };
}

export type SearchDraft = ReturnType<typeof createSearchDraft>;

export function searchConfigurationError(dialog: SearchDraft, tr: Translator = sourceText): string | null {
  return Object.values(searchConfigurationErrors(dialog, tr)).find(error => error !== null) ?? null;
}

/** Field errors shared by submission validation and accessible form feedback. */
export function searchConfigurationErrors(dialog: SearchDraft, tr: Translator = sourceText) {
  const keys = parseSearchPatterns(dialog.keysText);
  const regexes = parseSearchPatterns(dialog.regexesText);
  return {
    patterns: keys.length === 0 && regexes.length === 0 ? tr("至少输入一个关键词或正则") : null,
    codepages: dialog.codepages.length === 0 ? tr("至少选择一个编码页") : null,
    perFileCap: !Number.isInteger(dialog.perFileCap) || dialog.perFileCap < 1 ? tr("每文件上限须为正整数") : null,
    globalCap: !Number.isInteger(dialog.globalCap) || dialog.globalCap < 1 ? tr("全局上限须为正整数") : null,
  };
}

export function buildSearchOptions(dialog: SearchDraft): GlobalSearchOptions {
  const keys = parseSearchPatterns(dialog.keysText);
  const regexes = parseSearchPatterns(dialog.regexesText);
  const treeNodes =
    dialog.scopeMode === 'tree_nodes'
      ? parseSearchPatterns(dialog.treeNodesText).map(path =>
          path.startsWith('/') ? path : `/${path}`,
        )
      : [];
  return {
    keys,
    regexes,
    codepages: [...dialog.codepages],
    match_case: dialog.matchCase,
    search_deleted: dialog.searchDeleted,
    search_empty_space: dialog.searchEmptySpace,
    calc_hash: dialog.calcHash,
    scope: {
      partitions: dialog.scopeMode === 'partitions' ? [...dialog.partitions] : [],
      tags: dialog.scopeMode === 'tags' ? [...dialog.tags] : [],
      tree_nodes: treeNodes,
    },
    per_file_cap: dialog.perFileCap,
    stop_after_first: dialog.stopAfterFirst,
    global_cap: dialog.globalCap,
  };
}
