import { describe, expect, it } from 'vitest';
import type { FsTypeIdent } from '../../../lib/ipc/fs/derived';
import { usePreviewCategory } from './usePreviewCategory';

const ZIP_MAGIC = new Uint8Array([0x50, 0x4b, 0x03, 0x04, 0x14, 0x00]);
const TEXT = new TextEncoder().encode('plain text line\nsecond line\n');
const BINARY = new Uint8Array([0x00, 0x01, 0x02, 0x03, 0xff, 0x00]);

function ident(overrides: Partial<FsTypeIdent>): FsTypeIdent {
  return {
    label: 'txt',
    mime_type: 'text/plain',
    group: 'text',
    description: '',
    extensions: [],
    is_text: true,
    score: 0.99,
    inferred: true,
    file_len: 0,
    ...overrides,
  };
}

function resolve(options: {
  ext: string;
  bytes?: Uint8Array;
  kind?: 'file' | 'dir';
  typeIdent?: FsTypeIdent | null;
}) {
  const preview = usePreviewCategory({
    nodeKind: () => options.kind ?? 'file',
    nodeSize: () => options.bytes?.length ?? 0,
    ext: () => options.ext,
    bytes: () => options.bytes ?? new Uint8Array(),
    effectiveIdent: () => options.typeIdent ?? null,
    mediaCap: 16 * 1024 * 1024,
  });
  return { category: preview.category.value, extMismatch: preview.extMismatch.value };
}

describe('usePreviewCategory', () => {
  it('keeps office documents (ZIP containers) in the rich viewer by extension', () => {
    expect(resolve({ ext: 'docx', bytes: ZIP_MAGIC }).category).toBe('rich');
    expect(resolve({ ext: 'zip', bytes: ZIP_MAGIC }).category).toBe('zip');
    expect(resolve({ ext: '', bytes: ZIP_MAGIC }).category).toBe('zip');
  });

  it('lets a confident type identification win over a renamed extension', () => {
    const docx = ident({
      label: 'docx',
      mime_type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
      is_text: false,
      extensions: ['docx'],
    });
    expect(resolve({ ext: 'zip', bytes: ZIP_MAGIC, typeIdent: docx })).toEqual({ category: 'rich', extMismatch: true });
  });

  it('ignores low-confidence inferences', () => {
    const guess = ident({ label: 'zip', mime_type: 'application/zip', is_text: false, score: 0.5 });
    expect(resolve({ ext: 'docx', bytes: ZIP_MAGIC, typeIdent: guess }).category).toBe('rich');
  });

  it('keeps text-based rich formats out of the code view', () => {
    expect(resolve({ ext: 'eml', bytes: TEXT, typeIdent: ident({}) }).category).toBe('rich');
  });

  it('falls back to byte sniffing without a known extension', () => {
    expect(resolve({ ext: '', bytes: TEXT }).category).toBe('code');
    expect(resolve({ ext: '', bytes: BINARY }).category).toBe('unsupported');
    expect(resolve({ ext: 'crc', bytes: TEXT }).category).toBe('code');
  });

  it('keeps directories out of file viewers even when content hints exist', () => {
    expect(resolve({ ext: 'sqlite', kind: 'dir', bytes: TEXT, typeIdent: ident({}) }).category).toBe('unsupported');
  });

  it('falls back to ordinary content detection for removed key-value formats', () => {
    for (const ext of ['ldb', 'mmkv']) {
      expect(resolve({ ext, bytes: TEXT }).category).toBe('code');
      expect(resolve({ ext, bytes: BINARY }).category).toBe('unsupported');
      expect(resolve({ ext, typeIdent: ident({ label: 'sqlite', is_text: false }) }).category).toBe('sqlite');
    }
  });

  it('detects Vim swap files (.swp/.swo/.swn) by extension and b0VIM magic header', () => {
    const VIM_SWAP = new Uint8Array([0x62, 0x30, 0x56, 0x49, 0x4d, 0x20, 0x39, 0x2e, 0x31]); // "b0VIM 9.1"
    for (const ext of ['swp', 'swo', 'swn']) {
      expect(resolve({ ext, bytes: BINARY }).category).toBe('vimswap');
    }
    expect(resolve({ ext: 'tmp', bytes: VIM_SWAP, typeIdent: ident({ label: 'txt', is_text: true }) }).category).toBe('vimswap');
    expect(resolve({ ext: '', bytes: VIM_SWAP }).category).toBe('vimswap');
  });
});
