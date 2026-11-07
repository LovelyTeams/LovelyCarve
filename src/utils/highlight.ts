// Keyword-highlight helpers shared by text-style previews.
import { escapeHtml } from './safeHtml';

/** Escape the regex metacharacters in `s` so a user keyword is matched
 *  literally. */
export function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export interface HighlightResult {
  /** HTML safe to feed to `v-html`: escaped text with <mark> around matches. */
  html: string;
  /** Number of matches found. */
  count: number;
}

/**
 * HTML-escape `text` and wrap every case-insensitive occurrence of `query`
 * in `<mark>`. Empty or unparseable queries return the escaped text untouched.
 */
export function highlightHtml(text: string, query: string): HighlightResult {
  if (!query || query.length > text.length) return { html: escapeHtml(text), count: 0 };
  // Match the original text so HTML entity names cannot produce false hits.
  try {
    const pattern = new RegExp(escapeRegExp(query), 'gi');
    let html = '';
    let count = 0;
    let end = 0;
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(text))) {
      html += escapeHtml(text.slice(end, match.index));
      html += `<mark>${escapeHtml(match[0])}</mark>`;
      end = match.index + match[0].length;
      count++;
    }
    return { html: html + escapeHtml(text.slice(end)), count };
  } catch {
    return { html: escapeHtml(text), count: 0 };
  }
}

/** Count case-insensitive literal occurrences of `query` in `text`. */
export function countMatches(text: string, query: string): number {
  if (!query || !text) return 0;
  try {
    const re = new RegExp(escapeRegExp(query), 'gi');
    return (text.match(re) || []).length;
  } catch {
    return 0;
  }
}
