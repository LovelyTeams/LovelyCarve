# ios-backup-forensic

[![CI](https://github.com/SecurityRonin/ios-backup-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/ios-backup-forensic/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=githubsponsors)](https://github.com/sponsors/h4x0r)

**Read an encrypted iPhone backup in Rust — keybag, manifest and every file — without writing a byte of plaintext to disk.**

An encrypted iOS backup keeps its own index encrypted, so a tool that does not
implement the keybag reports `Manifest.db: file is not a database` and stops. The
evidence is intact and unreadable.

```rust
use ios_backup_core::{Backup, Credentials, Password};

let mut backup = Backup::open_with(
    std::path::Path::new("/evidence/00008110-001641201A29401E"),
    &Credentials::password(Password::from_file("pw.txt".as_ref())?),
)?;

println!("iOS {:?}, {} files", backup.metadata().product_version, backup.files().len());

let sms = backup.find("HomeDomain", "Library/SMS/sms.db").unwrap().clone();
std::fs::write("sms.db", backup.read(&sms)?)?;
# Ok::<(), ios_backup_core::Error>(())
```

```text
iOS Some("26.2"), 41832 files
```

## Two crates

| Crate | Role |
|---|---|
| **`ios-backup-core`** | The reader. File tree, metadata, decrypt-on-read. Emits no findings. |
| **`ios-backup-forensic`** | The auditor. Emits `forensicnomicon::report::Finding`. |

## What it reads

- **Encrypted and unencrypted backups**, Finder / iTunes / MobileSync.
- **The keybag** — `VERS`/`TYPE`/`SALT`/`ITER`, the iOS 10.2+ double-protection
  `DPSL`/`DPIC` round, and every per-class `WPKY`.
- **`Manifest.db`** — decrypted in memory and read with `sqlite-core`. No
  decrypted evidence is ever written to disk.
- **Per-file metadata** — the `NSKeyedArchiver` archive in `Files.file`, so
  size, mode, inode, timestamps and protection class come out typed.
- **Every file's bytes**, truncated to the manifest-recorded length rather than
  by stripping padding ([why](docs/decisions/0003-no-padding-strip-truncate-to-manifest-size.md)).

## Correctness

The cryptography is pinned to published vectors — RFC 3394 §4.1–§4.6, RFC 6070,
RFC 7914 §11, NIST SP 800-38A — and the full pipeline is checked against an
**independent implementation**: fixtures encrypted by Python's `cryptography`
(OpenSSL-backed) must decrypt byte-for-byte here.

Against a real encrypted backup the keybag parses to 11 class keys and the
production KDF — 10,000,000 SHA-256 rounds, then 10,000 SHA-1 — rejects a wrong
password in 3.16 s.

One gap is open and stated: no byte-exact decrypt of *real* evidence has been
performed, because that needs a real backup and its password. Full tiering, the
mutation controls, and what is not validated at all:
**[docs/validation.md](docs/validation.md)**.

## Robustness

Structurally hardened against hostile input: `unsafe_code = "forbid"`,
`unwrap_used`/`expect_used` denied in production code, and every integer read
through [`safe-read`](https://crates.io/crates/safe-read)'s bounded readers. A
length field is never trusted to be in range, and an unrecognised value is
reported verbatim rather than hidden.

**Not yet fuzzed.** ADR-0012 requires a fuzz target per parsed structure and a
full-pipeline harness; neither exists yet, so the panic-free claim rests on the
lint posture and hand-written malformed-input tests, not on a fuzzer having
failed to break it. Tracked in [validation.md](docs/validation.md).

A wrong password is always reported as a wrong password — never as an empty
backup, never as a corrupt keybag, and never as partial garbage
([why](docs/decisions/0004-unwrap-failure-is-a-wrong-password.md)).

## Passwords

Supply routes, ordered by how much they leak — which is also the recommendation:

| Route | Who else can see it |
|---|---|
| `Password::from_tty` — prompt, no echo | nobody |
| `Password::from_file` | anyone who can read the file |
| `Password::from_env` | the process tree, `/proc`, crash dumps |
| `Password::new` from a CLI argument | **every user on the machine**, via `ps` |

`Password` zeroizes on drop and renders as `Password(<redacted>)`, so a stray
`{:?}` cannot put a credential in a log.

## Mounting

A backup is a *logical* container — a captured file tree, no partition table, no
filesystem — so it belongs with AD1 and AFF4-Logical. `logical::LogicalView`
presents exactly the entry-list-plus-read-by-index shape
`disk_forensic::logical` consumes, so wiring it into `4n6mount` is an adapter
rather than a new code path in every consumer.

The one piece that does not fit yet is the password: `disk_forensic::logical::open`
takes no credentials, and already turns away an encrypted AFF4 for the same
reason. The small upstream change is specified in
**[ADR-0005](docs/decisions/0005-credentials-seam-for-mounting-tools.md)**.

## Install

```toml
[dependencies]
ios-backup-core = "0.1"
```

MSRV 1.88 — inherited from `plist`, not from this crate's own code ([why](docs/decisions/0007-msrv-is-inherited-from-plist.md)).

---

[Privacy](https://securityronin.github.io/ios-backup-forensic/privacy/) ·
[Terms](https://securityronin.github.io/ios-backup-forensic/terms/) ·
© Security Ronin Ltd
