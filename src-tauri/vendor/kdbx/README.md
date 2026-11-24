# kdbx-reader

Read-only KeePass KDBX 3.x / 4.x reader in pure Rust: a rewrite of the reading
half of [pykeepass](https://github.com/libkeepass/pykeepass) 4.2.0. See
[ORIGIN.md](ORIGIN.md) for what was taken over and what changed, and
[KDBX](../../../docs/formats/KDBX.md) for capabilities, limits and verification.

```rust
let key = kdbx::Key::new().with_password("password").with_keyfile(&keyfile_bytes)?;
let database = kdbx::Database::open(&file_bytes, &key)?;
for entry in database.entries() {
    println!("{:?} {:?}", entry.title(), entry.username());
}
// Without a key: version, cipher, compression and KDF parameters.
let header = kdbx::Header::parse(&file_bytes)?;
```

- `Header::parse`: the outer header, no key needed.
- `Database::open` / `open_with_limits`: AES-KDF, Argon2d, Argon2id; AES-256,
  Twofish, ChaCha20; GZip; Salsa20 / ChaCha20 inner streams; every block
  checked; the XML read into groups, entries, history, attachments, metadata
  and deleted objects.
- Queries: `groups`, `entries`, `find_entries`, `find_groups`, `entry_by_uuid`,
  `group_by_uuid`, `parent_path`, `recycle_bin`, `binary`, `attachment_data`,
  `deref` (field references), `xml` (plaintext XML), `transformed_key`,
  `database_salt`.

Nothing is written. Tests: `just test-kdbx` (see `tests/data/README.md`).

License: GPL-3.0-only (see `LICENSE`), as pykeepass.
