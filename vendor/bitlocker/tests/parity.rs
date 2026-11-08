//! Committed independent crypto vectors + format and failure regressions.
use bitlocker::{BdeError, BitLockerVolume};
use std::io::{Cursor, Read, Seek, SeekFrom};

fn projection(method: u16, size: usize) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/data/parity-{method:04x}-{size}.bin"));
    let data = std::fs::read(path).unwrap();
    let block_size = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let mut image = vec![0; 0x80000];
    image[..512].copy_from_slice(&data[8..520]);
    for offset in [0x20000, 0x30000, 0x40000] {
        image[offset..offset + block_size].copy_from_slice(&data[520..520 + block_size]);
    }
    let sectors = &data[520 + block_size..];
    image[0x10000..0x10000 + size].copy_from_slice(&sectors[..size]);
    image[0x12000..0x12000 + size].copy_from_slice(&sectors[size..]);
    image[0x60000..0x60000 + size].fill(0x7a);
    image
}

fn plaintext(size: usize) -> Vec<u8> {
    let mut plain: Vec<_> = (0..size).map(|index| (index * 7 + 19) as u8).collect();
    plain[3..11].copy_from_slice(b"NTFS    ");
    plain
}

#[test]
fn all_six_methods_512_and_4096_match_independent_crypto() {
    for method in 0x8000..=0x8005 {
        for size in [512, 4096] {
            let image = projection(method, size);
            let mut volume = BitLockerVolume::unlock_clear_key(Cursor::new(&image)).unwrap();
            let plain = plaintext(size);
            for offset in [0, 0x12000] {
                let mut output = vec![0; size];
                volume.read_at(offset, &mut output).unwrap();
                assert_eq!(output, plain, "{method:04x}/{size} at {offset}");
            }
            let mut tail = vec![0; size];
            volume.read_at(0x60000, &mut tail).unwrap();
            assert_eq!(tail, vec![0x7a; size]);
            for offset in [0x10000, 0x20000, 0x20000 + 0xf000, 0x30000, 0x40000] {
                volume.read_at(offset, &mut tail).unwrap();
                assert!(tail.iter().all(|byte| *byte == 0));
            }
            let mut unaligned = vec![0; size - 13];
            volume.read_at(13, &mut unaligned).unwrap();
            assert_eq!(unaligned, plain[13..]);
            volume.seek(SeekFrom::Start(0)).unwrap();
            let mut sequential = vec![0; size];
            volume.read_exact(&mut sequential).unwrap();
            assert_eq!(sequential, plain);
            let mut vmk =
                BitLockerVolume::unlock_with_vmk(Cursor::new(&image), &[0x46; 32]).unwrap();
            vmk.read_at(0, &mut sequential).unwrap();
            assert_eq!(sequential, plain);
            assert!(matches!(
                BitLockerVolume::unlock_with_vmk(Cursor::new(&image), &[0x47; 32]),
                Err(BdeError::AuthenticationFailed { .. })
            ));
            let key_size = match method {
                0x8000 | 0x8002 => 16,
                0x8005 => 64,
                _ => 32,
            };
            let fvek: Vec<u8> = (1..=key_size).collect();
            let tweak: Vec<u8> = match method {
                0x8000 => (65..81).collect(),
                0x8001 => (65..97).collect(),
                _ => vec![],
            };
            let mut keys =
                BitLockerVolume::unlock_with_keys(Cursor::new(&image), &fvek, &tweak).unwrap();
            keys.read_at(0, &mut sequential).unwrap();
            assert_eq!(sequential, plain);
            assert!(BitLockerVolume::unlock_with_keys(
                Cursor::new(&image),
                &fvek[..fvek.len() - 1],
                &tweak
            )
            .is_err());
            let mut startup = BitLockerVolume::unlock_with_startup_key(
                Cursor::new(&image),
                include_bytes!("data/parity-startup.bek"),
            )
            .unwrap();
            startup.read_at(0, &mut sequential).unwrap();
            assert_eq!(sequential, plain);
        }
    }
}

#[test]
fn metadata_fallback_rejects_truncated_and_malformed_primary() {
    let mut image = projection(0x8002, 512);
    image[0x20040..0x20044].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut volume = BitLockerVolume::unlock_clear_key(Cursor::new(&image)).unwrap();
    let mut output = [0; 512];
    volume.read_at(0, &mut output).unwrap();
    assert_eq!(output.as_slice(), plaintext(512));
    // An out-of-volume first candidate must still permit the backup.
    image[176..184].copy_from_slice(&(u64::MAX - 16).to_le_bytes());
    assert!(BitLockerVolume::unlock_clear_key(Cursor::new(&image)).is_ok());
}

#[test]
fn startup_key_identifier_and_seek_overflow_fail() {
    let image = projection(0x8001, 512);
    let mut bek = include_bytes!("data/parity-startup.bek").to_vec();
    bek[56] ^= 1;
    assert!(BitLockerVolume::unlock_with_startup_key(Cursor::new(&image), &bek).is_err());
    let mut volume = BitLockerVolume::unlock_clear_key(Cursor::new(&image)).unwrap();
    volume.seek(SeekFrom::Start(u64::MAX)).unwrap();
    assert!(volume.seek(SeekFrom::Current(1)).is_err());
}

#[test]
fn vista_plain_boot_and_mft_mirror_are_restored() {
    let mut image = projection(0x8002, 512);
    image[..3].copy_from_slice(&[0xeb, 0x52, 0x90]);
    image[160..176].fill(0);
    image[13] = 8;
    image[56..64].copy_from_slice(&32u64.to_le_bytes());
    for offset in [0x20000, 0x30000, 0x40000] {
        image[offset + 10..offset + 12].copy_from_slice(&1u16.to_le_bytes());
        image[offset + 56..offset + 64].copy_from_slice(&123u64.to_le_bytes());
    }
    let mut volume = BitLockerVolume::unlock_clear_key(Cursor::new(&image)).unwrap();
    let mut output = [0; 512];
    volume.read_at(0, &mut output).unwrap();
    assert_eq!(&output[3..11], b"NTFS    ");
    assert_eq!(&output[56..64], &123u64.to_le_bytes());
    volume.read_at(0x12000, &mut output).unwrap();
    assert_eq!(output.as_slice(), plaintext(512));
}

fn used_space_image() -> Vec<u8> {
    let mut image = projection(0x8004, 512);
    image[160..176].copy_from_slice(&[
        0x3b, 0x4d, 0xa8, 0x92, 0x80, 0xdd, 0x0e, 0x4d, 0x9e, 0x4e, 0xb1, 0xe3, 0x28, 0x4e, 0xae,
        0xd8,
    ]);
    image[200..208].copy_from_slice(&0x50000u64.to_le_bytes());
    image[208..216].copy_from_slice(&0x51000u64.to_le_bytes());
    for offset in [0x50000, 0x51000] {
        image[offset..offset + 8].copy_from_slice(b"FVE-EOW\0");
        image[offset + 8..offset + 10].copy_from_slice(&56u16.to_le_bytes());
        image[offset + 12..offset + 16].copy_from_slice(&512u32.to_le_bytes());
        image[offset + 16..offset + 20].copy_from_slice(&512u32.to_le_bytes());
        image[offset + 20..offset + 24].copy_from_slice(&8192u32.to_le_bytes());
        image[offset + 32..offset + 36].copy_from_slice(&1u32.to_le_bytes());
        image[offset + 56..offset + 64].copy_from_slice(&0x52000u64.to_le_bytes());
    }
    let offset = 0x52000;
    image[offset..offset + 10].copy_from_slice(b"FVE-EOWBM\0");
    image[offset + 10..offset + 12].copy_from_slice(&64u16.to_le_bytes());
    image[offset + 12..offset + 16].copy_from_slice(&4096u32.to_le_bytes());
    image[offset + 20..offset + 28].copy_from_slice(&0x12000u64.to_le_bytes());
    image[offset + 28..offset + 36].copy_from_slice(&0x4000u64.to_le_bytes());
    image[offset + 52..offset + 56].copy_from_slice(&512u32.to_le_bytes());
    for (offset, sequence, bitmap) in [(0x52200, 1u32, 0xff), (0x52400, 2u32, 1)] {
        image[offset..offset + 10].copy_from_slice(b"FVE-EOWBR\0");
        image[offset + 10..offset + 12].copy_from_slice(&36u16.to_le_bytes());
        image[offset + 16..offset + 20].copy_from_slice(&2u32.to_le_bytes());
        image[offset + 20..offset + 24].copy_from_slice(&sequence.to_le_bytes());
        image[offset + 36] = bitmap;
    }
    image[0x14000..0x14200].fill(0x3e);
    image
}

#[test]
fn used_space_bitmap_uses_latest_record_and_keeps_plaintext_raw() {
    let mut image = used_space_image();
    // A bad descriptor is recoverable through the second descriptor.
    image[0x50000] = 0;
    let mut volume = BitLockerVolume::unlock_clear_key(Cursor::new(image)).unwrap();
    for offset in [0x14000, 0x12000, 0x50000, 0x52000, 0x14000] {
        let mut output = [0; 512];
        volume.read_at(offset, &mut output).unwrap();
        match offset {
            0x14000 => assert_eq!(output, [0x3e; 512]),
            0x12000 => assert_eq!(output.as_slice(), plaintext(512)),
            _ => assert_eq!(output, [0; 512]),
        }
    }
}

#[test]
fn used_space_corrupt_geometry_and_ambiguous_records_fail_explicitly() {
    for (offset, value) in [
        (0x52400 + 20, 1u32),
        (0x52400 + 16, 1),
        (0x52000 + 52, u32::MAX),
    ] {
        let mut image = used_space_image();
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(BitLockerVolume::unlock_clear_key(Cursor::new(image)).is_err());
    }
}

#[test]
fn none_method_needs_no_key_and_malformed_entries_cannot_win_fallback() {
    let mut image = projection(0x8002, 512);
    for offset in [0x20000, 0x30000, 0x40000] {
        image[offset + 100..offset + 102].fill(0);
    }
    image[0x10000..0x10200].copy_from_slice(&plaintext(512));
    let mut volume = BitLockerVolume::unlock_clear_key(Cursor::new(&image)).unwrap();
    let mut output = [0; 512];
    volume.read_at(0, &mut output).unwrap();
    assert_eq!(output.as_slice(), plaintext(512));
    image[0x20070..0x20072].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(BitLockerVolume::unlock_clear_key(Cursor::new(image)).is_ok());
}
