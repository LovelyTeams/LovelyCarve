import { invokeCmd } from '../client';
import { type Page } from './types';
import type { ImageId, NodeId } from '../ids';


export interface MactimeSummary {
  lines_written: number;
  bytes_written: number;
  cancelled: boolean;
}

/**
 * Export a TSK-style mactime body file. Pass an empty `nodeId` to export the
 * whole image, or a directory node id to scope the timeline to that subtree.
 */
export function fsTimelineExport(args: {
  imageId: ImageId;
  nodeId: NodeId;
  destination: string;
}): Promise<MactimeSummary> {
  return invokeCmd<MactimeSummary>('fs_timeline_export', args);
}

export interface UsnRecord {
  usn: number;
  file_record: number;
  parent_record: number;
  timestamp: string | null;
  reasons: string[];
  name: string;
  file_attributes: number;
}

/**
 * Read the NTFS `$UsnJrnl` change journal for a volume. Pass a `part:N` /
 * `lvm:N:M` volume key or any `ntfs:` node id on that volume.
 */
export function fsUsnJournal(args: {
  imageId: ImageId;
  nodeId: NodeId;
}): Promise<UsnRecord[]> {
  return invokeCmd<UsnRecord[]>('fs_usn_journal', args);
}

/** Cursor-paged NTFS `$UsnJrnl` listing. Prefer this for large journals; the
 *  legacy `fsUsnJournal` still returns the full decoded window for existing
 *  callers. */
export function fsUsnJournalPage(args: {
  imageId: ImageId;
  nodeId: NodeId;
  cursor?: string | null;
  pageSize?: number | null;
}): Promise<Page<UsnRecord>> {
  return invokeCmd<Page<UsnRecord>>('fs_usn_journal_page', args);
}

export interface JournalBlockRef {
  sequence: number;
  fs_block: number;
  journal_offset: number;
}

export interface Ext4Journal {
  block_size: number;
  transactions: number;
  blocks: JournalBlockRef[];
  revoked: number[];
}

export interface Ext4JournalBlocksPage {
  blockSize: number;
  transactions: number;
  items: JournalBlockRef[];
  revokedTotal: number;
  nextCursor: string | null;
  total?: number;
  truncated: boolean;
}

/**
 * Mine the ext4 jbd2 journal for a volume. Pass an `ext4:` node id or a
 * `part:N` / `lvm:N:M` volume key. Returns historical block revisions.
 */
export function fsExt4Journal(args: {
  imageId: ImageId;
  nodeId: NodeId;
}): Promise<Ext4Journal> {
  return invokeCmd<Ext4Journal>('fs_ext4_journal', args);
}

/** Cursor-paged ext4 jbd2 historical block revisions. Returns metadata plus
 *  one page of `JournalBlockRef` rows; revoked blocks are exposed as a count
 *  here to keep the paged response bounded. */
export function fsExt4JournalBlocksPage(args: {
  imageId: ImageId;
  nodeId: NodeId;
  cursor?: string | null;
  pageSize?: number | null;
}): Promise<Ext4JournalBlocksPage> {
  return invokeCmd<Ext4JournalBlocksPage>('fs_ext4_journal_blocks_page', args);
}
