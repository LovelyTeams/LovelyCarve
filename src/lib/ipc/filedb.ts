// filedb metadata index IPC: the filesystem array (`volId` = index), the
// generic metadata filter path over the lowercase copy columns, and the
// MetaAddr `(imageId, volId, metaAddr, metaAddrHigh)` resolution channel.
import { invokeCmd } from './client';

/** One entry of an image's filesystem array; `volId` is its position. */
export interface FiledbVolume {
  volId: number;
  volumeKey: string;
  /** Node-id family used for numeric addressing: `ntfs` | `ext4` | `btrfs` | `path`. */
  fsPrefix: string;
  kind: string;
  size: number;
}

/** Filter for `filedbQueryFiles`. Text needles are matched case-insensitively
 *  against the precomputed lowercase copy columns (`name_low` / `full_path_low`). */
export interface FiledbFileFilter {
  namePrefix?: string | null;
  nameContains?: string | null;
  /** Case-insensitive whole-name glob: *, ?, [abc], [a-z], [^abc].
   *  Use pathPrefix separately for directory scope; % and _ are literal. */
  nameGlob?: string | null;
  pathPrefix?: string | null;
  pathContains?: string | null;
  /** Extensions without the leading dot; matched case-insensitively. */
  extensions?: string[];
  /** Node kinds: `file` | `dir` | `symlink` | `unallocated`. */
  kinds?: string[];
  minSize?: number | null;
  maxSize?: number | null;
  /** `2` = live entry, `1` = deleted. */
  isDelete?: 1 | 2 | null;
  limit?: number;
  offset?: number;
}

export interface FiledbFileHit {
  nid: string;
  name: string;
  fullPath: string;
  kind: string;
  size: number;
  /** `2` = live entry, `1` = deleted. */
  isDelete: number;
}

/** `QueryFileSt`-style four-tuple address. `fullPath` is required for
 *  path-addressed filesystems (`metaAddr === 0`). */
export interface FiledbFileQuery {
  imageId: string;
  volId: number;
  metaAddr: number;
  metaAddrHigh: number;
  nid?: string | null;
  fullPath?: string | null;
  isDelete?: 1 | 2;
}

export function filedbVolumes(args: { imageId: string }): Promise<FiledbVolume[]> {
  return invokeCmd<FiledbVolume[]>('filedb_volumes', args);
}

export function filedbQueryFiles(args: {
  imageId: string;
  volumeKey?: string | null;
  filter: FiledbFileFilter;
}): Promise<FiledbFileHit[]> {
  return invokeCmd<FiledbFileHit[]>('filedb_query_files', {
    imageId: args.imageId,
    volumeKey: args.volumeKey ?? null,
    filter: args.filter,
  });
}

/** Resolve a MetaAddr four-tuple to the node id accepted by `fsRead*` /
 *  `fsStat`. `null` when the address is not in the index. */
export function filedbResolveMetaAddr(query: FiledbFileQuery): Promise<string | null> {
  return invokeCmd<string | null>('filedb_resolve_meta_addr', { query });
}
