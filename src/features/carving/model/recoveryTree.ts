import { sourceText, type Translator } from '../../../i18n/translate';
import type { CarveArtifact, CarveCategory, RecoveryCandidate, RecoveryScan, RecoveryVolume } from '../../../lib/ipc/carve';
import type { FsNode } from '../../../lib/ipc/fs/types';
import { categoryOfName } from '../../../utils/fileCategory';
import { formatOffset } from './carveView';

/** Virtual root above the recovered volumes and the signature results. */
export const RECOVERY_ROOT = '/';
export const SIGNATURE_ROOT = 'sig';

export type RecoveryEntryKind = 'volume' | 'signatures' | 'format' | 'dir' | 'file';

export interface RecoveryEntry {
  key: string;
  /** `null` for entries directly under {@link RECOVERY_ROOT}. */
  parentKey: string | null;
  name: string;
  /** Path inside its volume or the signature results; "/" for roots. */
  path: string;
  kind: RecoveryEntryKind;
  node: FsNode | null;
  candidate: RecoveryCandidate | null;
  artifact: CarveArtifact | null;
  volume: RecoveryVolume | null;
  volumeIndex: number | null;
  fileCount: number;
  bytes: number;
}

export interface RecoveryTree { entries: Map<string, RecoveryEntry>; children: Map<string, RecoveryEntry[]>; roots: RecoveryEntry[] }

/** Signature results grouped by format; artifact rows are created on demand. */
export interface SignatureGroups {
  root: RecoveryEntry | null;
  formats: RecoveryEntry[];
  byFormat: Map<string, CarveArtifact[]>;
  byId: Map<string, CarveArtifact>;
}

export const EMPTY_RECOVERY_TREE: RecoveryTree = { entries: new Map(), children: new Map(), roots: [] };

function entry(fields: Partial<RecoveryEntry> & Pick<RecoveryEntry, 'key' | 'name' | 'path' | 'kind'>): RecoveryEntry {
  return { parentKey: null, node: null, candidate: null, artifact: null, volume: null, volumeIndex: null, fileCount: 0, bytes: 0, ...fields };
}

function volumeNames(volumes: RecoveryVolume[]): string[] {
  const names = volumes.map(volume => volume.partition.label?.trim() || volume.partition.kind.toUpperCase());
  // Same-name volumes (several lost NTFS volumes) stay distinguishable by start sector.
  return names.map((name, index) => names.indexOf(name) === names.lastIndexOf(name) ? name : `${name} @ LBA ${volumes[index]!.partition.start_lba}`);
}

/** Record identities keep same-name live/deleted directories separate. Paths
 * only supply missing ancestors when the original parent record is unavailable. */
export function buildFilesystemTree(scan: RecoveryScan | null): RecoveryTree {
  if (!scan) return EMPTY_RECOVERY_TREE;
  const entries = new Map<string, RecoveryEntry>();
  const paths = new Map<string, string>();
  const names = volumeNames(scan.volumes);
  const roots = scan.volumes.map((volume, index) => entry({ key: `v:${index}`, name: names[index]!, path: '/', kind: 'volume', volume, volumeIndex: index }));
  for (const root of roots) entries.set(root.key, root);
  const directoryKey = (volume: number, id: string) => `d:${volume}:${id}`;
  for (const directory of scan.directories ?? []) {
    const key = directoryKey(directory.volume_index, directory.node.id);
    entries.set(key, entry({ key, name: directory.node.name, path: directory.path, kind: 'dir', node: directory.node, volumeIndex: directory.volume_index }));
    const pathKey = `${directory.volume_index}:${directory.path}`;
    if (!paths.has(pathKey)) paths.set(pathKey, key);
  }
  function ensurePath(volume: number, path: string): string {
    const rootKey = `v:${volume}`;
    const components = path.split('/').filter(Boolean).slice(0, 64);
    let parent = rootKey; let current = '';
    for (const name of components) {
      current += `/${name}`;
      const known = paths.get(`${volume}:${current}`);
      if (known) { parent = known; continue; }
      const key = `p:${rootKey}:${current}`;
      if (!entries.has(key)) entries.set(key, entry({ key, parentKey: parent, name, path: current, kind: 'dir', volumeIndex: volume }));
      parent = key;
    }
    return parent;
  }
  const parentPath = (path: string) => path.slice(0, path.lastIndexOf('/'));
  for (const directory of scan.directories ?? []) {
    const current = entries.get(directoryKey(directory.volume_index, directory.node.id))!;
    const parent = directory.node.parent_id ? directoryKey(directory.volume_index, directory.node.parent_id) : '';
    current.parentKey = entries.has(parent) && parent !== current.key ? parent : ensurePath(directory.volume_index, parentPath(directory.path));
  }
  for (const candidate of scan.candidates) {
    if (!entries.has(`v:${candidate.volume_index}`)) continue;
    const parent = candidate.node.parent_id !== null ? directoryKey(candidate.volume_index, candidate.node.parent_id) : '';
    const key = `f:${candidate.id}`;
    entries.set(key, entry({
      key, parentKey: entries.has(parent) ? parent : ensurePath(candidate.volume_index, parentPath(candidate.path)),
      name: candidate.node.name, path: candidate.path, kind: 'file', node: candidate.node, candidate,
      volumeIndex: candidate.volume_index, fileCount: 1, bytes: candidate.node.size,
    }));
  }
  // Bound malformed parent cycles before flattening or calculating totals.
  for (const current of entries.values()) {
    if (current.kind === 'volume') continue;
    const seen = new Set([current.key]); let parent = current.parentKey; let depth = 0;
    while (parent !== null) {
      if (seen.has(parent) || depth++ >= 64) { current.parentKey = `v:${current.volumeIndex}`; break; }
      seen.add(parent); parent = entries.get(parent)?.parentKey ?? null;
    }
  }
  const children = new Map<string, RecoveryEntry[]>();
  for (const current of entries.values()) {
    if (current.parentKey !== null) { const list = children.get(current.parentKey) ?? []; list.push(current); children.set(current.parentKey, list); }
    if (current.kind !== 'file') continue;
    let parent = current.parentKey;
    while (parent !== null) { const ancestor = entries.get(parent); if (!ancestor) break; ancestor.fileCount++; ancestor.bytes += current.bytes; parent = ancestor.parentKey; }
  }
  for (const list of children.values()) list.sort(compareEntries);
  return { entries, children, roots };
}

/** Directories first, then natural name order. */
export function compareEntries(left: RecoveryEntry, right: RecoveryEntry): number {
  return Number(left.kind === 'file') - Number(right.kind === 'file') || left.name.localeCompare(right.name, undefined, { numeric: true });
}

/** Groups loaded signature results by format; `visible` keeps an empty root for a running carve. */
export function groupArtifacts(artifacts: readonly CarveArtifact[], formatLabel: (formatId: string) => string, visible: boolean, tr: Translator = sourceText): SignatureGroups {
  const byFormat = new Map<string, CarveArtifact[]>();
  const byId = new Map<string, CarveArtifact>();
  let bytes = 0;
  for (const artifact of artifacts) {
    byId.set(artifact.id, artifact);
    bytes += artifact.length;
    const list = byFormat.get(artifact.format_id);
    if (list) list.push(artifact); else byFormat.set(artifact.format_id, [artifact]);
  }
  if (!visible && artifacts.length === 0) return { root: null, formats: [], byFormat, byId };
  const root = entry({ key: SIGNATURE_ROOT, name: tr("签名恢复"), path: '/', kind: 'signatures', fileCount: artifacts.length, bytes });
  const formats = [...byFormat].map(([formatId, list]) => entry({
    key: `${SIGNATURE_ROOT}:${formatId}`, parentKey: SIGNATURE_ROOT, name: formatLabel(formatId), path: `/${formatLabel(formatId)}`,
    kind: 'format', fileCount: list.length, bytes: list.reduce((total, artifact) => total + artifact.length, 0),
  })).sort(compareEntries);
  return { root, formats, byFormat, byId };
}

export function artifactEntry(artifact: CarveArtifact, formatLabel: (formatId: string) => string): RecoveryEntry {
  return entry({
    key: `a:${artifact.id}`, parentKey: `${SIGNATURE_ROOT}:${artifact.format_id}`, name: artifact.name,
    path: `/${formatLabel(artifact.format_id)}/${artifact.name}`, kind: 'file', artifact, fileCount: 1, bytes: artifact.length,
  });
}

/** Filesystem files map their extension onto the carving categories; folders have none. */
export function entryCategory(item: RecoveryEntry): CarveCategory | null {
  if (item.artifact) return item.artifact.category;
  if (item.kind !== 'file') return null;
  switch (categoryOfName(item.name)) {
    case 'image': return 'image';
    case 'video': return 'video';
    case 'audio': return 'audio';
    case 'pdf': case 'document': case 'spreadsheet': return 'document';
    case 'archive': return 'archive';
    case 'database': return 'database';
    case 'code': case 'executable': case 'email': case 'other': return null;
  }
}

/** Name, path, extension, format or start offset contain the lower-cased needle. */
export function entryMatches(item: RecoveryEntry, needle: string): boolean {
  if (item.name.toLocaleLowerCase().includes(needle) || item.path.toLocaleLowerCase().includes(needle)) return true;
  const artifact = item.artifact;
  return !!artifact && (artifact.extension.toLocaleLowerCase().includes(needle) || artifact.format_id.toLocaleLowerCase().includes(needle)
    || artifact.region.toLocaleLowerCase().includes(needle) || formatOffset(artifact.offset).toLocaleLowerCase().includes(needle));
}

/** Exportable files carry a readable mapping; signature results always do. */
export function isRecoverable(item: RecoveryEntry): boolean {
  return item.kind === 'file' && (item.artifact !== null || item.candidate?.readable === true);
}
