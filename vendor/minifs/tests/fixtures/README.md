# MINIFS synthetic fixture

`synthetic.minifs` is original invented test data, CC0-1.0.
Generate from the repository root with `just generate-minifs-fixtures`.
The independent encoder is `scripts/minifs-fixtures.py` (Python stdlib `struct`
and `lzma` / liblzma). No real firmware bytes are included.

SHA-256: `754d27848b397677a5e0b82772d70766af36c2cbcdf46d993a1a93305dd0fda2`.

Four files in two shared chunks: `/web/js/app.js` (bytes 0–255 repeated 289
times, then `MINIFS test\n`), `/web/js/empty` (zero bytes), `/hello.txt`
(`hello\n`) and `/unicode/测试.txt` (`你好 MINIFS\n`, UTF-8).
