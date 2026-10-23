import { describe, expect, it } from 'vitest';
import {
  buildFlatFilter,
  flatRowKey,
  flatSortColumn,
  parseDeletedFilter,
  type FlatFilterSources,
} from './flatQuery';

function sources(overrides: Partial<FlatFilterSources> = {}): FlatFilterSources {
  const inactiveTime = { active: false, appliedFrom: null, appliedTo: null };
  return {
    keyword: '',
    columnFilters: {},
    sizeFilter: { active: false, appliedLe: null, appliedGe: null, appliedMod: null },
    timeFilters: { created: inactiveTime, accessed: inactiveTime, modified: inactiveTime },
    ...overrides,
  };
}

describe('buildFlatFilter', () => {
  it('omits every inactive condition so no filter is an empty object', () => {
    expect(buildFlatFilter(sources())).toEqual({});
    // 未激活的大小 / 时间状态即便带着旧的 applied 值也不发。
    expect(
      buildFlatFilter(
        sources({
          sizeFilter: { active: false, appliedLe: 10, appliedGe: 1, appliedMod: { n: 512, r: 0 } },
          timeFilters: {
            created: { active: false, appliedFrom: '2026-01-01 00:00:00', appliedTo: null },
            accessed: { active: false, appliedFrom: null, appliedTo: null },
            modified: { active: false, appliedFrom: null, appliedTo: null },
          },
        }),
      ),
    ).toEqual({});
  });

  it('maps keyword, column funnels, size and time state onto the wire filter', () => {
    const filter = buildFlatFilter(
      sources({
        keyword: '  Evidence ',
        columnFilters: {
          name: '*.pdf ',
          path: ' /Windows/System32 ',
          ext: 'PD',
          kind: '文件',
          link: 'etc',
          inode: '42',
          deleted: '是',
          md5: 'ignored-by-server',
        },
        sizeFilter: { active: true, appliedLe: 2048, appliedGe: 1024, appliedMod: { n: 512, r: 0 } },
        timeFilters: {
          created: { active: true, appliedFrom: '2026-01-01 00:00:00', appliedTo: null },
          accessed: { active: false, appliedFrom: null, appliedTo: null },
          modified: { active: true, appliedFrom: null, appliedTo: '2026-03-02 23:59:59' },
        },
      }),
    );
    expect(filter).toEqual({
      keyword: 'Evidence',
      // 带通配符的名称模式原样保留(尾随空格可能是模式的一部分)。
      nameQuery: '*.pdf ',
      pathQuery: '/Windows/System32',
      extQuery: 'PD',
      kindQuery: '文件',
      linkQuery: 'etc',
      inodeQuery: '42',
      deleted: true,
      sizeGe: 1024,
      sizeLe: 2048,
      sizeMod: { divisor: 512, remainder: 0 },
      timeCreated: { from: '2026-01-01 00:00:00', to: null },
      timeModified: { from: null, to: '2026-03-02 23:59:59' },
    });
  });
});

describe('helpers', () => {
  it('parses the deleted funnel like the level-browsing filter', () => {
    expect(parseDeletedFilter('')).toBeNull();
    expect(parseDeletedFilter('  ')).toBeNull();
    expect(parseDeletedFilter('是')).toBe(true);
    expect(parseDeletedFilter('TRUE')).toBe(true);
    expect(parseDeletedFilter('1')).toBe(true);
    expect(parseDeletedFilter('否')).toBe(false);
  });

  it('only exposes server-sortable columns and builds stable row keys', () => {
    expect(flatSortColumn('name')).toBe('name');
    expect(flatSortColumn('modified')).toBe('modified');
    expect(flatSortColumn('md5')).toBeNull();
    expect(flatSortColumn('index')).toBeNull();
    expect(flatSortColumn(null)).toBeNull();
    expect(flatRowKey(7, 'ntfs:0:42')).toBe('flat:7:ntfs:0:42');
  });
});
