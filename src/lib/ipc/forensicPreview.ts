// Vim swap (.swp/.swo/.swn) 预览 IPC：类型镜像
// src-tauri/src/core/preview/vim_swap.rs 的 Serialize 输出。
import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';

export interface VimSwapMeta {
  version: string;
  page_size: number;
  mtime: number;
  mtime_iso: string | null;
  inode: number;
  pid: number;
  user: string;
  host: string;
  fname: string;
}

export interface VimSwapDigest {
  meta: VimSwapMeta;
  lines: string[];
  bytes_read: number;
  truncated: boolean;
  data_block_count: number;
  pointer_block_count: number;
  unknown_block_count: number;
  encrypted: boolean;
  notes: string[];
}

export function fsVimSwapPreview(args: {
  imageId: ImageId;
  nodeId: NodeId;
}): Promise<VimSwapDigest> {
  return invokeCmd<VimSwapDigest>('fs_vim_swap_preview', args);
}
