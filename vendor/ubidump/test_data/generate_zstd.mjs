// CC0-1.0 independent Zstandard encoder fixture, Node stdlib (libzstd).
// First run generate_ubifs.py to create payload.bin, then this file, then
// generate_ubifs.py again. This encoder is independent of the Rust decoder.
import { readFileSync, writeFileSync } from 'node:fs';
import { zstdCompressSync, constants } from 'node:zlib';
const payload = readFileSync(new URL('payload.bin', import.meta.url));
for (const block of [0, 2]) {
  const input = payload.subarray(block * 4096, (block + 1) * 4096);
  const encoded = zstdCompressSync(input, { params: {
    [constants.ZSTD_c_compressionLevel]: 3,
    [constants.ZSTD_c_checksumFlag]: 1,
  }});
  writeFileSync(new URL(`zstd-${block}.frame`, import.meta.url), encoded);
}
