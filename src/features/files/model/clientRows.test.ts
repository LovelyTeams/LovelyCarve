import { describe, expect, it } from 'vitest';
import { translate } from '../../../i18n/translate';
import { projectFileRows, type ClientFileRows } from './clientRows';
import type { DiskFsNode } from './model';

function node(id: string, kind: DiskFsNode['kind']): DiskFsNode {
  return {
    id, kind, name: '文件列表', size: 1, deleted: false, parent_id: null, inode: null,
    timestamps: { created: null, modified: null, accessed: null, birth: null, mft_changed: null },
  };
}

function rows(kindQuery: string, tagQuery = ''): ClientFileRows {
  return {
    entries: [node('file', 'file'), node('folder', 'dir'), node('link', 'symlink'), node('unalloc:1', 'file')],
    deepsearch: false, directoryClientFilter: true, effectiveFilters: { kind: kindQuery, tag: tagQuery },
    digestCache: new Map(), ftypeCache: new Map(), tagOfNode: item => item.id === 'file' ? '重点' : undefined,
    fullPathOf: item => '/' + item.name, selectedCategories: new Set(), categoryOfNode: () => 'other',
    sizeFilter: { active: false }, appliedSizePredicate: () => null, activeTimeFilterKeys: [], matchesTimeFilter: () => true,
    hideEmptyDirs: false, stackDepth: 0, emptySubtree: {}, dirCounts: {}, sortKey: null, sortDir: 'asc',
    sortContext: { fullPathOf: item => '/' + item.name, tagOf: () => undefined, ftypeLabelOf: () => '', digestOf: () => undefined },
  };
}

describe('bilingual file filters', () => {
  it('accepts either language independently of the current interface language', () => {
    const english = (key: string) => translate(key, 'en');
    for (const [query, expected] of [
      ['file', 'file'], ['文件', 'file'], ['FOLDER', 'folder'], ['目录', 'folder'],
      ['symbolic', 'link'], ['符号', 'link'], ['unallocated', 'unalloc:1'], ['未分配空间', 'unalloc:1'],
    ]) {
      expect(projectFileRows(rows(query)).map(item => item.id), query).toEqual([expected]);
      expect(projectFileRows(rows(query), english).map(item => item.id), query).toEqual([expected]);
    }
    expect(projectFileRows(rows('file'), english)[0].name).toBe('文件列表');
  });

  it('filters translated tag labels while retaining the persisted Chinese tag', () => {
    const input = rows('', 'Important');
    expect(projectFileRows(input).map(item => item.id)).toEqual(['file']);
    expect(input.tagOfNode(input.entries[0])).toBe('重点');
  });
});
