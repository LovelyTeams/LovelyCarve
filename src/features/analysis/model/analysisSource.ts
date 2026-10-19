import type { AnalysisRecord, RecordSource } from '../../../lib/ipc/analysisResults';
import type { FileLocationTarget } from '../../../lib/models/fileLocation';

export interface AnalysisSource {
  /** The path inside its volume, or the node id. */
  label: string;
  /** Where in the source the record is (`第 12 行`). */
  note: string | null;
  target: FileLocationTarget;
}

const text = (value: unknown): string => typeof value === 'string' && value.trim() ? value : '';

function located(source: RecordSource, imageId: string): AnalysisSource | null {
  const range = source.offset !== undefined ? { offset: source.offset, length: source.length } : {};
  const note = source.note ?? null;
  if (source.root && source.path) {
    return { label: source.path, note, target: { imageId, rootId: source.root, path: source.path, ...range } };
  }
  if (source.nodeId) {
    return { label: source.path || source.nodeId, note, target: { imageId, nodeId: source.nodeId, ...range } };
  }
  return null;
}

/** Results of an earlier version kept their provenance in attributes. */
function legacySources(record: AnalysisRecord, imageId: string): AnalysisSource[] {
  const values = record.values;
  const rootId = text(values['source.root']) || text(values['linux.root']);
  const path = text(values['source.path']);
  const declared = Array.isArray(values['source.paths']) ? values['source.paths'].filter((value): value is string => typeof value === 'string' && !!value.trim()) : [];
  const legacy = text(values['linux.source']);
  const legacyPaths = legacy.startsWith('systemd/') ? [] : legacy.split(';').filter(value => value.includes('/'));
  const paths = [...new Set(path ? [path, ...declared] : declared.length ? declared : legacyPaths)];
  if (rootId && paths.length) return paths.map(path => ({ label: path, note: null, target: { imageId, rootId, path } }));
  const nodeId = text(values['source.node']) || text(record.sources[0]?.nodeId);
  return nodeId ? [{ label: path || nodeId, note: null, target: { imageId, nodeId } }] : [];
}

/** Provenance is an evidence address: a path inside a volume or a file-tree
 *  node, never a label or a record id. */
export function analysisSources(record: AnalysisRecord, imageId: string): AnalysisSource[] {
  imageId = record.imageId || imageId;
  if (record.legacy) return legacySources(record, imageId);
  const seen = new Set<string>();
  const sources: AnalysisSource[] = [];
  for (const source of record.sources) {
    const found = located(source, imageId);
    if (!found) continue;
    const key = JSON.stringify([found.target, found.note]);
    if (seen.has(key)) continue;
    seen.add(key);
    sources.push(found);
  }
  return sources;
}
