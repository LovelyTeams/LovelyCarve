# Independent synthetic YAFFS2 fixtures

`generate.py` uses only Python stdlib to encode NAND pages, OOB tags and
object headers independently of the Rust reader. All fixture bytes are original
CC0-1.0 data. `fixtures.json` records geometry, lengths and SHA-256.

`base-le.yaffs` and `base-be-ecc.yaffs` contain a UTF-8 directory, a known
6600-byte file, hard/symbolic links, an empty file, a device and a deleted name.
The BE case includes a two-byte spare prefix and independent tag ECC with
LE ECC words. `versions.yaffs` deliberately stores sequence order differently
from physical order; shrink removes old tails before a later extension and
post-header write. `shadow.yaffs` covers rename replacement. The sparse fixture
has a length above 4 GiB and extended Unix time, with only one allocated data page.
`unlink-hardlink.yaffs` follows YAFFS's unlink sequence: promote a link's path
to the original object, delete that link object, and preserve another link's bytes.

These synthetic images are not real NAND dumps and do not provide hardware
body-ECC validation. Public-image checks use the optional independent yaffshiv
oracle described in the format guide, without redistributing those input bytes.

`generate_legacy.py` independently encodes YAFFS1 LE/BE dispersed tags, tag ECC,
page-body ECC and 512/16 pages. `legacy-versions.yaffs` includes serial 3→0,
deleted old pages, same-serial copies, shadow and different header/data-derived
lengths. All bytes are original CC0-1.0; hashes are in `legacy-fixtures.json`.
