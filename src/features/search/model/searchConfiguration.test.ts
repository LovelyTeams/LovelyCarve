import { describe, expect, it } from 'vitest';
import { buildSearchOptions, createSearchDraft, searchConfigurationError } from './searchConfiguration';

describe('search draft serialization', () => {
  it('keeps default limits and encodings and creates independent mutable draft arrays', () => {
    const draft = createSearchDraft(); draft.keysText = 'key'; draft.regexesText = 'pass\\w+';
    expect(buildSearchOptions(draft)).toEqual({ keys: ['key'], regexes: ['pass\\w+'], codepages: ['UTF-8', 'GBK', 'UTF-16LE', 'UTF-16BE'], match_case: false, search_deleted: true, search_empty_space: false, calc_hash: false, scope: { partitions: [], tags: [], tree_nodes: [] }, per_file_cap: 100, stop_after_first: false, global_cap: 200000 });
    draft.codepages.pop(); expect(createSearchDraft().codepages).toHaveLength(4);
  });
  it('serializes only the selected scope and takes array snapshots', () => {
    const draft = createSearchDraft(); draft.keysText = 'key'; draft.scopeMode = 'tags'; draft.partitions = ['part:1']; draft.tags = ['重要']; draft.treeNodesText = 'Users/alice';
    const options = buildSearchOptions(draft); draft.tags.push('另一个');
    expect(options.scope).toEqual({ partitions: [], tags: ['重要'], tree_nodes: [] });
  });
  it('retains missing-pattern, encoding and positive integer limit validation', () => {
    const draft = createSearchDraft(); expect(searchConfigurationError(draft)).toBe('至少输入一个关键词或正则'); draft.keysText = 'key'; draft.codepages = [];
    expect(searchConfigurationError(draft)).toBe('至少选择一种编码'); draft.codepages = ['ASCII']; draft.perFileCap = 1.5;
    expect(searchConfigurationError(draft)).toBe('每文件上限须为正整数'); draft.perFileCap = 1; draft.globalCap = 0;
    expect(searchConfigurationError(draft)).toBe('全局上限须为正整数');
  });
});
