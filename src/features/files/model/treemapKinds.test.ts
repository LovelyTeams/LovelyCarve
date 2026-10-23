import { describe, expect, it } from 'vitest';
import type { FsSizeTreeEntry } from '../../../lib/ipc/fs/sizeTree';
import type { FsNode } from '../../../lib/ipc/fs/types';
import {
  ageBucketOf, dominantKind, formatUnixSeconds, inSystemArea, isSystemRootName, isVolumeRootId,
  treemapAvailableAt, treemapKindOf,
} from './treemapKinds';

const node = (id: string, name: string, kind: FsNode['kind'] = 'file'): FsNode => ({
  id, name, kind, parent_id: 'part:2', size: 1, deleted: false, inode: null,
  timestamps: { modified: null, accessed: null, created: null, mft_changed: null, birth: null },
});
const entry = (value: FsNode | null, extensions: FsSizeTreeEntry['extensions'] = []): FsSizeTreeEntry => ({
  node: value, bytes: 1, files: 1, dirs: 0, newest: null, children: null, others: null, extensions,
});
const top = { system: false, volumeRoot: true, depth: 0 };

describe('treemap kinds', () => {
  it('colors files by extension and directories by what fills them', () => {
    expect(treemapKindOf(entry(node('ntfs:part:2:40', 'IMG_0001.JPG')), top)).toBe('image');
    expect(treemapKindOf(entry(node('ntfs:part:2:41', 'clip.mkv')), top)).toBe('media');
    expect(treemapKindOf(entry(node('ntfs:part:2:42', 'report.docx')), top)).toBe('document');
    const windows = entry(node('ntfs:part:2:43', 'Windows', 'dir'), [
      { extension: 'dll', bytes: 900 },
      { extension: 'log', bytes: 300 },
      { extension: 'txt', bytes: 200 },
    ]);
    expect(treemapKindOf(windows, top)).toBe('executable');
    // Two text extensions together outweigh the larger single image extension.
    expect(dominantKind([{ extension: 'log', bytes: 300 }, { extension: 'txt', bytes: 200 }, { extension: 'jpg', bytes: 400 }])).toBe('code');
    expect(dominantKind([])).toBe('other');
  });

  it('names NTFS metafiles and Windows system files at the volume root, not deeper', () => {
    expect(isSystemRootName('$MFT')).toBe(true);
    expect(isSystemRootName('pagefile.sys')).toBe(true);
    expect(isSystemRootName('System Volume Information')).toBe(true);
    expect(isSystemRootName('$Recycle.Bin')).toBe(false);
    expect(treemapKindOf(entry(node('ntfs:part:2:44', 'pagefile.sys')), top)).toBe('system');
    // Inside the recycle bin, `$R…` files are user data.
    expect(treemapKindOf(entry(node('ntfs:part:2:45', '$RQ2X8.docx')), { ...top, volumeRoot: false })).toBe('document');
    expect(treemapKindOf(entry(node('ntfs:part:2:46', 'notes.txt')), { ...top, system: true })).toBe('system');
    expect(treemapKindOf(entry(node('ntfs-ads:part:2:47:00', 'a.txt:hidden.exe (数据流)')), top)).toBe('other');
  });

  it('serves NTFS directories and NTFS volume roots only', () => {
    expect(isVolumeRootId('part:2')).toBe(true);
    expect(isVolumeRootId('lvm:1:0')).toBe(true);
    expect(isVolumeRootId('ntfs:part:2:5')).toBe(false);
    expect(treemapAvailableAt('ntfs:part:2:5', [])).toBe(true);
    expect(treemapAvailableAt('part:2', [node('ntfs:part:2:0', '$MFT')])).toBe(true);
    expect(treemapAvailableAt('part:1', [node('ext4:part:1:2', 'etc', 'dir')])).toBe(false);
    expect(treemapAvailableAt('root', [node('part:2', '分区 2', 'dir')])).toBe(false);
  });

  it('knows when the current directory lies in a system area', () => {
    const base = [{ id: 'root', name: '/' }, { id: 'part:2', name: 'C' }];
    expect(inSystemArea([...base, { id: 'ntfs:part:2:11', name: '$Extend' }, { id: 'ntfs:part:2:40', name: '$RmMetadata' }])).toBe(true);
    expect(inSystemArea([...base, { id: 'ntfs:part:2:60', name: 'Users' }])).toBe(false);
    expect(inSystemArea(base)).toBe(false);
  });
});

describe('treemap ages', () => {
  const day = 86_400;
  it('measures age from the volume newest write', () => {
    const now = 1_767_225_600;
    expect(ageBucketOf(now - 2 * day, now)).toBe(0);
    expect(ageBucketOf(now - 20 * day, now)).toBe(1);
    expect(ageBucketOf(now - 100 * day, now)).toBe(2);
    expect(ageBucketOf(now - 300 * day, now)).toBe(3);
    expect(ageBucketOf(now - 3000 * day, now)).toBe(4);
    expect(ageBucketOf(now + day, now)).toBe(0);
    expect(ageBucketOf(null, now)).toBeNull();
    expect(ageBucketOf(now, null)).toBeNull();
  });

  it('formats Unix seconds like the list', () => {
    expect(formatUnixSeconds(1_766_217_600)).toBe('2025-12-20 08:00:00');
    expect(formatUnixSeconds(null)).toBe('—');
  });
});
