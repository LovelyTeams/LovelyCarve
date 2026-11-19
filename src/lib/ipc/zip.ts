// ZIP 压缩包预览 IPC：镜像内 .zip 文件「原类型视图」的内部结构（中央目录）与
// 单个成员内容。类型镜像 src-tauri/src/core/zip_preview.rs 的 Serialize 输出
// （snake_case）。结构解析与成员解压都在 Rust 后端，前端只负责构树与渲染。
import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';

/** 压缩包内一个成员（文件 / 目录 / 符号链接 / 不可读）。 */
export interface ZipPreviewEntry {
  /** 规范化的包内路径，`/` 分隔，无首尾斜杠。 */
  path: string;
  /** 末段路径（展示名）。 */
  name: string;
  /** `"file"` / `"dir"` / `"symlink"` / `"other"`（other = 加密或未支持的压缩方法）。 */
  kind: 'file' | 'dir' | 'symlink' | 'other';
  /** 解压后（逻辑）大小；目录为 0。 */
  size: number;
  /** 压缩后（存储）大小；目录为 0。 */
  compressed_size: number;
  /** 压缩方法标签，如 "Stored" / "Deflate" / "Bzip2"。 */
  method: string;
  /** 成员是否加密（不可解压预览）。 */
  encrypted: boolean;
  /** 内容 CRC-32（8 位小写十六进制）；目录为 null。 */
  crc32: string | null;
  /** 修改时间（ISO-8601 UTC）；缺失 / MS-DOS 时间越界时为 null。 */
  modified: string | null;
  /** 符号链接成员的目标路径；其余为 null。 */
  link_target: string | null;
}

/** 整包预览：全部成员 + 汇总统计。 */
export interface ZipPreview {
  entries: ZipPreviewEntry[];
  /** 扫描到的真实成员总数（可能 > entries.length，见 entries_truncated）。 */
  entry_count: number;
  /** 可读的普通文件成员数。 */
  file_count: number;
  /** 显式目录成员数（树会自行合成缺失的祖先目录）。 */
  dir_count: number;
  /** 全部非目录成员的解压大小之和。 */
  total_uncompressed: number;
  /** 全部非目录成员的压缩大小之和。 */
  total_compressed: number;
  /** 标记为加密的成员数。 */
  encrypted_count: number;
  /** 压缩包级注释（已 lossy 解码）；空则为 null。 */
  comment: string | null;
  /** 明细列表是否因上限被截断（统计仍按全部成员）。 */
  entries_truncated: boolean;
  /** 仅读到文件前缀（文件超出读取上限）；中央目录在尾部，能解析说明其恰好落在已读范围内。 */
  truncated_bytes: boolean;
  warnings: string[];
}

/** 解析镜像内 ZIP 文件的内部结构。非 ZIP / 损坏 / 过大尾部不可达时 reject，
 *  错误信息可直接展示。 */
export function fsZipPreview(args: { imageId: ImageId; nodeId: NodeId }): Promise<ZipPreview> {
  return invokeCmd<ZipPreview>('fs_zip_preview', args);
}

/** 解压并读取压缩包内某一个成员的内容（后端硬上限 1 MiB），返回 `ArrayBuffer`
 *  （Tauri 原始 IPC 响应，用 `new Uint8Array(buf)` 包装）。目录 / 符号链接 / 加密 /
 *  未支持压缩方法的成员 reject。 */
export function fsZipMemberRead(args: {
  imageId: ImageId;
  nodeId: NodeId;
  memberPath: string;
  maxBytes: number;
}): Promise<ArrayBuffer> {
  return invokeCmd<ArrayBuffer>('fs_zip_member_read', args);
}
