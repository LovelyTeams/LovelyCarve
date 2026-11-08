# Vendored iOS backup reader and auditor

- Source: https://github.com/SecurityRonin/ios-backup-forensic
- Commit: `8d8cef447ae212e7dd22dc0425156bf4ddfcfa05`
- License: Apache-2.0; the original LICENSE and crate README files are retained.

Local changes:

- Share LovelyCarve's vendored `sqlite-core` and enable report serialization.
- Add cancellation polling to the auditor; cancelled work never returns a partial report.
- Add bounded plaintext streaming and CBC range reads in `core/src/backup/stream.rs`, reject short blobs for known manifest sizes, and recover absent sizes from the final encrypted block.
- Replace the `sqlite3` executable dependency in the unreadable-row test with a committed synthetic database generated using Python's standard `sqlite3`. Its schema and two rows match the original test exactly.

The encrypted fixtures are upstream synthetic test data, not user evidence.
The two `streaming-*` fixtures extend the same independent Python generator
with a 262401-byte payload. Regenerate with Python + cryptography:

```text
python tools/mint_streaming_backup.py tests/data/streaming-plain --plain
python tools/mint_streaming_backup.py tests/data/streaming-encrypted
```

Real-device decryption still requires validation against a backup and its password;
synthetic cross-implementation checks do not establish coverage of every iOS version.
