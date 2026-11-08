#![no_main]

//! The full pipeline, entered the way a tool enters it: a directory on disk.
//!
//! The per-structure targets in `core/fuzz` each hand one parser its own bytes.
//! That misses the failures that only appear when the parsers are *composed* —
//! a plist whose `IsEncrypted` contradicts a `Manifest.db` that is not SQLite,
//! a keybag that parses but unwraps nothing, a manifest row pointing at a blob
//! that is not there. So this target builds a real backup directory and runs
//! `Backup::open` and `audit` over it.
//!
//! Both files are fuzzer-controlled, because fixing either one would leave the
//! branch that reconciles them permanently untested.

use std::fs;

use ios_backup_core::{Backup, Credentials, Password};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Two bytes of split point, so the fuzzer decides how much of its budget
    // goes to the plist and how much to the database rather than us fixing the
    // ratio. Anything shorter cannot carry both.
    let Some((head, body)) = data.split_first_chunk::<2>() else {
        return;
    };
    let split = usize::from(u16::from_le_bytes(*head)).min(body.len());
    let (plist_bytes, db_bytes) = body.split_at(split);

    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let root = dir.path();

    if fs::write(root.join("Manifest.plist"), plist_bytes).is_err() {
        return;
    }
    if fs::write(root.join("Manifest.db"), db_bytes).is_err() {
        return;
    }

    // A wrong password is the ordinary case here, not an edge case: almost no
    // fuzzer-generated keybag will unwrap. Offering one exercises the whole
    // derive-and-fail path instead of stopping at "no credentials".
    let credentials = Credentials::password(Password::new("fuzz"));

    if let Ok(mut backup) = Backup::open_with(root, &credentials) {
        let findings = ios_backup_forensic::audit(&backup);
        // Force every finding to render: a Display impl that indexes into
        // evidence is as reachable as the parser that produced it.
        for finding in &findings {
            let _ = format!("{finding:?}");
        }

        // Reading is where a manifest row and the blob tree meet, so walk it.
        let entries = backup.files().to_vec();
        for entry in entries.iter().take(64) {
            let _ = backup.read(entry);
        }
    }
});
