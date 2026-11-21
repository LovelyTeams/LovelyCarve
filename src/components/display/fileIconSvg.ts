// 文件节点图标的 SVG：src/assets/file-icons 下的源文件随包内联（?raw），
// 按名第一次用到时转换并缓存（换成 currentColor、补 viewBox、id 加前缀）。
import type { FileIconName } from '../../utils/fileIcons';
import { markSafeFromTrustedSource, type SafeHtmlString } from '../../utils/safeHtml';
import { inlineSvgIcon } from '../../utils/svgIcon';

const SOURCES = import.meta.glob<string>('../../assets/file-icons/**/*.svg', {
  eager: true,
  query: '?raw',
  import: 'default',
});

/** 随包的图标源文件，按文件名（不含扩展名）索引；子目录只用来区分出处。 */
export const BUNDLED_FILE_ICONS: ReadonlyMap<FileIconName, string> = new Map(
  Object.entries(SOURCES).map(([path, source]) => [path.slice(path.lastIndexOf('/') + 1, -'.svg'.length), source]),
);

export interface FileIconSvg {
  viewBox: string;
  fill: string;
  body: SafeHtmlString;
}

const FALLBACK: FileIconName = 'file';
const EMPTY: FileIconSvg = { viewBox: '0 0 16 16', fill: 'none', body: markSafeFromTrustedSource('') };
const converted = new Map<FileIconName, FileIconSvg>();

export function fileIconSvg(name: FileIconName): FileIconSvg {
  const cached = converted.get(name);
  if (cached) return cached;
  const resolved = BUNDLED_FILE_ICONS.has(name) ? name : FALLBACK;
  const source = BUNDLED_FILE_ICONS.get(resolved);
  if (source === undefined) return EMPTY;
  const icon = inlineSvgIcon(source, `lc-file-icon-${resolved}-`);
  // 源文件是随包的静态资源，不含证据数据。
  const svg = { viewBox: icon.viewBox, fill: icon.fill, body: markSafeFromTrustedSource(icon.body) };
  converted.set(name, svg);
  return svg;
}
