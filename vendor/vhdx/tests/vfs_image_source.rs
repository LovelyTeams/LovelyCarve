//! Integration test: VhdxReader::open_reader composes as a forensic-vfs ImageSource.
// Integration tests are separate crates; the lib's cfg_attr(test, allow(unwrap_used))
// does not apply here. Suppress pedantic lints that are normal in test helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_range_loop,
    clippy::many_single_char_names,
    clippy::unreadable_literal,
    clippy::items_after_statements,
    clippy::doc_markdown
)]
//!
//! Oracle: qemu-img 11.0.2 encoded `tests/data/fat.vhdx` from a FAT16 raw image
//! and decoded it back to `roundtrip.raw`. The SHA-256 of `roundtrip.raw` is the
//! ground-truth constant — an independent oracle produced it, not this reader.
//!
//! The test exercises the exact call path the forensic-vfs engine uses:
//!   `VhdxReader::open_reader(Box::new(Cursor::new(...)))` → `SeekPoolSource::single`.
//!
//! `SeekPoolSource` is NOT in the published `forensic-vfs 0.1.0` (it was added
//! in a later local commit). We test `ImageSource` type-compatibility here via
//! `forensic_vfs::ImageSource` and the manual `DynWrapper` below, matching the
//! engine's wrapped usage, while the SHA-256 / sector assertions are made directly
//! via `VhdxReader`'s own `Read + Seek` interface (which is what the engine
//! also wraps in SeekPoolSource).
//!
//! Generator commands (verbatim, for reproduction):
//!   hdiutil create -size 8m -fs "MS-DOS FAT16" -volname VHDXTEST /tmp/fat16img
//!   hdiutil convert /tmp/fat16img.dmg -format UDTO -o /tmp/fat16img_raw
//!   qemu-img convert -f raw -O vhdx /tmp/fat16img_raw.cdr /tmp/fat.vhdx
//!   qemu-img convert -f vhdx -O raw /tmp/fat.vhdx /tmp/roundtrip.raw
//!   qemu-img --version  →  qemu-img version 11.0.2

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use forensic_vfs::{ImageSource, SourceId};

/// SHA-256 of the round-trip raw produced by qemu-img (the independent oracle).
/// qemu encoded fat.vhdx, qemu decoded it back — neither value was authored by us.
const ROUNDTRIP_SHA256: &str = "f350908d5b99b0000f7bd4235ce841db1ee82c809f9fe9a19040257ec1eff4ed";

/// The virtual disk size qemu-img produces for an 8 MiB raw image → VHDX → raw.
const VIRTUAL_DISK_SIZE: u64 = 8 * 1024 * 1024; // 8 MiB

/// Offset of the MBR boot signature (0x55 0xAA) within the virtual disk.
const MBR_BOOT_SIG_OFFSET: u64 = 510;

/// Offset of the VHDXTEST volume label within the virtual disk.
/// FAT16 partition starts at sector 1 (offset 512); the 11-byte volume label
/// lives at offset 0x2B within the FAT boot sector = 512 + 43 = 555.
const VHDXTEST_LABEL_OFFSET: u64 = 555;

/// The VHDX under test: a FAT16 image with the "VHDXTEST" volume label,
/// encoded by qemu-img 11.0.2 from an 8 MiB raw image.
static FAT_VHDX: &[u8] = include_bytes!("data/fat.vhdx");

// ---------------------------------------------------------------------------
// A thin `ImageSource` wrapper so we can verify the `VhdxReader` type satisfies
// the `ImageSource` contract via `Arc<dyn ImageSource>` — the shape the engine uses.
// ---------------------------------------------------------------------------

/// Wraps a `Read + Seek` reader as an `ImageSource` using a mutex — the same
/// technique `SeekPoolSource::single` uses in the engine.
struct DynWrapper<R: Read + Seek + Send> {
    inner: Mutex<R>,
    len: u64,
}

impl<R: Read + Seek + Send> DynWrapper<R> {
    fn new(mut reader: R) -> Self {
        let len = reader.seek(SeekFrom::End(0)).unwrap();
        reader.seek(SeekFrom::Start(0)).unwrap();
        Self {
            inner: Mutex::new(reader),
            len,
        }
    }
}

impl<R: Read + Seek + Send + 'static> ImageSource for DynWrapper<R> {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> forensic_vfs::VfsResult<usize> {
        let io_err = |op: &'static str| {
            move |source: std::io::Error| forensic_vfs::VfsError::Io { op, source }
        };
        let avail = self.len.saturating_sub(offset);
        if avail == 0 {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(avail) as usize;
        let mut g = self.inner.lock().unwrap();
        g.seek(SeekFrom::Start(offset)).map_err(io_err("seek"))?;
        let mut total = 0;
        while total < want {
            match g.read(&mut buf[total..want]).map_err(io_err("read"))? {
                0 => break,
                n => total += n,
            }
        }
        Ok(total)
    }

    fn source_id(&self) -> SourceId {
        SourceId::ROOT
    }
}

/// Open via `open_reader` and confirm `virtual_disk_size` is reported correctly,
/// and the reader composes as `Arc<dyn ImageSource>` with matching `len`.
#[test]
fn open_reader_reports_correct_virtual_size() {
    let reader = vhdx::VhdxReader::open_reader(Box::new(Cursor::new(FAT_VHDX.to_vec())))
        .expect("open_reader must succeed on a valid VHDX");
    let vsize = reader.virtual_disk_size();
    assert_eq!(vsize, VIRTUAL_DISK_SIZE, "virtual disk size mismatch");

    // Wrap the reader as an ImageSource via our DynWrapper (same mutex pattern
    // that SeekPoolSource::single uses in the engine).
    let src: Arc<dyn ImageSource> = Arc::new(DynWrapper::new(reader));
    assert_eq!(
        src.len(),
        VIRTUAL_DISK_SIZE,
        "ImageSource::len must equal virtual_disk_size"
    );
}

/// Read the MBR boot signature — canonical FAT/MBR marker at virtual offset 510.
/// Tier-1 assertion: offset and expected bytes come from the qemu round-trip raw.
#[test]
fn read_at_returns_mbr_boot_signature() {
    let reader = vhdx::VhdxReader::open_reader(Box::new(Cursor::new(FAT_VHDX.to_vec())))
        .expect("open_reader must succeed");
    let vsize = reader.virtual_disk_size();
    let src: Arc<dyn ImageSource> = Arc::new(DynWrapper::new(reader));

    let mut sector = [0u8; 512];
    let n = src.read_at(0, &mut sector).expect("read sector 0");
    assert_eq!(n, 512, "sector 0 read returned wrong byte count");
    assert_eq!(
        &sector[MBR_BOOT_SIG_OFFSET as usize..MBR_BOOT_SIG_OFFSET as usize + 2],
        &[0x55, 0xAA],
        "MBR boot signature not found at offset {MBR_BOOT_SIG_OFFSET}"
    );
    let _ = vsize;
}

/// Read the FAT16 volume label — confirms right bytes from the oracle image decode faithfully.
#[test]
fn read_at_finds_vhdxtest_label() {
    let reader = vhdx::VhdxReader::open_reader(Box::new(Cursor::new(FAT_VHDX.to_vec())))
        .expect("open_reader must succeed");
    let vsize = reader.virtual_disk_size();
    let src: Arc<dyn ImageSource> = Arc::new(DynWrapper::new(reader));

    let mut buf = [0u8; 8];
    let n = src
        .read_at(VHDXTEST_LABEL_OFFSET, &mut buf)
        .expect("read label");
    assert_eq!(n, 8);
    assert_eq!(&buf, b"VHDXTEST", "FAT16 volume label mismatch");
    let _ = vsize;
}

/// Read the whole virtual disk, SHA-256 it, compare against the qemu oracle.
/// The constant was produced by `sha256sum /tmp/roundtrip.raw` where roundtrip.raw
/// was decoded by qemu-img from the committed fat.vhdx — independent of this reader.
#[test]
fn full_read_sha256_matches_oracle() {
    let mut reader = vhdx::VhdxReader::open_reader(Box::new(Cursor::new(FAT_VHDX.to_vec())))
        .expect("open_reader must succeed");

    reader.seek(SeekFrom::Start(0)).unwrap();
    let mut all = Vec::with_capacity(VIRTUAL_DISK_SIZE as usize);
    reader.read_to_end(&mut all).unwrap();
    assert_eq!(
        all.len(),
        VIRTUAL_DISK_SIZE as usize,
        "read_to_end length mismatch"
    );

    let digest = hex_lower(&sha256(&all));
    assert_eq!(
        digest, ROUNDTRIP_SHA256,
        "SHA-256 of full virtual disk does not match qemu oracle"
    );
}

/// A read that starts entirely past EOF via ImageSource must return 0.
#[test]
fn read_past_eof_returns_zero() {
    let reader = vhdx::VhdxReader::open_reader(Box::new(Cursor::new(FAT_VHDX.to_vec())))
        .expect("open_reader must succeed");
    let vsize = reader.virtual_disk_size();
    let src: Arc<dyn ImageSource> = Arc::new(DynWrapper::new(reader));

    let mut buf = [0xFFu8; 512];
    let n = src
        .read_at(vsize, &mut buf)
        .expect("read at EOF must not error");
    assert_eq!(n, 0, "read at or beyond EOF must return 0");
}

// ---------------------------------------------------------------------------
// Minimal SHA-256 — avoids adding sha2 as a dev-dep. Standard algorithm only;
// do NOT copy for production code — use the sha2 crate there.
// ---------------------------------------------------------------------------

fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64) * 8;
    // Pad: append 0x80, then zeros, then the 64-bit big-endian bit length.
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let compress = |state: &mut [u32; 8], block: &[u8]| {
        let mut w = [0u32; 64];
        for i in 0..16 {
            let j = i * 4;
            w[i] = u32::from_be_bytes([block[j], block[j + 1], block[j + 2], block[j + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    };

    for chunk in msg.chunks(64) {
        compress(&mut state, chunk);
    }
    let mut out = [0u8; 32];
    for (i, &s) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&s.to_be_bytes());
    }
    out
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
