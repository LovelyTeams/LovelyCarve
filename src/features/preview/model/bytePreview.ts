// Text and hex presentation of a byte slice read for an inner preview (ZIP
// members, KDBX attachments).

export const HEX_VIEW_CAP = 32 * 1024;
const TEXT_CHUNK_CHARS = 8000;

export interface HexRow {
  offset: string;
  bytes: string;
  ascii: string;
}

/** Header sniff: a NUL means binary; otherwise enough printable bytes mean text. */
export function looksTextual(bytes: Uint8Array): boolean {
  const length = Math.min(bytes.length, 4096);
  if (length === 0) return true;
  let printable = 0;
  for (let index = 0; index < length; index++) {
    const byte = bytes[index];
    if (byte === 0) return false;
    if (byte === 9 || byte === 10 || byte === 13 || (byte >= 0x20 && byte <= 0x7e) || byte >= 0x80) printable += 1;
  }
  return printable / length > 0.85;
}

/** UTF-8 text in chunks small enough to lay out lazily. */
export function textChunks(bytes: Uint8Array | null): string[] {
  if (!bytes || bytes.length === 0) return [];
  const text = new TextDecoder('utf-8', { fatal: false }).decode(bytes);
  const out: string[] = [];
  for (let index = 0; index < text.length; index += TEXT_CHUNK_CHARS) {
    out.push(text.slice(index, index + TEXT_CHUNK_CHARS));
  }
  return out;
}

/** Rows of 16 bytes, up to `HEX_VIEW_CAP` bytes. */
export function hexRows(bytes: Uint8Array | null): HexRow[] {
  if (!bytes) return [];
  const length = Math.min(bytes.length, HEX_VIEW_CAP);
  const rows: HexRow[] = [];
  for (let offset = 0; offset < length; offset += 16) {
    const end = Math.min(offset + 16, length);
    let hex = '';
    let ascii = '';
    for (let index = offset; index < end; index++) {
      hex += bytes[index].toString(16).padStart(2, '0').toUpperCase() + ' ';
      ascii += bytes[index] >= 0x20 && bytes[index] <= 0x7e ? String.fromCharCode(bytes[index]) : '.';
    }
    // The last row keeps the byte column width so the ASCII column aligns.
    rows.push({ offset: offset.toString(16).padStart(8, '0').toUpperCase(), bytes: hex.padEnd(16 * 3, ' '), ascii });
  }
  return rows;
}
