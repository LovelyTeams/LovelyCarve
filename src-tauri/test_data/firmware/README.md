# Synthetic firmware fixtures

Original generated data, CC0-1.0. Run `generate.py` with Python 3 after generating
the vendored backhand DD-WRT fixture. Only synthetic content and public format
constants are used; there is no real firmware or user data.

The independent Python generator wraps the small v3 filesystem in a TRX v1
container with CRC, a Buffalo plaintext file-length header and default-key ENC.
The bgn variant adds an encrypted 152-byte synthetic product tag. This verifies
both outer layouts through LovelyCarve's normal image and filesystem dispatch.

`generate_wrappers.py` independently encodes `synthetic-uimage.bin`,
`synthetic-safeloader.bin` and `synthetic.chk` using Python's CRC32 / MD5 and a
weighted-sum CHK checksum oracle. These contain the original CramFS fixture
from `../rom-images/`, and exercise header verification, separate kernel/rootfs
segments, file listing, cross-page reads and hashes through the standalone CLI.

| File | SHA-256 |
| --- | --- |
| synthetic-bgn.enc | `709307922f9ccb9b85904a4f11fc37545fb0deff632d96e79d7acd5f0c1ab15b` |
| synthetic.enc | `d789405856318177d1a18490ca53253913cf451387608919720ab1755af563fb` |
| synthetic-uimage.bin | `95d3d6790a6f0a129ddf9fa12c2c0f24b1e3889e9b6ed798afd50efa22caaf06` |
| synthetic-safeloader.bin | `553ecd8791a52a8702c536bf3b39e94d3a34ffe1abae597cbe0b69a07b54a31a` |
| synthetic.chk | `60df39928da308455cd18f6b2e76e3f59a1f27f938a5a9d1c0e355141fc88274` |
