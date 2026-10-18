import { sourceText, type Translator } from '../../../i18n/translate';
import type { FsTypeIdent, HashAlgo } from '../../../lib/ipc/fs/derived';
import type { DiskFsNode as FsNode } from './model';

export function ftypeCellText(t: FsTypeIdent): string {
  return t.inferred ? `${t.label} ${(t.score * 100).toFixed(0)}%` : t.label;
}

export function ftypeTitle(t: FsTypeIdent, tr: Translator = sourceText): string {
  const parts = [t.description, t.mime_type];
  if (t.extensions.length > 0) parts.push(tr("常见扩展名 {0}", { 0: t.extensions.join('/') }));
  parts.push(t.inferred ? tr("模型置信度 {0}%", { 0: (t.score * 100).toFixed(1) }) : tr("规则判定"));
  return parts.join(' · ');
}

export const DERIVED_NODE_QUERY_LIMIT = 5_000;
export const HASH_ALGO_LABEL: Record<HashAlgo, string> = {
  md5: 'MD5',
  sha1: 'SHA1',
  sha256: 'SHA256',
  sm3: 'SM3',
};
export const ALL_HASH_ALGOS: HashAlgo[] = ['md5', 'sha1', 'sha256', 'sm3'];
export const HASH_MENU_OPTIONS: { label: string; algos: HashAlgo[] }[] = [
  ...ALL_HASH_ALGOS.map(a => ({ label: HASH_ALGO_LABEL[a], algos: [a] })),
  { label: '全部', algos: ALL_HASH_ALGOS },
];

export function fileNodeIdsForDerivedHydration(nodes: FsNode[], alreadyHydrated: Set<string>): string[] {
  const seen = new Set<string>();
  const ids: string[] = [];
  for (const node of nodes) {
    if (node.kind !== 'file' || seen.has(node.id) || alreadyHydrated.has(node.id)) continue;
    seen.add(node.id);
    ids.push(node.id);
  }
  return ids;
}

export function chunkNodeIds(nodeIds: string[]): string[][] {
  const chunks: string[][] = [];
  for (let i = 0; i < nodeIds.length; i += DERIVED_NODE_QUERY_LIMIT) {
    chunks.push(nodeIds.slice(i, i + DERIVED_NODE_QUERY_LIMIT));
  }
  return chunks;
}
