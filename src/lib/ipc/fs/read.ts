import { invokeCmd } from '../client';
import type { ImageId, NodeId } from '../ids';



/** Read up to `maxBytes` of a file (backend hard-caps at 2 MiB) as raw bytes.
 *  Returns an `ArrayBuffer` (Tauri raw IPC response) — wrap with `new Uint8Array(buf)`.
 *  Feeds the detail panel's hex view, and its text / code / Markdown views after
 *  decoding with the label from `fsPreviewTextEncoding`. */
export function fsPreview(args: { imageId: ImageId; nodeId: NodeId; maxBytes: number }): Promise<ArrayBuffer> {
  return invokeCmd<ArrayBuffer>('fs_preview', args);
}

/** Read a whole file as raw bytes. The backend rejects files over 16 MiB even
 *  if a caller skips its frontend size guard. */
export function fsRead(args: { imageId: ImageId; nodeId: NodeId }): Promise<ArrayBuffer> {
  return invokeCmd<ArrayBuffer>('fs_read', args);
}

/** Whole-file read for the Flyfish File Viewer rich preview path, with a much
 *  higher cap than `fsRead` (backend `fs_read_large`, 16 MiB). Larger files
 *  take the `fsMediaTemp` asset-protocol path instead. */
export function fsReadLarge(args: { imageId: ImageId; nodeId: NodeId; taskId: string }): Promise<ArrayBuffer> {
  return invokeCmd<ArrayBuffer>('fs_read_large', args);
}

/** Read one logical file range as raw bytes. The backend hard-caps each call at
 *  16 MiB and rejects offset + length overflow. */
export function fsReadRange(args: {
  imageId: ImageId;
  nodeId: NodeId;
  offset: number;
  length: number;
}): Promise<ArrayBuffer> {
  return invokeCmd<ArrayBuffer>('fs_read_range', args);
}

/** 后端 chardetng 检测 + encoding_rs 解码的文本预览结果（core::text_decode::DecodedText）。
 *  文本/代码/Markdown/结构化「原类型视图」直接渲染 `text`，避免 GBK/GB18030/Big5/
 *  Shift-JIS 等非 UTF-8 文本被当 UTF-8 解出乱码。 */
export interface DecodedText {
  /** 用检测出的编码解码为 UTF-8 的文本（前端直接渲染）。 */
  text: string;
  /** 实际使用的编码 WHATWG 规范名，如 "UTF-8" / "GBK" / "Big5" / "Shift_JIS"。 */
  encoding: string;
  /** 编码由文件头 BOM 声明（权威）而非统计检测——UI 据此把提示表述为「BOM 声明」。 */
  from_bom: boolean;
  /** 解码时出现过该编码无法映射的字节（已用 U+FFFD 替换）——检测可能误判 / 本是二进制的信号。 */
  had_errors: boolean;
  /** 本段在文件中的起始字节偏移。 */
  offset?: number;
  /** 下一段的起始偏移（落在字符边界上，末尾被截断的字符留给下一段）；已读到文件末尾时为 null。 */
  next_offset?: number | null;
}

/** 读取并按自动检测的编码解码文件文本（后端 chardetng + encoding_rs，读取上限由 maxBytes
 *  传入、后端再以 2 MiB 封顶）。十六进制视图仍用 `fsPreview` 的原始字节；文本类视图用本结果
 *  （二者传同一个 PREVIEW_CAP，覆盖同一段前缀）。`encoding`（WHATWG 标签）跳过检测；
 *  `offset` 从文件中间读一段，未指定编码时按文件开头检测。 */
export function fsPreviewText(args: {
  imageId: ImageId;
  nodeId: NodeId;
  maxBytes: number;
  offset?: number;
  encoding?: string;
}): Promise<DecodedText> {
  return invokeCmd<DecodedText>('fs_preview_text', args);
}

/** `fsPreviewText` 第一段的编码结论，不含文本（core::text_decode::TextEncodingGuess）。 */
export interface TextEncodingGuess {
  /** WHATWG 规范名，可直接作为 `TextDecoder` 的 label。 */
  encoding: string;
  from_bom: boolean;
  had_errors: boolean;
  /** 这段字节之后文件还有内容：末尾被截断的字符不进文本（`TextDecoder` 用 `stream: true`）。 */
  continues: boolean;
}

/** 检测 `fsPreview` 同一段前缀的文本编码（与 `fsPreviewText` 第一段同一套检测），不回传文本。
 *  encoding_rs 与 `TextDecoder` 实现同一份 WHATWG 编码标准，前端用 `encoding` 解码自己持有的
 *  字节即可得到相同文本。两个命令须传同一个 `maxBytes`（PREVIEW_CAP）。 */
export function fsPreviewTextEncoding(args: { imageId: ImageId; nodeId: NodeId; maxBytes: number }): Promise<TextEncodingGuess> {
  return invokeCmd<TextEncodingGuess>('fs_preview_text_encoding', args);
}
