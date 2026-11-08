#![no_main]
//! Fuzz the volume-header + metadata parse path over arbitrary bytes. Invariant:
//! must never panic — the header signature check, metadata-block navigation,
//! encryption-context substring extraction, and segment-descriptor parse all
//! run on attacker-controllable input.

use std::io::Cursor;

use filevault::metadata::{
    locate_encrypted_metadata, parse_segments, plaintext_metadata_size,
};
use filevault::volume_header::VolumeHeader;
use filevault::{context, parse_info};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Whole-pipeline parse over a reader (header -> metadata -> context/LV info).
    let _ = parse_info(Cursor::new(data.to_vec()));

    // Header parse in isolation.
    if let Ok(header) = VolumeHeader::parse(data) {
        // Treat the arbitrary bytes as both a first block and a full region.
        let _ = plaintext_metadata_size(&header, data);
        let _ = locate_encrypted_metadata(&header, data);
    }

    // Metadata block navigation / segment parse over arbitrary bytes.
    let _ = parse_segments(data, 4096);
    let _ = parse_segments(data, 0);

    // Encryption-context and LV-info substring extraction over arbitrary bytes.
    let _ = context::EncryptionContext::extract(data);
    let _ = context::LogicalVolumeInfo::extract(data);
});
