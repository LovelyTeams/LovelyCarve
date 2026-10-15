import { sourceText, type Translator } from '../../../i18n/translate';
// 全局搜索面板的纯展示逻辑。wire 层
// 命中为 `lib/ipc/search.ts` 的 `GlobalSearchHit`（snake_case 原样）；本模块
// 只做格式化与高亮，不做状态。
import type { GlobalSearchHit } from '../../../lib/ipc/search';

export interface HighlightSegment {
  text: string;
  matched: boolean;
}

/** 关键词/正则多行输入 → 去重模式列表（换行、逗号分隔；保留原样，trim 后
 *  为空的行丢弃——后端同样会再做一次）。 */
export function parseSearchPatterns(input: string): string[] {
  return [...new Set(
    input
      .split(/[\r\n,，]+/)
      .map(pattern => pattern.trim())
      .filter(Boolean),
  )];
}

/** 偏移列：十六进制 + "h"。未分配/磁盘间隙命中同样是源内
 *  偏移，格式一致。 */
export function formatHitOffset(hit: GlobalSearchHit): string {
  return `${Math.max(0, hit.file_offset).toString(16).toUpperCase()}h`;
}

export function formatSearchFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

/** 来源列：文件 / 未分配（$未分配簇）/ 磁盘间隙。 */
export function sourceKindLabel(sourceKind: string, tr: Translator = sourceText): string {
  if (sourceKind === 'unallocated') return tr("未分配");
  if (sourceKind === 'diskgap') return tr("磁盘间隙");
  return tr("文件");
}

/** 任务树节点标题：关键词在前、正则在后；空侧省略。 */
export function taskDisplayTitle(keywords: string[], regexes: string[], tr: Translator = sourceText): string {
  const parts: string[] = [];
  if (keywords.length > 0) parts.push(keywords.join('、'));
  if (regexes.length > 0) parts.push(`/${regexes[0]}${regexes.length > 1 ? tr(" 等{0}条", { 0: regexes.length }) : ''}`);
  return parts.join(' + ') || tr("（空）");
}

/** 以首个模式为中心截取上下文预览（长内容收进悬浮完整上下文）。 */
export function buildSearchContextPreview(
  text: string,
  pattern: string,
  maxCharacters = 96,
): string {
  const limit = Math.max(16, Math.floor(maxCharacters));
  if (text.length <= limit) return text;

  const matchIndex = pattern
    ? text.toLocaleLowerCase().indexOf(pattern.toLocaleLowerCase())
    : -1;
  if (matchIndex < 0) return `${text.slice(0, limit - 1)}…`;

  const matchLength = Math.max(1, pattern.length);
  const contentBudget = Math.max(matchLength, limit - 2);
  let start = Math.max(0, matchIndex - Math.floor((contentBudget - matchLength) / 2));
  const end = Math.min(text.length, start + contentBudget);
  start = Math.max(0, end - contentBudget);

  return `${start > 0 ? '…' : ''}${text.slice(start, end)}${end < text.length ? '…' : ''}`;
}

/** 把上下文按模式切分为高亮片段，渲染层用 <mark>。大小写不敏感。 */
export function highlightSearchPattern(text: string, pattern: string): HighlightSegment[] {
  if (!pattern) return [{ text, matched: false }];

  const segments: HighlightSegment[] = [];
  const normalizedText = text.toLocaleLowerCase();
  const normalizedPattern = pattern.toLocaleLowerCase();
  let cursor = 0;

  while (cursor < text.length) {
    const matchIndex = normalizedText.indexOf(normalizedPattern, cursor);
    if (matchIndex < 0) {
      segments.push({ text: text.slice(cursor), matched: false });
      break;
    }
    if (matchIndex > cursor) {
      segments.push({ text: text.slice(cursor, matchIndex), matched: false });
    }
    const matchEnd = matchIndex + pattern.length;
    segments.push({ text: text.slice(matchIndex, matchEnd), matched: true });
    cursor = matchEnd;
  }

  return segments.length > 0 ? segments : [{ text, matched: false }];
}
