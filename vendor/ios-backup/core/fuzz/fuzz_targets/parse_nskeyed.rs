#![no_main]

//! `NSKeyedArchiver` payloads carry an object table addressed by index, so a
//! malformed archive can point a reference at anything — including itself. The
//! resolver must terminate and stay in bounds on input it did not write.

use ios_backup_core::nskeyed::Archive;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(archive) = Archive::parse(data) {
        // Parsing alone leaves the object graph unwalked. Reach through it, so a
        // cyclic or out-of-range `CF$UID` is exercised rather than merely stored.
        let _ = archive.root();
        let _ = archive.root_i64("size");
        let _ = archive.root_string("path");
        let _ = archive.root_data("EncryptionKey");
    }
});
