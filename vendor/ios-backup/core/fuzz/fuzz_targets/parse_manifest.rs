#![no_main]

//! `Manifest.db` is a SQLite database supplied by the evidence, and in an
//! encrypted backup it is decrypted in memory before it is read — so the reader
//! meets arbitrary bytes whenever a password is wrong or a page is damaged.

use ios_backup_core::manifest::read_files;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(files) = read_files(data.to_vec()) {
        // A row is only half-parsed until its metadata blob is decoded and its
        // blob path derived; both read fields the fuzzer controls.
        for file in &files.files {
            let _ = file.blob_relative_path();
        }
        let _ = files.unreadable;
    }
});
