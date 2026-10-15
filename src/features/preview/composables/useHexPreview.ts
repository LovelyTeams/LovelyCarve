
import { tr } from '../../../composables/useLocale';
import { CHARSETS, type CharsetOption } from '../model/charsets';
import { computed, onScopeDispose, ref, watch, type Ref } from 'vue';
import { notifyError, notifyInfo, notifySuccess } from '../../../composables/useToast';

interface UseHexPreviewOptions {
  bytes: Ref<Uint8Array>;
  page: Ref<number>;
  pageSize: Ref<number>;
  searchQuery: Ref<string>;
}

interface DecodedChar {
  ch: string;
  off: number;
}

const DEFAULT_CHARSETS = ['ascii', 'utf-8', 'utf-16le', 'utf-16be'];
const CHARSET_KEY = 'lc.hexCharsets';
const HEX_HEADER_BYTES = Array.from({ length: 16 }, (_, index) => index.toString(16).toUpperCase() + '  ').join('');

const decoderCache = new Map<string, TextDecoder>();

function loadSelectedCharsets(): string[] {
  try {
    const raw = localStorage.getItem(CHARSET_KEY);
    if (raw) {
      const parsed: unknown = JSON.parse(raw);
      if (Array.isArray(parsed)) {
        const valid = parsed.filter((id): id is string => typeof id === 'string' && CHARSETS.some(charset => charset.id === id));
        if (valid.length || parsed.length === 0) return valid;
      }
    }
  } catch {
    // localStorage unavailable: fall back to defaults.
  }
  return [...DEFAULT_CHARSETS];
}

function getDecoder(label: string): TextDecoder | null {
  let decoder = decoderCache.get(label);
  if (!decoder) {
    try {
      decoder = new TextDecoder(label, { fatal: false, ignoreBOM: true });
    } catch {
      return null;
    }
    decoderCache.set(label, decoder);
  }
  return decoder;
}

function sanitizeChar(char: string): string {
  const codePoint = char.codePointAt(0) ?? 0;
  if (codePoint < 0x20 || (codePoint >= 0x7f && codePoint <= 0x9f) || codePoint === 0xfffd || codePoint === 0xfeff) return '.';
  return char;
}

function singleByteChars(chunk: Uint8Array, charset: CharsetOption, rowBase: number): DecodedChar[] {
  const out: DecodedChar[] = [];
  if (charset.decoder === 'ascii') {
    for (let index = 0; index < chunk.length; index += 1) {
      const byte = chunk[index];
      out.push({ ch: byte >= 0x20 && byte <= 0x7e ? String.fromCharCode(byte) : '.', off: rowBase + index });
    }
    return out;
  }
  const decoder = getDecoder(charset.decoder);
  if (!decoder) {
    for (let index = 0; index < chunk.length; index += 1) out.push({ ch: '.', off: rowBase + index });
    return out;
  }
  const text = decoder.decode(chunk);
  for (let index = 0; index < chunk.length; index += 1) {
    out.push({ ch: sanitizeChar(text[index] ?? '.'), off: rowBase + index });
  }
  return out;
}

function multiByteChars(chunk: Uint8Array, charset: CharsetOption, rowBase: number): DecodedChar[] {
  const out: DecodedChar[] = [];
  const decoder = getDecoder(charset.decoder);
  if (!decoder) return out;
  let segmentStart = 0;
  for (let byteIndex = 0; byteIndex < chunk.length; byteIndex += 1) {
    const piece = decoder.decode(chunk.subarray(byteIndex, byteIndex + 1), { stream: true });
    if (piece.length > 0) {
      for (const char of piece) out.push({ ch: sanitizeChar(char), off: rowBase + segmentStart });
      segmentStart = byteIndex + 1;
    }
  }
  const tail = decoder.decode();
  for (const char of tail) out.push({ ch: sanitizeChar(char), off: rowBase + segmentStart });
  return out;
}

function offFromEvent(event: MouseEvent): number | null {
  const element = (event.target as HTMLElement | null)?.closest<HTMLElement>('[data-off]');
  if (!element) return null;
  const value = Number(element.dataset.off);
  return Number.isFinite(value) ? value : null;
}

function isReadableCp(codePoint: number): boolean {
  return !(codePoint < 0x20 || (codePoint >= 0x7f && codePoint <= 0x9f) || codePoint === 0xfffd || codePoint === 0xfeff);
}

export function useHexPreview(options: UseHexPreviewOptions) {
  const currentHexStart = ref(-1);
  const selectedCharsets = ref<string[]>(loadSelectedCharsets());
  const charsetMenuOpen = ref(false);
  const selAnchor = ref(-1);
  const selFocus = ref(-1);
  const selecting = ref(false);

  watch(
    selectedCharsets,
    (value) => {
      try {
        localStorage.setItem(CHARSET_KEY, JSON.stringify(value));
      } catch {
        // Storage is best-effort.
      }
    },
    { deep: true },
  );

  const needleByteLen = computed(() =>
    options.searchQuery.value ? new TextEncoder().encode(options.searchQuery.value).length : 0,
  );

  const hexMatch = computed(() => {
    const set = new Set<number>();
    const starts: number[] = [];
    const query = options.searchQuery.value;
    if (!query) return { set, starts, count: 0 };
    const needle = new TextEncoder().encode(query);
    const haystack = options.bytes.value;
    if (needle.length > 0) {
      for (let index = 0; index + needle.length <= haystack.length; index += 1) {
        let hit = true;
        for (let offset = 0; offset < needle.length; offset += 1) {
          if (haystack[index + offset] !== needle[offset]) {
            hit = false;
            break;
          }
        }
        if (hit) {
          for (let offset = 0; offset < needle.length; offset += 1) set.add(index + offset);
          starts.push(index);
          index += needle.length - 1;
        }
      }
    }
    return { set, starts, count: starts.length };
  });

  const selectedCharsetList = computed(() => CHARSETS.filter(charset => selectedCharsets.value.includes(charset.id)));

  function toggleCharset(id: string) {
    selectedCharsets.value = selectedCharsets.value.includes(id)
      ? selectedCharsets.value.filter(candidate => candidate !== id)
      : [...selectedCharsets.value, id];
  }

  const hexGridCols = computed(() => {
    const count = selectedCharsetList.value.length;
    const base = '56px minmax(360px, max-content)';
    return count > 0 ? `${base} repeat(${count}, minmax(19ch, 1fr))` : base;
  });

  const hexRows = computed(() => {
    const base = (options.page.value - 1) * options.pageSize.value;
    const slice = options.bytes.value.subarray(base, options.page.value * options.pageSize.value);
    const matched = hexMatch.value.set;
    const currentStart = currentHexStart.value;
    const currentEnd = currentStart >= 0 ? currentStart + needleByteLen.value : -1;
    const charsets = selectedCharsetList.value;
    const rows: {
      offset: string;
      bytes: { off: number; hex: string; m: boolean; cur: boolean }[];
      padStr: string;
      cols: { off: number; ch: string; m: boolean; cur: boolean }[][];
    }[] = [];

    for (let index = 0; index < slice.length; index += 16) {
      const chunk = slice.subarray(index, index + 16);
      const rowBase = base + index;
      const offset = rowBase.toString(16).toUpperCase().padStart(4, '0') + 'h';
      const byteCells = [];
      for (let byteIndex = 0; byteIndex < chunk.length; byteIndex += 1) {
        const globalIndex = rowBase + byteIndex;
        byteCells.push({
          off: globalIndex,
          hex: chunk[byteIndex].toString(16).toUpperCase().padStart(2, '0') + ' ',
          m: matched.has(globalIndex),
          cur: globalIndex >= currentStart && globalIndex < currentEnd,
        });
      }
      const padStr = ' '.repeat(Math.max(0, (16 - chunk.length) * 3));
      const cols = charsets.map((charset) => {
        const chars = charset.singleByte ? singleByteChars(chunk, charset, rowBase) : multiByteChars(chunk, charset, rowBase);
        return chars.map(char => ({
          off: char.off,
          ch: char.ch,
          m: matched.has(char.off),
          cur: char.off >= currentStart && char.off < currentEnd,
        }));
      });
      rows.push({ offset, bytes: byteCells, padStr, cols });
    }
    return rows;
  });

  const selMin = computed(() => (selAnchor.value < 0 ? -1 : Math.min(selAnchor.value, selFocus.value)));
  const selMax = computed(() => (selAnchor.value < 0 ? -1 : Math.max(selAnchor.value, selFocus.value)));
  const hasSelection = computed(() => selMin.value >= 0);
  const selCount = computed(() => (hasSelection.value ? selMax.value - selMin.value + 1 : 0));

  function inSel(off: number): boolean {
    return selMin.value >= 0 && off >= selMin.value && off <= selMax.value;
  }

  function onHexMouseDown(event: MouseEvent) {
    if (event.button !== 0) return;
    const offset = offFromEvent(event);
    if (offset === null) return;
    event.preventDefault();
    selAnchor.value = offset;
    selFocus.value = offset;
    selecting.value = true;
    window.addEventListener('mousemove', onHexDragMove);
    window.addEventListener('mouseup', onHexDragEnd);
  }

  function onHexDragMove(event: MouseEvent) {
    if (!selecting.value) return;
    const offset = offFromEvent(event);
    if (offset !== null && offset !== selFocus.value) selFocus.value = offset;
  }

  function onHexDragEnd() {
    selecting.value = false;
    window.removeEventListener('mousemove', onHexDragMove);
    window.removeEventListener('mouseup', onHexDragEnd);
  }
  onScopeDispose(onHexDragEnd);

  function clearHexSelection() {
    selAnchor.value = -1;
    selFocus.value = -1;
    onHexDragEnd();
  }

  function selectionSlice(): Uint8Array {
    return options.bytes.value.subarray(selMin.value, selMax.value + 1);
  }

  async function writeClipboard(text: string, okMsg: string) {
    try {
      await navigator.clipboard.writeText(text);
      notifySuccess(okMsg);
    } catch (error) {
      notifyError(tr("复制失败"), String(error));
    }
  }

  function copySelectionHex() {
    const slice = selectionSlice();
    const hex = Array.from(slice, byte => byte.toString(16).toUpperCase().padStart(2, '0')).join(' ');
    void writeClipboard(hex, tr("已复制 {0} 字节十六进制", { 0: slice.length }));
  }

  function copySelectionText(charset: CharsetOption) {
    const slice = selectionSlice();
    let text = '';
    if (charset.decoder === 'ascii') {
      for (const byte of slice) if (byte >= 0x20 && byte <= 0x7e) text += String.fromCharCode(byte);
    } else {
      let raw = '';
      try {
        raw = new TextDecoder(charset.decoder, { fatal: false, ignoreBOM: true }).decode(slice);
      } catch {
        raw = '';
      }
      for (const char of raw) if (isReadableCp(char.codePointAt(0) ?? 0)) text += char;
    }
    void writeClipboard(text, tr("已复制文本（{0}）· {1} 字", { 0: charset.label, 1: text.length }));
  }

  function onHexContextMenu(event: MouseEvent) {
    event.preventDefault();
    if (!hasSelection.value) {
      notifyInfo(tr("请先拖选要复制的字节"));
      return;
    }
    const target = event.target as HTMLElement | null;
    if (target?.closest('.hex-bytes')) {
      copySelectionHex();
      return;
    }
    const colElement = target?.closest<HTMLElement>('.hex-text-col[data-cs]');
    const id = colElement?.dataset.cs;
    if (id) {
      const charset = CHARSETS.find(candidate => candidate.id === id);
      if (charset) copySelectionText(charset);
    }
  }

  return {
    currentHexStart,
    hexHeaderBytes: HEX_HEADER_BYTES,
    hexMatch,
    selectedCharsets,
    charsetMenuOpen,
    selectedCharsetList,
    toggleCharset,
    hexGridCols,
    hexRows,
    selMin,
    selMax,
    hasSelection,
    selCount,
    inSel,
    onHexMouseDown,
    onHexDragEnd,
    clearHexSelection,
    onHexContextMenu,
  };
}
