export type PreviewCategory =
  | 'markdown'
  | 'image'
  | 'audio'
  | 'video'
  | 'structured'
  | 'csv'
  | 'code'
  | 'binary'
  | 'gguf'
  | 'sqlite'
  | 'realm'
  | 'rich'
  | 'zip'
  | 'unsupported';

/** Shared content-preview limits and view state; window docking has a separate owner. */
export type DetailTab = 'detail' | 'raw' | 'text' | 'hex';
export const PREVIEW_CAP = 1024 * 1024;
export const MEDIA_CAP = 16 * 1024 * 1024;
export const PREVIEW_TABS = [
  { key: 'detail', label: '详细信息' }, { key: 'raw', label: '原类型' },
  { key: 'text', label: '文本' }, { key: 'hex', label: '十六进制' },
] as const;
export const PREVIEW_PAGE_SIZES = [1024, 2048, 4096, 8192].map(bytes => ({ value: bytes, label: `${bytes / 1024} KiB` }));
export const STREAM_MIME_EXT: Readonly<Record<string, string>> = {
  'video/mp4': 'mp4', 'video/webm': 'webm', 'video/quicktime': 'mov', 'video/x-matroska': 'mkv',
  'audio/mpeg': 'mp3', 'audio/wav': 'wav', 'audio/ogg': 'ogg', 'audio/flac': 'flac',
  'audio/mp4': 'm4a', 'audio/aac': 'aac',
};

/** Preserve the existing detail-panel precision and binary unit thresholds. */
export function formatPreviewBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = size / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index++; }
  return `${value.toFixed(2)} ${units[index]}`;
}
