# sqlite-core

Native, read-only, input-fuzzed, panic-free-by-construction SQLite file-format reader for forensics.

`sqlite-core` parses the raw SQLite database file format directly — header,
pages, cells, overflow chains, the freelist, index b-trees (including
`WITHOUT ROWID` tables, whose rows live in the index), and an uncheckpointed
`-wal` overlay — without linking the SQLite engine. It never writes the evidence
file or its sidecars, and it is built to survive hostile input: malformed,
truncated, and corrupted databases return typed errors instead of panicking.

It is the raw decode layer consumed by [`sqlite-forensic`](../forensic) (the
anomaly auditor and deleted-record carver) and the `sqlite4n6` CLI.

It also provides a small SQLite *writer* (the `rebuild` module) that materializes
recovered records into a **fresh** database — pure Rust, no engine linkage, used
to emit the CLI's `--db` `*.carved.db`. This writes only the new output file; the
evidence database is never written.

## Use

```rust
use sqlite_core::Database;

let bytes = std::fs::read("evidence.db")?;
let db = Database::open(bytes)?; // bytes are owned, never written back
println!("{} pages", db.page_count());
```

[Privacy Policy](https://securityronin.github.io/sqlite-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/sqlite-forensic/terms/) · © 2026 Security Ronin Ltd
