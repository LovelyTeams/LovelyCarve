# Origin

`kdbx-reader` 0.1.0 is a Rust rewrite of the reading half of pykeepass 4.2.0,
<https://github.com/libkeepass/pykeepass>, commit
`aec256659e7b2b497a37891476241b720f60c84e` (2026-08-09), by Philipp Schmitt,
Evan Widloski and contributors, licensed GPL-3.0. The rewrite keeps that
license (GPL-3.0-only); the full text is in `LICENSE`.

## Taken over

- `kdbx_parsing/kdbx.py`, `kdbx3.py`, `kdbx4.py`, `common.py`: the outer
  header of both layouts, the KDBX 4 VariantDictionary, the composite key and
  keyfile forms, AES-KDF and Argon2, the master and HMAC keys, the hashed
  (KDBX 3) and HMAC (KDBX 4) block streams, payload decryption and GZip, the
  KDBX 4 inner header, and the Salsa20 / ChaCha20 inner streams with their key
  derivation.
- `pykeepass.py`, `entry.py`, `group.py`, `attachment.py`, `baseelement.py`:
  the database model and its read-only queries (`version`,
  `encryption_algorithm`, `kdf_algorithm`, `transformed_key`,
  `database_salt`, `root_group`, `recyclebin_group`, `groups`, `entries`,
  `find_entries`, `find_groups`, `binaries`, `attachments`, `deref`, `xml`,
  entry fields, tags, OTP, history, auto-type, times, paths).

## Not taken over

Everything that writes: `save`, `create_database`, seed rotation, adding,
moving, trashing and deleting groups, entries, attachments and binaries,
field setters, `touch`, `reindex`. The construct structures and lxml XPath
queries are replaced by direct parsing and an element tree; queries take
closures instead of XPath keyword arguments.

## Changes

- Integrity: the KDBX 3.1 `Meta/HeaderHash` is compared with the header
  (`HeaderHash::Mismatch`, opened anyway); KDBX 4 checks the header SHA-256
  before the key and every block HMAC including the final one; block indices
  and the final zero hash of KDBX 3 are enforced.
- Protected values: every `Protected="True"` element consumes the inner stream
  in document order, so protected `Meta/Binaries` entries (KDBX 3) no longer
  misalign the values after them. Values that are not base64 are kept as
  stored with a reason instead of being logged and skipped silently.
- Keyfiles follow KeePass: an XML keyfile with an unknown version or a bad
  `Hash` is an error, a 2.0 keyfile without `Hash` is accepted, 64-character
  keyfiles must be strictly hexadecimal.
- Argon2 takes the secret key (`K`) and associated data (`A`) parameters;
  AES-KDF seeds that are not 32 bytes are hashed as KeePass does. Work,
  memory, payload and XML size limits (`Limits`) are checked before
  computing; Argon2 memory is allocated fallibly.
- KDBX 2.x headers and the KeePass 2 pre-release signature read with the
  KDBX 3 layout; KeePass 1.x is recognised and refused (`Error::KeePass1`).
- Times keep their zone: KDBX 3 text times without a designator are
  `Timestamp::Unzoned`, not UTC.
- Field references accept `O` (other fields) as a search field and stop after
  12 levels.
- Salsa20 is implemented in the crate; the other primitives are the RustCrypto
  `aes`, `twofish`, `chacha20`, `argon2`, `sha2` and `hmac` crates.

## Test data

`tests/data/pykeepass/` holds pykeepass's test databases and keyfiles from the
same commit (GPL-3.0; provenance and hashes in its README). The synthetic
corpus `tests/data/synthetic.json` is written by
`scripts/make-kdbx-fixtures.py` in the LovelyCarve repository.
