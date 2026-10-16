import { expect, it } from 'vitest';
import { artifactEntry, buildFilesystemTree, entryCategory, entryMatches, groupArtifacts, isRecoverable } from './recoveryTree';
import type { CarveArtifact, RecoveryCandidate, RecoveryScan } from '../../../lib/ipc/carve';
import type { FsNode } from '../../../lib/ipc/fs/types';
function node(id: string, name: string, parent: string, kind: 'file' | 'dir', deleted = false): FsNode { return { id, name, parent_id: parent, kind, size: kind === 'file' ? 10 : 0, deleted, inode: null, timestamps: { modified: null, accessed: null, created: null, mft_changed: null, birth: null } }; }
function candidate(id: string, parent: string, name: string): RecoveryCandidate { return { id, volume_index: 0, node: node(`file:${id}`, name, parent, 'file'), path: `/temp/${name}`, readable: true, initialized_size: null }; }
function scan(): RecoveryScan { return { scan_id: 'scan', image_id: 'image', options: { fat: true, ntfs: true, exfat: true, include_existing: false }, ranges: [], volumes: [{ partition: { id: 0, kind: 'ntfs', start_lba: 32768, length_lba: 100, label: 'Recovered' }, lost: true, reconstruction: { mft_offset: 100, cluster_size: 4096, record_size: 1024, sector_size: 512 }, error: null }], candidates: [candidate('one', 'dir:1', 'live.txt'), candidate('two', 'dir:2', 'deleted.txt')], directories: [{ volume_index: 0, path: '/temp', node: node('dir:1', 'temp', 'part:0', 'dir') }, { volume_index: 0, path: '/temp', node: node('dir:2', 'temp', 'part:0', 'dir', true) }], truncated: false, warnings: [], read_faults: { bad_regions: [], synthetic_bytes: 0, read_faults_detected: false } }; }
function artifact(id: string, format: string, offset: number): CarveArtifact { return { id, name: `f${offset}.${format}`, format_id: format, extension: format, category: format === 'pdf' ? 'document' : 'image', offset, length: 5, confidence: 90, region: '整盘', read_fault: false }; }
const label = (format: string) => format.toUpperCase();

it('preserves two same-name directories by record identity and totals their files', () => {
  const tree = buildFilesystemTree(scan());
  expect(tree.children.get('v:0')?.map(entry => [entry.key, entry.name])).toEqual([['d:0:dir:1', 'temp'], ['d:0:dir:2', 'temp']]);
  expect(tree.children.get('d:0:dir:1')?.map(entry => entry.key)).toEqual(['f:one']);
  expect(tree.children.get('d:0:dir:2')?.map(entry => entry.key)).toEqual(['f:two']);
  expect([tree.roots[0]?.fileCount, tree.roots[0]?.bytes, tree.roots[0]?.name]).toEqual([2, 20, 'Recovered']);
});

it('bounds cyclic directory references and keeps every file reachable from its volume', () => {
  const result = scan(); result.directories[0]!.node.parent_id = 'dir:2'; result.directories[1]!.node.parent_id = 'dir:1';
  const tree = buildFilesystemTree(result);
  expect(tree.roots[0]?.fileCount).toBe(2);
});

it('fills missing ancestors from the path and names same-label volumes by start sector', () => {
  const result = scan();
  result.volumes.push({ ...result.volumes[0]!, partition: { ...result.volumes[0]!.partition, start_lba: 99 } });
  result.candidates.push({ ...candidate('three', 'missing', 'orphan.txt'), path: '/lost/dir/orphan.txt' });
  const tree = buildFilesystemTree(result);
  expect(tree.roots.map(root => root.name)).toEqual(['Recovered @ LBA 32768', 'Recovered @ LBA 99']);
  expect(tree.entries.get('f:three')?.parentKey).toBe('p:v:0:/lost/dir');
  expect(tree.entries.get('p:v:0:/lost/dir')?.parentKey).toBe('p:v:0:/lost');
});

it('groups signature results by format with lazily created file rows', () => {
  const groups = groupArtifacts([artifact('a', 'png', 10), artifact('b', 'pdf', 20), artifact('c', 'png', 30)], label, true);
  expect(groups.root?.fileCount).toBe(3);
  expect(groups.formats.map(entry => [entry.key, entry.name, entry.fileCount])).toEqual([['sig:pdf', 'PDF', 1], ['sig:png', 'PNG', 2]]);
  const row = artifactEntry(groups.byId.get('c')!, label);
  expect([row.key, row.parentKey, row.path, isRecoverable(row), entryCategory(row)]).toEqual(['a:c', 'sig:png', '/PNG/f30.png', true, 'image']);
  expect(entryMatches(row, '0x000000000000001e')).toBe(true);
  expect(groupArtifacts([], label, false).root).toBeNull();
  expect(groupArtifacts([], label, true).root?.fileCount).toBe(0);
});

it('maps filesystem files onto carving categories and honours readable mappings', () => {
  const tree = buildFilesystemTree(scan());
  const file = tree.entries.get('f:one')!;
  expect([entryCategory(file), entryCategory({ ...file, name: 'photo.JPG' }), entryCategory(tree.roots[0]!)]).toEqual([null, 'image', null]);
  expect(isRecoverable(file)).toBe(true);
  expect(isRecoverable({ ...file, candidate: { ...file.candidate!, readable: false } })).toBe(false);
});
