import { computed, nextTick, onScopeDispose, ref, watch, type Ref } from 'vue';
import { countMatches, escapeRegExp } from '../../../utils/highlight';
import type { DetailTab, PreviewCategory } from '../model/preview';
import type { SafeHtmlString } from '../../../utils/safeHtml';

type MatchNavigator = { focusMatch: (index: number, scroll?: boolean) => void };

/** Search projection and DOM match focus for one preview content instance. */
export function usePreviewSearch(options: {
  searchQuery: Ref<string>; activeTab: Ref<DetailTab>; category: Ref<PreviewCategory>;
  decodedText: Ref<string>; renderedMarkdown: Ref<SafeHtmlString>;
  page: Ref<number>; pageSize: Ref<number>; currentHexStart: Ref<number>;
  hexMatch: Ref<{ count: number; starts: number[] }>;
}) {
  const { searchQuery, activeTab, category, decodedText, renderedMarkdown, page, pageSize, currentHexStart, hexMatch } = options;
  let disposed = false;
  let focusGeneration = 0;
  onScopeDispose(() => { disposed = true; focusGeneration++; });
  // ---- Keyword navigation state ----
  // `currentMatch` is the 0-based index of the focused match in the *active*
  // view (-1 = none yet). DOM views read it to toggle a `.current` mark; the
  // Code view is driven via codeViewerRef; hex match focus lives in useHexPreview.
  const currentMatch = ref(-1);
  const mdMatchCount = ref(0);
  const csvMatchCount = ref(0);
  const textEl = ref<HTMLPreElement | null>(null);
  const mdEl = ref<HTMLDivElement | null>(null);
  const hexEl = ref<HTMLDivElement | null>(null);
  const codeViewerRef = ref<MatchNavigator | null>(null);
  const csvViewerRef = ref<MatchNavigator | null>(null);
  const sqliteViewerRef = ref<(MatchNavigator & { matchCount: number | null }) | null>(null);

  // Match count shown next to the search box, computed against the active view.
  const matchCount = computed(() => {
    if (!searchQuery.value || activeTab.value === 'detail') return 0;
    if (activeTab.value === 'hex') return hexMatch.value.count;
    if (activeTab.value === 'raw') {
      if (category.value === 'markdown') return mdMatchCount.value;
      if (category.value === 'csv') return csvMatchCount.value;
      if (category.value === 'sqlite') return sqliteViewerRef.value?.matchCount ?? 0;
      if (category.value === 'code' || category.value === 'structured') return countMatches(decodedText.value, searchQuery.value);
      return 0; // media / unsupported — nothing textual to navigate
    }
    return countMatches(decodedText.value, searchQuery.value);
  });

  // ---- Markdown view keyword highlight ------------------------------------
  // renderedMarkdown is already safe HTML; we own this element so we set its
  // innerHTML directly, then walk text nodes wrapping matches in <mark>. Done
  // in the DOM (never via string injection of the query) so it stays safe.
  function highlightTextNodes(root: HTMLElement, query: string): number {
    if (!query) return 0;
    let re: RegExp;
    try {
      re = new RegExp(escapeRegExp(query), 'gi');
    } catch {
      return 0;
    }
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    const targets: Text[] = [];
    let node: Node | null;
    while ((node = walker.nextNode())) {
      const t = node as Text;
      if (t.parentElement?.tagName === 'MARK') continue;
      re.lastIndex = 0;
      if (re.test(t.data)) targets.push(t);
    }
    let count = 0;
    for (const t of targets) {
      const data = t.data;
      const frag = document.createDocumentFragment();
      let last = 0;
      re.lastIndex = 0;
      let m: RegExpExecArray | null;
      while ((m = re.exec(data))) {
        if (m.index > last) frag.appendChild(document.createTextNode(data.slice(last, m.index)));
        const mark = document.createElement('mark');
        mark.textContent = m[0];
        frag.appendChild(mark);
        count += 1;
        last = m.index + m[0].length;
        if (m[0].length === 0) re.lastIndex += 1;
      }
      if (last < data.length) frag.appendChild(document.createTextNode(data.slice(last)));
      t.replaceWith(frag);
    }
    return count;
  }

  // Re-render markdown (clearing any old marks) then re-apply highlight. Runs
  // post-flush so the v-if'd container exists. Depends on decodedText (not
  // renderedMarkdown) so we never run the markdown renderer for binary files.
  watch(
    [decodedText, searchQuery, activeTab, category],
    () => {
      if (activeTab.value !== 'raw' || category.value !== 'markdown') {
        mdMatchCount.value = 0;
        return;
      }
      const generation = focusGeneration;
      nextTick(() => {
        if (disposed || generation !== focusGeneration || activeTab.value !== 'raw' || category.value !== 'markdown') return;
        const el = mdEl.value;
        if (!el) return;
        el.innerHTML = renderedMarkdown.value;
        mdMatchCount.value = highlightTextNodes(el, searchQuery.value);
      });
    },
    { flush: 'post' },
  );

  // ---- Keyword match navigation (prev / next) -----------------------------

  function clearDomCurrent(container: HTMLElement | null) {
    container?.querySelectorAll('mark.current').forEach((el) => el.classList.remove('current'));
  }

  // Text / markdown: marks are 1-per-match in document order, so index i maps
  // straight to the i-th <mark>.
  function focusDomMatch(container: HTMLElement | null, i: number) {
    if (!container) return;
    const marks = container.querySelectorAll('mark');
    marks.forEach((el) => el.classList.remove('current'));
    const target = marks[i] as HTMLElement | undefined;
    if (target) {
      target.classList.add('current');
      target.scrollIntoView({ block: 'center', inline: 'nearest' });
    }
  }

  // Hex: a match may live on another page — switch pages, mark its bytes
  // current (via currentHexStart), then scroll its row into view.
  function focusHexMatch(i: number) {
    const start = hexMatch.value.starts[i];
    if (start === undefined) return;
    currentHexStart.value = start;
    const targetPage = Math.floor(start / pageSize.value) + 1;
    if (page.value !== targetPage) page.value = targetPage;
    const generation = focusGeneration;
    nextTick(() => {
      if (disposed || generation !== focusGeneration) return;
      const rowIdx = Math.floor((start - (targetPage - 1) * pageSize.value) / 16);
      // +1 skips the header row (which also carries .hex-row).
      const row = hexEl.value?.querySelectorAll('.hex-row')[rowIdx + 1] as HTMLElement | undefined;
      row?.scrollIntoView({ block: 'center' });
    });
  }

  function applyFocus(i: number) {
    if (activeTab.value === 'hex') {
      focusHexMatch(i);
    } else if (activeTab.value === 'text') {
      focusDomMatch(textEl.value, i);
    } else if (activeTab.value === 'raw') {
      if (category.value === 'markdown') focusDomMatch(mdEl.value, i);
      else if (category.value === 'code') codeViewerRef.value?.focusMatch(i);
      else if (category.value === 'csv') csvViewerRef.value?.focusMatch(i);
      else if (category.value === 'sqlite') sqliteViewerRef.value?.focusMatch(i);
    }
  }

  function gotoMatch(delta: number) {
    const total = matchCount.value;
    if (total <= 0) return;
    const idx = currentMatch.value < 0
      ? (delta > 0 ? 0 : total - 1)
      : (currentMatch.value + delta + total) % total;
    currentMatch.value = idx;
    applyFocus(idx);
  }

  function onSearchKeydown(e: KeyboardEvent) {
    if (e.isComposing || e.keyCode === 229) return;
    if (e.key === 'Enter' || e.key === 'ArrowDown') {
      e.preventDefault();
      gotoMatch(e.shiftKey ? -1 : 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      gotoMatch(-1);
    } else if (e.key === 'Escape') {
      // 文档级 Escape 链（文件列表）后两级是关面板/清勾选——搜索框里的
      // Esc 只该清搜索词，必须在这里消费掉。
      e.stopPropagation();
      if (searchQuery.value) searchQuery.value = '';
    }
  }

  function restoreMatch(index: number) {
    if (index < 0 || index >= matchCount.value) return;
    currentMatch.value = index;
    // CodeMirror queues scrolling for its next measure; keep the saved viewport.
    if (activeTab.value === 'raw' && category.value === 'code') codeViewerRef.value?.focusMatch(index, false);
    else applyFocus(index);
  }

  // A new query, switching views, or a new file all clear navigation state.
  function resetMatchNav() {
    focusGeneration++;
    currentMatch.value = -1;
    currentHexStart.value = -1;
    clearDomCurrent(textEl.value);
    clearDomCurrent(mdEl.value);
    // 表格的 <mark> 在子组件里，靠 focusMatch(-1) 清当前态。
    csvViewerRef.value?.focusMatch(-1);
    sqliteViewerRef.value?.focusMatch(-1);
  }
  watch(searchQuery, resetMatchNav);
  watch(activeTab, resetMatchNav);

  return { currentMatch, matchCount, mdMatchCount, csvMatchCount, textEl, mdEl, hexEl, codeViewerRef, csvViewerRef, sqliteViewerRef,
    gotoMatch, restoreMatch, onSearchKeydown, resetMatchNav };
}
