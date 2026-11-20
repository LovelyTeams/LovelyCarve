// Vim 交换文件（.swp / .swo / …）解析 IPC：原类型视图的未保存缓冲区与块 0 元数据。
// 类型镜像 src-tauri/src/core/preview/vim_swap.rs 的 Serialize 输出（snake_case）。
import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';

/** 块 0 记录的编辑会话。 */
export interface VimSwapMeta {
  /** 写入交换文件的 Vim 版本，如 `VIM 9.0`。 */
  version: string;
  page_size: number;
  /** 原文件的修改时间（Unix 秒，0 为未记录）。 */
  mtime: number;
  /** `mtime` 的 RFC 3339（UTC）形式，未记录时为 null。 */
  mtime_iso: string | null;
  inode: number;
  pid: number;
  user: string;
  host: string;
  /** 正在编辑的原文件路径。 */
  fname: string;
}

export interface VimSwapDigest {
  meta: VimSwapMeta;
  /** 数据块中取出的行，按块内存放顺序。 */
  lines: string[];
  bytes_read: number;
  /** 文件超过读取上限，只解析了开头。 */
  truncated: boolean;
  data_block_count: number;
  pointer_block_count: number;
  unknown_block_count: number;
  /** 块 0 可读但没有数据块，交换文件可能已加密。 */
  encrypted: boolean;
  notes: string[];
}

export function fsVimSwapPreview(args: { imageId: ImageId; nodeId: NodeId }): Promise<VimSwapDigest> {
  return invokeCmd<VimSwapDigest>('fs_vim_swap_preview', args);
}
