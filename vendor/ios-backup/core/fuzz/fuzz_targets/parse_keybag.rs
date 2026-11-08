#![no_main]

//! The keybag is the most attacker-reachable structure in a backup: it is a
//! TLV stream read *before* any password is known, so every length field in it
//! is untrusted input that the reader must survive.

use ios_backup_core::keybag::KeyBag;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = KeyBag::parse(data);
});
