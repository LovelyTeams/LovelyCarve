//! BitLocker VMK extractor + FVE-driven recovery-key derivation.
//!
//! Two-step pipeline:
//!
//! 1. `scan_memory_file` mmaps a RAW memory dump and locates every
//!    `FVE_KEY_DATUM` candidate. A candidate is a 44-byte structure whose
//!    fixed-byte header survives two masked integer comparisons and whose
//!    32-byte key region carries enough entropy.
//! 2. `recovery_key_from_vmk` reads the BitLocker volume header at a
//!    partition slice, walks the FVE metadata to the first AES-CCM datum
//!    beneath the VMK Info → Stretch Key path, AES-CTR-decrypts it with the
//!    candidate VMK and, if the embedded Key datum is 16 bytes, formats the
//!    BitLocker 48-digit recovery password.
//!
//! The recovered password enters the same pure Rust authenticated unlock path
//! as an explicitly supplied recovery password; the existing IPC contract
//! returns both the recovered password and the unlocked-volume status.

use std::collections::HashSet;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use aes::cipher::{BlockCipherEncrypt, KeyInit};
use aes::{Aes128, Aes256};
use anyhow::{anyhow, Context, Result};
use memchr::memchr_iter;
use memmap2::Mmap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};

use crate::evidence::image::{BitlockerStatus, DiskImage};
use crate::evidence::partitions::{Partition, PartitionKind};

const FVE_MAGIC: &[u8; 8] = b"-FVE-FS-";

const DATUM_SIZE: usize = 44;
const HEADER_MARKER: u8 = 0x2C;
const HEADER_WORD0_MASK: u64 = 0xFFFF_FFFF_FF00_FFFF;
const HEADER_WORD0_EXPECTED: u64 = 0x0000_0001_0000_002C;
const HEADER_WORD1_MASK: u32 = 0xFF00_FF00;
const HEADER_WORD1_EXPECTED: u32 = 0x0000_2000;
const MAX_ZEROS_IN_KEY: u32 = 3;
const SCAN_CHUNK: usize = 1 << 20;
const SCAN_OVERLAP: usize = DATUM_SIZE - 1;

/// One VMK candidate produced by `scan_memory_file`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmkCandidate {
    pub offset: u64,
    pub type_flag: u8,
    pub key_type: u8,
    pub vmk_hex: String,
}

#[inline(always)]
fn header_matches(slice: &[u8]) -> bool {
    if slice.len() < 12 {
        return false;
    }
    let word0 = u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]);
    if (word0 & HEADER_WORD0_MASK) != HEADER_WORD0_EXPECTED {
        return false;
    }
    let word1 = u32::from_le_bytes([slice[8], slice[9], slice[10], slice[11]]);
    (word1 & HEADER_WORD1_MASK) == HEADER_WORD1_EXPECTED
}

/// Branchless SWAR popcount of zero bytes in a 32-byte key region —
/// `(x − 0x01..) & !x & 0x80..` lights one high bit per zero byte.
#[inline(always)]
fn count_zero_bytes_32(key: &[u8]) -> u32 {
    if key.len() < 32 {
        return 0;
    }
    let mut total: u32 = 0;
    for chunk in key[..32].as_chunks::<8>().0 {
        let lane = u64::from_le_bytes(*chunk);
        let zeros = lane.wrapping_sub(0x0101_0101_0101_0101) & !lane & 0x8080_8080_8080_8080;
        total += zeros.count_ones();
    }
    total
}

fn scan_chunk(data: &[u8], chunk_base: u64) -> Vec<VmkCandidate> {
    if data.len() < DATUM_SIZE {
        return Vec::new();
    }
    let horizon = data.len() - DATUM_SIZE + 1;
    let mut out = Vec::new();
    for index in memchr_iter(HEADER_MARKER, &data[..horizon]) {
        let slice = &data[index..index + DATUM_SIZE];
        if !header_matches(slice) {
            continue;
        }
        if count_zero_bytes_32(&slice[12..44]) > MAX_ZEROS_IN_KEY {
            continue;
        }
        out.push(VmkCandidate {
            offset: chunk_base + index as u64,
            type_flag: slice[2],
            key_type: slice[8],
            vmk_hex: hex::encode(&slice[12..44]),
        });
    }
    out
}

/// Parallel SIMD-assisted scan of a RAW memory dump. Convenience wrapper
/// over [`scan_memory_file_with_progress`] with a no-op progress callback
/// and a never-set cancel flag — kept for unit tests and any caller that
/// does not need the progress-event plumbing.
pub fn scan_memory_file(path: &Path) -> Result<Vec<VmkCandidate>> {
    let cancel = AtomicBool::new(false);
    scan_memory_file_with_progress(path, &|_done, _total| {}, &cancel)
}

/// Parallel scan with throttled progress callbacks and a cooperative
/// cancellation flag. The caller passes `&AtomicBool`; flip it to true at
/// any time to drain the rest of the rayon work as no-ops within seconds.
/// `progress` fires roughly every 150 ms while the scan runs (plus one
/// final 100% tick). The function returns `Err("memory scan cancelled")`
/// when cancellation was observed before completion.
pub fn scan_memory_file_with_progress(
    path: &Path,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
    cancel: &AtomicBool,
) -> Result<Vec<VmkCandidate>> {
    let file = File::open(path).with_context(|| format!("open '{}'", path.display()))?;
    // SAFETY: read-only mmap of a path the user just supplied; matches the
    // standard memmap2 contract used elsewhere in the project (see
    // `core::evidence::image::ImageSource::open`).
    let mmap = unsafe { Mmap::map(&file) }.with_context(|| format!("mmap '{}'", path.display()))?;

    let total = mmap.len() as u64;
    progress(0, total);
    if total == 0 {
        return Ok(Vec::new());
    }

    let counter = AtomicU64::new(0);
    let stop = AtomicBool::new(false);

    let candidates = std::thread::scope(|scope| -> Vec<VmkCandidate> {
        // Ticker thread — emits a progress event every 150 ms until the
        // rayon collect returns and we set `stop`. Cheap (one atomic load
        // per tick) and avoids cross-thread synchronization on the progress
        // channel during the hot scan loop.
        let ticker = scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(150));
                let done = counter.load(Ordering::Relaxed).min(total);
                progress(done, total);
            }
        });

        let starts: Vec<usize> = (0..mmap.len()).step_by(SCAN_CHUNK).collect();
        let mut candidates: Vec<VmkCandidate> = starts
            .into_par_iter()
            .flat_map_iter(|base| {
                if cancel.load(Ordering::Relaxed) {
                    return Vec::new().into_iter();
                }
                let end = (base + SCAN_CHUNK + SCAN_OVERLAP).min(mmap.len());
                let result = scan_chunk(&mmap[base..end], base as u64);
                counter.fetch_add(SCAN_CHUNK as u64, Ordering::Relaxed);
                result.into_iter()
            })
            .collect();

        stop.store(true, Ordering::Relaxed);
        let _ = ticker.join();

        candidates.sort_unstable_by_key(|candidate| candidate.offset);
        candidates.dedup_by_key(|candidate| candidate.offset);
        candidates
    });

    progress(total, total);
    if cancel.load(Ordering::Relaxed) {
        anyhow::bail!("memory scan cancelled");
    }
    Ok(candidates)
}

// ── FVE metadata parsing ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct FveDatum<'a> {
    entry_type: u16,
    data: &'a [u8],
}

fn iter_fve_datums(block: &[u8]) -> Vec<FveDatum<'_>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= block.len() {
        let size = u16::from_le_bytes([block[pos], block[pos + 1]]) as usize;
        if size < 8 || pos + size > block.len() {
            break;
        }
        let entry_type = u16::from_le_bytes([block[pos + 4], block[pos + 5]]);
        out.push(FveDatum {
            entry_type,
            data: &block[pos + 8..pos + size],
        });
        pos += size;
    }
    out
}

struct AesCcmDatum<'a> {
    counter_base: [u8; 12],
    ciphertext: &'a [u8],
}

fn parse_aes_ccm_datum<'a>(datum: &FveDatum<'a>) -> Option<AesCcmDatum<'a>> {
    let body = datum.data;
    if body.len() < 28 {
        return None;
    }
    let mut counter_base = [0u8; 12];
    counter_base.copy_from_slice(&body[0..12]);
    Some(AesCcmDatum {
        counter_base,
        ciphertext: &body[28..],
    })
}

#[allow(clippy::large_enum_variant)]
enum AesCipher {
    Aes128(Aes128),
    Aes256(Aes256),
}

/// AES-CTR counter block is
/// `[0x02, counter_base[0..11], counter_be_3bytes]` with counter starting
/// at 1 (counter 0 is reserved for the AES-CCM MAC tag).
fn aes_ctr_decrypt(key: &[u8], counter_base: &[u8; 12], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let cipher = match key.len() {
        16 => AesCipher::Aes128(
            Aes128::new_from_slice(key).map_err(|_| anyhow!("AES-128 init: bad key length"))?,
        ),
        32 => AesCipher::Aes256(
            Aes256::new_from_slice(key).map_err(|_| anyhow!("AES-256 init: bad key length"))?,
        ),
        other => return Err(anyhow!("unsupported AES key size: {other}")),
    };

    let mut plaintext = ciphertext.to_vec();
    let mut counter_block = [0u8; 16];
    counter_block[0] = 0x02;
    counter_block[1..13].copy_from_slice(counter_base);

    let mut offset = 0usize;
    let mut counter: u32 = 1;
    while offset < plaintext.len() {
        counter_block[13] = ((counter >> 16) & 0xFF) as u8;
        counter_block[14] = ((counter >> 8) & 0xFF) as u8;
        counter_block[15] = (counter & 0xFF) as u8;

        let mut keystream: aes::Block = counter_block.into();
        match &cipher {
            AesCipher::Aes128(inner) => inner.encrypt_block(&mut keystream),
            AesCipher::Aes256(inner) => inner.encrypt_block(&mut keystream),
        }

        let block_size = (plaintext.len() - offset).min(16);
        for index in 0..block_size {
            plaintext[offset + index] ^= keystream[index];
        }
        offset += block_size;
        counter = counter.wrapping_add(1);
    }
    Ok(plaintext)
}

fn decrypt_aes_ccm(entry: &AesCcmDatum<'_>, key: &[u8]) -> Result<Option<Vec<u8>>> {
    let plaintext = aes_ctr_decrypt(key, &entry.counter_base, entry.ciphertext)?;
    for inner in &iter_fve_datums(&plaintext) {
        if inner.entry_type == 1 && inner.data.len() >= 4 {
            return Ok(Some(inner.data.to_vec()));
        }
    }
    Ok(None)
}

fn read_u32_le(buf: &[u8], off: usize) -> Option<u32> {
    let slice = buf.get(off..off + 4)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

fn read_u64_le(buf: &[u8], off: usize) -> Option<u64> {
    let slice = buf.get(off..off + 8)?;
    Some(u64::from_le_bytes(slice.try_into().ok()?))
}

/// Locate the first valid FVE metadata block at the given BitLocker volume
/// offset. Tries each of the three header-recorded slots and returns
/// `(metadata_offset, slot)` for the first one whose magic matches.
fn locate_fve_metadata(image: &DiskImage, volume_offset: u64) -> Result<Option<(u64, u8)>> {
    let header = image.read_range(volume_offset, 0xC8)?;
    if header.len() < 0xC8 || &header[3..11] != FVE_MAGIC {
        return Ok(None);
    }
    for slot in 0..3u8 {
        let Some(rel) = read_u64_le(&header, 0xB0 + (slot as usize) * 8) else {
            continue;
        };
        let abs = volume_offset.wrapping_add(rel);
        let probe = image.read_range(abs, 8)?;
        if probe == FVE_MAGIC {
            return Ok(Some((abs, slot)));
        }
    }
    Ok(None)
}

/// Read the entries block of the first metadata area anchored at
/// `magic_off`. First-metadata-area layout:
///   `area[0..4]`  total size of the area
///   `area[8..12]` offset where entries start, relative to the area base
fn read_fve_metadata_entries(image: &DiskImage, magic_off: u64) -> Result<Option<Vec<u8>>> {
    let area_off = magic_off + 64;
    let header = image.read_range(area_off, 12)?;
    if header.len() < 12 {
        return Ok(None);
    }
    let Some(total_size) = read_u32_le(&header, 0) else {
        return Ok(None);
    };
    let Some(entry_start) = read_u32_le(&header, 8) else {
        return Ok(None);
    };
    let total_size = total_size as u64;
    let entry_start = entry_start as u64;
    if entry_start < 8 || entry_start >= total_size {
        return Ok(None);
    }
    let abs_start = area_off + entry_start;
    let length = (total_size - entry_start) as usize;
    let entries = image.read_range(abs_start, length)?;
    if entries.len() < length {
        return Ok(None);
    }
    Ok(Some(entries))
}

/// Format 16 key bytes as a BitLocker 48-digit recovery key: 8 groups of
/// 6 digits, each `u16_le * 11`, joined with '-'.
fn format_recovery_key(key_bytes: &[u8]) -> String {
    debug_assert!(key_bytes.len() >= 16);
    let mut groups = Vec::with_capacity(8);
    for index in 0..8 {
        let value = u16::from_le_bytes([key_bytes[2 * index], key_bytes[2 * index + 1]]);
        groups.push(format!("{:06}", value as u32 * 11));
    }
    groups.join("-")
}

fn try_extract_recovery_key(fve_area: &[u8], vmk: &[u8]) -> Result<Option<String>> {
    for datum in &iter_fve_datums(fve_area) {
        if datum.entry_type != 8 || datum.data.len() <= 28 {
            continue;
        }
        for stretch in &iter_fve_datums(&datum.data[28..]) {
            if stretch.entry_type != 3 || stretch.data.len() <= 20 {
                continue;
            }
            for ccm_datum in &iter_fve_datums(&stretch.data[20..]) {
                if ccm_datum.entry_type != 5 {
                    continue;
                }
                let Some(ccm) = parse_aes_ccm_datum(ccm_datum) else {
                    break;
                };
                if let Some(key_data) = decrypt_aes_ccm(&ccm, vmk)? {
                    let key_bytes = &key_data[4..];
                    if key_bytes.len() == 16 {
                        return Ok(Some(format_recovery_key(key_bytes)));
                    }
                }
                break;
            }
        }
    }
    Ok(None)
}

/// Resolve the BitLocker volume header offset for `partition`. Returns
/// `None` when the partition is not BitLocker.
pub fn partition_volume_offset(partition: &Partition, sector_size: usize) -> Option<u64> {
    if partition.kind != PartitionKind::Bitlocker {
        return None;
    }
    Some(partition.start_lba * sector_size as u64)
}

/// Derive the BitLocker 48-digit recovery key for `partition` using the
/// supplied VMK. Returns `Ok(None)` if the metadata cannot be parsed or
/// the VMK does not unlock the AES-CCM datum.
pub fn recovery_key_from_vmk(
    image: &DiskImage,
    partition: &Partition,
    vmk: &[u8],
) -> Result<Option<String>> {
    let Some(volume_offset) = partition_volume_offset(partition, image.sector_size) else {
        return Ok(None);
    };
    let Some((metadata_offset, _slot)) = locate_fve_metadata(image, volume_offset)? else {
        return Ok(None);
    };
    let Some(entries) = read_fve_metadata_entries(image, metadata_offset)? else {
        return Ok(None);
    };
    try_extract_recovery_key(&entries, vmk)
}

/// Decode a 64-character lowercase/uppercase hex VMK string into 32 bytes.
pub fn parse_vmk_hex(input: &str) -> Result<Vec<u8>> {
    let trimmed: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = hex::decode(&trimmed).map_err(|err| anyhow!("invalid VMK hex: {err}"))?;
    if bytes.len() != 32 && bytes.len() != 16 {
        anyhow::bail!(
            "VMK must be 32 bytes (64 hex chars) or 16 bytes (32 hex chars), got {}",
            bytes.len()
        );
    }
    Ok(bytes)
}

// ── IPC types and input fingerprinting ───────────────────────────────────────
//
// The structs below augment `VmkCandidate` with the recovery key derived
// against a specific partition's FVE metadata, plus a "已尝试" flag the
// frontend uses to dim VMKs the user already pasted into a failed unlock.
// They live here so `bde_vmk` owns the full BitLocker-input domain
// end-to-end — input normalisation, hashing for de-dup, and the
// IPC-shaped result types.
//
// Hash fingerprints are SHA-1 of the *normalised* input. SHA-1 is fine
// for identity-only de-dup (we are never proving a key is good — we are
// just recognising "the user pasted this exact wrong key already"); we
// never retain plaintext credentials.

#[derive(Debug, Serialize)]
pub struct VmkScanCandidate {
    pub offset: u64,
    pub type_flag: u8,
    pub key_type: u8,
    pub vmk_hex: String,
    /// Recovery key derived against the BitLocker partition's FVE
    /// metadata. `None` when this VMK does not unlock that volume.
    pub recovery_key: Option<String>,
    /// True when this exact VMK has already failed an unlock attempt on
    /// this `(image_id, partition_id)` in the current session — surfaced
    /// to the UI as a "已尝试" pill so the operator does not re-click it.
    pub already_tried: bool,
}

#[derive(Debug, Serialize)]
pub struct VmkScanResult {
    pub partition_id: u8,
    pub candidates: Vec<VmkScanCandidate>,
}

/// Payload of the `bitlocker-scan-progress` backend event emitted while the
/// rayon scan walks the memory dump.
#[derive(Debug, Clone, Serialize)]
pub struct BitlockerScanProgress {
    pub task_id: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Serialize)]
pub struct BitlockerVmkUnlockResult {
    pub status: BitlockerStatus,
    pub recovery_key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BdeStoredKey {
    pub partition_id: u8,
    pub recovery_key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BdeAttemptsView {
    pub password_hashes: Vec<String>,
    pub recovery_hashes: Vec<String>,
    pub vmk_hashes: Vec<String>,
}

/// Per-partition record of every previously-failed unlock attempt. Stored
/// SHA-1 hashes (20 bytes each) are sufficient to recognise repeat inputs
/// without ever retaining plaintext credentials. Materialised on demand
/// from `CaseDb::bitlocker_attempts_for_partition`.
#[derive(Default, Debug, Clone)]
pub struct BdeAttempts {
    pub failed_password_hashes: HashSet<[u8; 20]>,
    pub failed_recovery_hashes: HashSet<[u8; 20]>,
    pub failed_vmk_hashes: HashSet<[u8; 20]>,
}

/// Discriminator used by `CaseDb::bitlocker_attempts` to tag a SHA-1
/// fingerprint by which BitLocker input it came from. Exhaustive `match`
/// over this enum forces every storage call site to address all three
/// kinds.
#[derive(Debug, Clone, Copy)]
pub enum BdeAttemptKind {
    Password,
    Recovery,
    Vmk,
}

impl BdeAttemptKind {
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Recovery => "recovery",
            Self::Vmk => "vmk",
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "password" => Some(Self::Password),
            "recovery" => Some(Self::Recovery),
            "vmk" => Some(Self::Vmk),
            _ => None,
        }
    }
}

/// SHA-1 fingerprint of a normalised BitLocker input. The 20-byte hash is
/// the canonical key used by `CaseDb::bitlocker_attempts` so we never
/// retain plaintext passwords / VMKs in any persistent store.
pub fn hash_attempt(value: &str) -> [u8; 20] {
    let digest = Sha1::digest(value.as_bytes());
    digest.into()
}

/// Normalise a BitLocker recovery password so equivalent user inputs
/// (with or without dashes / whitespace) hash to the same fingerprint.
/// Recovery keys are 48 digits regardless of presentation, so we keep
/// digits only — dashed and undashed pastes of the same key collide
/// intentionally.
pub fn normalize_recovery(value: &str) -> String {
    value.chars().filter(|ch| ch.is_ascii_digit()).collect()
}

/// Normalise a VMK hex string (lowercase, whitespace removed).
pub fn normalize_vmk_hex(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_datum(type_flag: u8, key_type: u8, key: [u8; 32]) -> [u8; DATUM_SIZE] {
        let mut datum = [0u8; DATUM_SIZE];
        datum[0] = 0x2C;
        datum[2] = type_flag;
        datum[4] = 0x01;
        datum[8] = key_type;
        datum[9] = 0x20;
        datum[12..44].copy_from_slice(&key);
        datum
    }

    #[test]
    fn finds_single_datum_at_offset() {
        let datum = make_datum(0x05, 0x20, [0xAB; 32]);
        let mut buffer = vec![0u8; 4096];
        let position = 1337;
        buffer[position..position + DATUM_SIZE].copy_from_slice(&datum);
        let hits = scan_chunk(&buffer, 0);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].offset, position as u64);
        assert_eq!(hits[0].type_flag, 0x05);
        assert_eq!(hits[0].key_type, 0x20);
        assert_eq!(hits[0].vmk_hex, hex::encode([0xAB; 32]));
    }

    #[test]
    fn rejects_low_entropy_key() {
        let mut key = [0x42u8; 32];
        for byte in &mut key[0..4] {
            *byte = 0;
        }
        let datum = make_datum(0x05, 0x20, key);
        let mut buffer = vec![0u8; 1024];
        buffer[100..100 + DATUM_SIZE].copy_from_slice(&datum);
        assert!(scan_chunk(&buffer, 0).is_empty());
    }

    #[test]
    fn rejects_bad_fixed_bytes() {
        let mut datum = make_datum(0x05, 0x20, [0xCD; 32]);
        datum[4] = 0x02;
        let mut buffer = vec![0u8; 1024];
        buffer[200..200 + DATUM_SIZE].copy_from_slice(&datum);
        assert!(scan_chunk(&buffer, 0).is_empty());
    }

    #[test]
    fn recovery_key_format_matches_efdd() {
        let key = [
            0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x04, 0x00, 0x05, 0x00, 0x06, 0x00, 0x07, 0x00,
            0x08, 0x00,
        ];
        let expected = "000011-000022-000033-000044-000055-000066-000077-000088".to_string();
        assert_eq!(format_recovery_key(&key), expected);
    }

    #[test]
    fn aes_ctr_roundtrip() {
        let key = [0x11u8; 16];
        let counter_base = [0x22u8; 12];
        let plaintext = b"bitlocker-vmk-ctr-roundtrip-test-vector".to_vec();
        let ciphertext = aes_ctr_decrypt(&key, &counter_base, &plaintext).expect("encrypt");
        let roundtrip = aes_ctr_decrypt(&key, &counter_base, &ciphertext).expect("decrypt");
        assert_eq!(roundtrip, plaintext);
    }

    #[test]
    fn parse_vmk_hex_accepts_64_chars() {
        let hex = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let bytes = parse_vmk_hex(hex).expect("32 bytes");
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn parse_vmk_hex_strips_whitespace() {
        let hex = "0011 2233 4455 6677 8899 aabb ccdd eeff 0011 2233 4455 6677 8899 aabb ccdd eeff";
        assert_eq!(parse_vmk_hex(hex).expect("32 bytes").len(), 32);
    }

    #[test]
    fn parse_vmk_hex_rejects_wrong_length() {
        assert!(parse_vmk_hex("deadbeef").is_err());
    }

    #[test]
    fn recovery_normalisation_collapses_whitespace_and_dashes() {
        // All three presentations of the same 48-digit recovery key must
        // produce the same fingerprint so a re-typed-with-different-spacing
        // duplicate hits the failure cache.
        let with_dashes =
            normalize_recovery("123456-234567-345678-456789-567890-678901-789012-890123");
        let with_spaces =
            normalize_recovery(" 123456-234567 345678 456789-567890-678901-789012-890123 ");
        let without_dashes = normalize_recovery("123456234567345678456789567890678901789012890123");
        assert_eq!(hash_attempt(&with_dashes), hash_attempt(&with_spaces));
        assert_eq!(hash_attempt(&with_dashes), hash_attempt(&without_dashes));
    }

    #[test]
    fn vmk_hex_normalisation_is_case_and_whitespace_insensitive() {
        let lower_packed = normalize_vmk_hex("4a564b20bbb766ff35df3f48aea7bee8");
        let mixed_spaced = normalize_vmk_hex("4A56 4B20 BBb7 66FF 35Df 3F48 aEa7 bEe8");
        let tab_separated = normalize_vmk_hex("4A56\t4B20\tBBB7\t66FF\t35DF\t3F48\tAEA7\tBEE8");
        assert_eq!(hash_attempt(&lower_packed), hash_attempt(&mixed_spaced));
        assert_eq!(hash_attempt(&lower_packed), hash_attempt(&tab_separated));
    }
}
