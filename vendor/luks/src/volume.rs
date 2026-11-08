//! Public API: parse a LUKS1 or LUKS2 container and unlock it from a passphrase.
//!
//! [`LuksVolume::unlock_with_passphrase`] auto-detects the version. Both run the
//! same shape — parse the header, try each keyslot (KDF → decrypt AF material →
//! AF-merge → master key), verify the master key against the digest, and return a
//! [`DecryptedPayload`] that decrypts the data segment on demand.

use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

use crate::af;
use crate::crypto::{
    derive_key_argon2_within, derive_key_within, Argon2Params, DERIVATION_BUDGET, UNLOCK_BUDGET,
};
use crate::error::{LuksError, Result};
use crate::header::{Luks1Header, LUKS1_PHDR_LEN, MK_DIGEST_LEN, SECTOR};
use crate::header2::{Luks2Header, Luks2Kdf};
mod layout;
use crate::sector_cipher::{validate_cipher, SectorCipher};
pub(crate) use layout::{payload1, payload2, read_header2};
use zeroize::Zeroizing;

/// Namespace for opening a LUKS volume. State lives in [`DecryptedPayload`].
pub struct LuksVolume;

/// The cipher/version facts of an unlocked volume.
#[derive(Debug, Clone)]
pub struct VolumeInfo {
    /// Volume identifier from the on-disk header.
    pub uuid: String,
    /// LUKS version (1 or 2).
    pub version: u16,
    /// Cipher name, e.g. `aes`.
    pub cipher_name: String,
    /// Cipher mode fed to the sector cipher, e.g. `xts-plain64`.
    pub cipher_mode: String,
    /// Master-key length in bytes.
    pub key_bytes: u32,
    /// Encryption sector size in bytes (512 or 4096).
    pub sector_size: usize,
}

/// Split a LUKS2 `encryption` string (`aes-xts-plain64`) into (name, mode). For a
/// LUKS1 header the mode is already separate, so `cipher_mode_of` is the identity.
pub(crate) fn split_encryption(enc: &str) -> (String, String) {
    match enc.split_once('-') {
        Some((name, mode)) => (name.to_string(), mode.to_string()),
        None => (enc.to_string(), String::new()),
    }
}

impl LuksVolume {
    /// Unlock a LUKS1 or LUKS2 container, auto-detecting the version from the
    /// header.
    ///
    /// # Errors
    /// See [`Self::unlock1_with_passphrase`] / [`Self::unlock2_with_passphrase`].
    pub fn unlock_with_passphrase<R: Read + Seek>(
        reader: R,
        passphrase: &[u8],
    ) -> Result<DecryptedPayload<R>> {
        Self::unlock_with_passphrase_within(reader, passphrase, UNLOCK_BUDGET)
    }

    /// [`Self::unlock_with_passphrase`] against an explicit TOTAL budget.
    ///
    /// The auto-detecting entry point carries the budget too, because the
    /// callers that do not know a container's version up front — the VFS layer,
    /// the fuzz target, a triage sweep over an image — are exactly the ones that
    /// must not silently lose the bound at the version dispatch.
    ///
    /// # Errors
    /// [`LuksError::UnlockBudgetExceeded`] when the total runs out, plus
    /// everything [`Self::unlock_with_passphrase`] returns.
    pub fn unlock_with_passphrase_within<R: Read + Seek>(
        mut reader: R,
        passphrase: &[u8],
        budget: Duration,
    ) -> Result<DecryptedPayload<R>> {
        let mut ver = [0u8; 8];
        reader.seek(SeekFrom::Start(0))?;
        read_fill(&mut reader, &mut ver)?;
        reader.seek(SeekFrom::Start(0))?;
        match crate::bytes::be_u16(&ver, 6) {
            1 => Self::unlock1_with_passphrase_within(reader, passphrase, budget),
            2 => Self::unlock2_with_passphrase_within(reader, passphrase, budget),
            other => {
                if ver.starts_with(&crate::header::LUKS_MAGIC) {
                    Err(LuksError::UnsupportedVersion { version: other })
                } else {
                    Err(LuksError::NotLuks {
                        found: crate::bytes::bytes_n::<6>(&ver, 0),
                    })
                }
            }
        }
    }

    /// [`Self::unlock1_with_passphrase`] against an explicit TOTAL budget.
    ///
    /// The budget spans every keyslot the unlock tries, not each derivation. A
    /// forensic caller working through a large image may want a tighter bound
    /// than the default; a crafted header cannot exceed whichever is given.
    ///
    /// # Errors
    /// [`LuksError::UnlockBudgetExceeded`] when the total runs out, plus
    /// everything [`Self::unlock1_with_passphrase`] returns.
    pub fn unlock1_with_passphrase_within<R: Read + Seek>(
        reader: R,
        passphrase: &[u8],
        budget: Duration,
    ) -> Result<DecryptedPayload<R>> {
        Self::unlock1_inner(reader, passphrase, UnlockDeadline::new(budget))
    }

    /// Unlock a LUKS1 container from `reader` with `passphrase`.
    ///
    /// # Errors
    /// Header-parse errors, [`LuksError::NoActiveKeyslot`], or
    /// [`LuksError::AuthenticationFailed`] on a wrong passphrase.
    pub fn unlock1_with_passphrase<R: Read + Seek>(
        reader: R,
        passphrase: &[u8],
    ) -> Result<DecryptedPayload<R>> {
        Self::unlock1_inner(reader, passphrase, UnlockDeadline::new(UNLOCK_BUDGET))
    }

    fn unlock1_inner<R: Read + Seek>(
        mut reader: R,
        passphrase: &[u8],
        deadline: UnlockDeadline,
    ) -> Result<DecryptedPayload<R>> {
        let mut hdr_buf = vec![0u8; LUKS1_PHDR_LEN];
        reader.seek(SeekFrom::Start(0))?;
        read_fill(&mut reader, &mut hdr_buf)?;
        let header = Luks1Header::parse(&hdr_buf)?;
        validate_cipher(
            &header.cipher_name,
            &header.cipher_mode,
            header.key_bytes as usize,
        )?;
        if header.active_keyslots().next().is_none() {
            return Err(LuksError::NoActiveKeyslot);
        }
        let key_bytes = header.key_bytes as usize;
        let master_key =
            recover_master_key1(&mut reader, &header, passphrase, key_bytes, deadline)?;
        payload1(reader, &header, master_key)
    }

    /// [`Self::unlock2_with_passphrase`] against an explicit TOTAL budget.
    ///
    /// Same bound as the LUKS1 twin, and needed for the same reason:
    /// `recover_master_key2` derives once per keyslot plus once per digest that
    /// references it, over a slot count and per-slot cost the header chooses.
    ///
    /// # Errors
    /// [`LuksError::UnlockBudgetExceeded`] when the total runs out, plus
    /// everything [`Self::unlock2_with_passphrase`] returns.
    pub fn unlock2_with_passphrase_within<R: Read + Seek>(
        reader: R,
        passphrase: &[u8],
        budget: Duration,
    ) -> Result<DecryptedPayload<R>> {
        Self::unlock2_inner(reader, passphrase, UnlockDeadline::new(budget))
    }

    /// Unlock a LUKS2 container from `reader` with `passphrase`.
    ///
    /// # Errors
    /// Header/JSON-parse errors, [`LuksError::NoActiveKeyslot`] if there is no
    /// crypt segment or keyslot, or [`LuksError::AuthenticationFailed`].
    pub fn unlock2_with_passphrase<R: Read + Seek>(
        reader: R,
        passphrase: &[u8],
    ) -> Result<DecryptedPayload<R>> {
        Self::unlock2_inner(reader, passphrase, UnlockDeadline::new(UNLOCK_BUDGET))
    }

    fn unlock2_inner<R: Read + Seek>(
        mut reader: R,
        passphrase: &[u8],
        deadline: UnlockDeadline,
    ) -> Result<DecryptedPayload<R>> {
        let (header, uuid) = read_header2(&mut reader)?;

        let segment = header
            .crypt_segment()
            .cloned()
            .ok_or(LuksError::NoActiveKeyslot)?;
        if header.keyslots.is_empty() {
            return Err(LuksError::NoActiveKeyslot);
        }

        let (cipher_name, cipher_mode) = split_encryption(&segment.encryption);
        for slot in &header.keyslots {
            validate_cipher(&cipher_name, &cipher_mode, slot.key_size)?;
        }
        let master_key = recover_master_key2(&mut reader, &header, passphrase, deadline)?;
        payload2(reader, &segment, uuid, master_key)
    }
}

/// Remaining allowance in an unlock's total budget.
///
/// Bounds the WHOLE unlock rather than each derivation. `recover_master_key*`
/// derives twice per active keyslot over up to 8 attacker-chosen slots, so a
/// per-derivation budget leaves the sum unbounded — see
/// [`crypto::UNLOCK_BUDGET`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnlockDeadline {
    started: Instant,
    total: Duration,
}

impl UnlockDeadline {
    pub(crate) fn new(total: Duration) -> Self {
        Self {
            started: Instant::now(),
            total,
        }
    }

    /// Time left, or the error naming how far the unlock got before running out.
    ///
    /// Clamped to [`crypto::DERIVATION_BUDGET`], so the aggregate bound does not
    /// quietly widen the per-derivation one it was added alongside: without the
    /// clamp a single slot could spend the entire [`crypto::UNLOCK_BUDGET`],
    /// which is three times what one derivation was ever allowed. Both ceilings
    /// hold together — no derivation over 30s, no unlock over 90s.
    pub(crate) fn remaining(self, slots_tried: usize) -> Result<Duration> {
        self.total
            .checked_sub(self.started.elapsed())
            .filter(|left| !left.is_zero())
            .map(|left| left.min(DERIVATION_BUDGET))
            .ok_or(LuksError::UnlockBudgetExceeded {
                slots_tried,
                budget_secs: self.total.as_secs(),
            })
    }
}

/// LUKS1: try each active keyslot until one yields a master key whose digest
/// matches.
fn recover_master_key1<R: Read + Seek>(
    reader: &mut R,
    header: &Luks1Header,
    passphrase: &[u8],
    key_bytes: usize,
    deadline: UnlockDeadline,
) -> Result<Vec<u8>> {
    for (tried, slot) in header.active_keyslots().enumerate() {
        let slot_key = Zeroizing::new(derive_key_within(
            &header.hash_spec,
            passphrase,
            &slot.salt,
            slot.iterations,
            key_bytes,
            deadline.remaining(tried)?,
        )?);
        let material_len = af::material_len(key_bytes, slot.stripes as usize)?;
        let mut material = Zeroizing::new(vec![0u8; material_len]);
        reader.seek(SeekFrom::Start(
            u64::from(slot.key_material_offset) * SECTOR,
        ))?;
        if read_available(reader, &mut material)? < material_len {
            continue;
        }
        SectorCipher::new(&header.cipher_name, &header.cipher_mode, &slot_key)?.decrypt_area(
            &mut material,
            512,
            0,
        )?;
        let candidate = Zeroizing::new(af::merge(
            &header.hash_spec,
            &material,
            key_bytes,
            slot.stripes as usize,
        )?); // hash was validated by the slot-key KDF
        let digest = derive_key_within(
            &header.hash_spec,
            &candidate,
            &header.mk_digest_salt,
            header.mk_digest_iter,
            MK_DIGEST_LEN,
            deadline.remaining(tried)?,
        )?; // cov:unreachable: hash_spec already validated by the slot-key derive_key above
        if digest == header.mk_digest {
            return Ok(candidate.to_vec());
        }
    }
    Err(LuksError::AuthenticationFailed)
}

/// LUKS2: try each keyslot (Argon2 or PBKDF2 KDF) and verify against a digest
/// that references it.
///
/// `deadline` bounds the loop as a whole. Each iteration re-reads what is left
/// before committing to any derivation, so a header cannot multiply a
/// per-derivation budget by its own slot count. For the Argon2 branch that is
/// the *only* bound on a slot that is already running — see
/// [`crypto::derive_key_argon2_within`] — but it is what stops the next slot
/// from starting, which is the multiplication this exists to prevent.
fn recover_master_key2<R: Read + Seek>(
    reader: &mut R,
    header: &Luks2Header,
    passphrase: &[u8],
    deadline: UnlockDeadline,
) -> Result<Vec<u8>> {
    for (tried, slot) in header.keyslots.iter().enumerate() {
        let slot_key = Zeroizing::new(match &slot.kdf {
            Luks2Kdf::Argon2 {
                kind,
                time,
                memory,
                cpus,
                salt,
            } => derive_key_argon2_within(
                &Argon2Params {
                    kind,
                    time: *time,
                    memory: *memory,
                    cpus: *cpus,
                    salt,
                },
                passphrase,
                slot.area_key_size,
                deadline.remaining(tried)?,
            )?,
            Luks2Kdf::Pbkdf2 {
                hash,
                iterations,
                salt,
            } => derive_key_within(
                hash,
                passphrase,
                salt,
                *iterations,
                slot.area_key_size,
                deadline.remaining(tried)?,
            )?,
        });

        let (area_name, area_mode) = split_encryption(&slot.area_encryption);
        let material_len = af::material_len(slot.key_size, slot.af_stripes as usize)?;
        let mut material = Zeroizing::new(vec![0u8; material_len]);
        reader.seek(SeekFrom::Start(slot.area_offset))?;
        if read_available(reader, &mut material)? < material_len {
            continue;
        }
        SectorCipher::new(&area_name, &area_mode, &slot_key)?.decrypt_area(
            &mut material,
            512,
            0,
        )?;
        let candidate = Zeroizing::new(af::merge(
            &slot.af_hash,
            &material,
            slot.key_size,
            slot.af_stripes as usize,
        )?);

        for dig in &header.digests {
            if dig.kind == "pbkdf2" && dig.keyslots.iter().any(|k| k == &slot.id) {
                let d = derive_key_within(
                    &dig.hash,
                    &candidate,
                    &dig.salt,
                    dig.iterations,
                    dig.digest.len(),
                    deadline.remaining(tried)?,
                )?;
                if !dig.digest.is_empty() && d == dig.digest {
                    return Ok(candidate.to_vec());
                }
            }
        }
    }
    Err(LuksError::AuthenticationFailed)
}

/// A plaintext view of an unlocked LUKS payload.
pub struct DecryptedPayload<R> {
    reader: R,
    master_key: Zeroizing<Vec<u8>>,
    cipher: SectorCipher,
    payload_offset: u64,
    sector_size: usize,
    iv_tweak: u128,
    total_size: u64,
    position: u64,
    info: VolumeInfo,
}

impl<R: Read + Seek> DecryptedPayload<R> {
    /// Cipher/version facts of the volume.
    #[must_use]
    pub fn info(&self) -> &VolumeInfo {
        &self.info
    }

    /// The recovered master key (sensitive).
    #[must_use]
    pub fn master_key(&self) -> &[u8] {
        &self.master_key
    }

    /// Size of the encrypted payload in bytes.
    #[must_use]
    pub fn payload_size(&self) -> u64 {
        self.total_size.saturating_sub(self.payload_offset)
    }

    /// Read decrypted payload bytes at payload-relative `offset` into `buf`,
    /// filling it completely; reads beyond the payload fail.
    ///
    /// # Errors
    /// Propagates I/O errors and any cipher error.
    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(buf.len() as u64)
            .map_or(true, |end| end > self.payload_size())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "read exceeds decrypted data area",
            )
            .into());
        }
        let ss = self.sector_size as u64;
        let step = (self.sector_size / 512).max(1) as u128;
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let unit = pos / ss;
            let within = (pos % ss) as usize;
            let physical = self.payload_offset + unit * ss;

            let mut ct = vec![0u8; self.sector_size];
            self.reader.seek(SeekFrom::Start(physical))?;
            self.reader.read_exact(&mut ct)?;
            let tweak = self
                .iv_tweak
                .checked_add(u128::from(unit) * step)
                .ok_or_else(|| std::io::Error::other("LUKS IV overflow"))?;
            self.cipher.decrypt_area(&mut ct, self.sector_size, tweak)?;

            let take = (self.sector_size - within).min(buf.len() - done);
            buf[done..done + take].copy_from_slice(&ct[within..within + take]);
            done += take;
        }
        Ok(())
    }
}

impl<R: Read + Seek> Read for DecryptedPayload<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let size = self.payload_size();
        if self.position >= size {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(size - self.position) as usize;
        self.read_at(self.position, &mut buf[..n])
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.position += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for DecryptedPayload<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let size = self.payload_size();
        let new = match pos {
            SeekFrom::Start(o) => i128::from(o),
            SeekFrom::End(o) => i128::from(size) + i128::from(o),
            SeekFrom::Current(o) => i128::from(self.position) + i128::from(o),
        };
        if new < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.position =
            u64::try_from(new).map_err(|_| std::io::Error::other("LUKS seek overflow"))?;
        Ok(self.position)
    }
}

/// Read exactly `buf.len()` bytes, erroring on premature EOF.
fn read_fill<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<()> {
    reader.read_exact(buf)?;
    Ok(())
}

/// Read up to `buf.len()` bytes, zero-filling the remainder on EOF. Returns the
/// number of real bytes read.
fn read_available<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    for b in &mut buf[filled..] {
        *b = 0;
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    //! Hermetic round-trip tests: build a synthetic LUKS container in memory with
    //! the same audited primitives, unlock it, and assert the plaintext comes
    //! back. These are Tier-3 self-consistency scaffolding — the real correctness
    //! proof is the `cryptsetup` oracle (`tests/oracle_luks{1,2}.rs`).

    use std::time::Duration;

    use super::*;
    use crate::af;
    use crate::crypto::{derive_key_argon2_within, derive_key_within, Argon2Params};
    use crate::header::{
        KEYSLOT_LEN, KEY_DISABLED, KEY_ENABLED, LUKS1_PHDR_LEN, LUKS_MAGIC, LUKS_NUM_KEYS,
    };
    use crate::header2::LUKS2_BIN_HDR_LEN;
    use aes::cipher::KeyInit;
    use aes::Aes256;
    use base64::Engine;
    use std::io::Cursor;
    use xts_mode::Xts128;

    const PASS: &[u8] = b"luks-TEST";

    /// XTS-encrypt `buf` in place, one `unit_size`-byte unit at a time, with the
    /// per-unit plain64 tweak `base + u * (unit_size/512)` — the encrypt inverse
    /// of `xts_decrypt_area` (64-byte AES-256 key only, which is all the fixtures
    /// use).
    fn xts_encrypt(key: &[u8], buf: &mut [u8], unit_size: usize, base: u128) {
        let step = (unit_size / 512).max(1) as u128;
        let (k1, k2) = key.split_at(32);
        let xts = Xts128::<Aes256>::new(Aes256::new(k1.into()), Aes256::new(k2.into()));
        for (u, chunk) in buf.chunks_mut(unit_size).enumerate() {
            let tweak = (base + u as u128 * step).to_le_bytes();
            xts.encrypt_sector(chunk, tweak);
        }
    }

    fn pbkdf2_sha256(pw: &[u8], salt: &[u8], iters: u32, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pw, salt, iters, &mut out);
        out
    }

    fn known_plain(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i as u8) ^ 0x3c).collect()
    }

    // ---- LUKS1 ------------------------------------------------------------

    const L1_KM_OFF: u32 = 2; // sector 2 = byte 1024 (past the 592-byte phdr)
    const L1_PAYLOAD_OFF: u32 = 4; // sector 4 = byte 2048
    const L1_STRIPES: u32 = 8;
    const L1_KEY_BYTES: u32 = 64;
    const L1_ITERS: u32 = 5;
    const L1_MK_ITER: u32 = 7;

    /// Build a valid LUKS1 container (`aes` / `xts-plain64`, AES-256-XTS) unlocking
    /// to `master`, plus the 3-sector plaintext payload. Returns (image, plain).
    fn build_luks1(master: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let salt = [0x11u8; 32];
        let mk_salt = [0x22u8; 32];
        let slot_key = pbkdf2_sha256(PASS, &salt, L1_ITERS, L1_KEY_BYTES as usize);

        let mut material =
            af::af_split::<sha2::Sha256>(master, L1_KEY_BYTES as usize, L1_STRIPES as usize, 0x5a);
        xts_encrypt(&slot_key, &mut material, 512, 0);

        let mk_digest = pbkdf2_sha256(master, &mk_salt, L1_MK_ITER, 20);

        let mut payload = known_plain(3 * 512);
        let plain = payload.clone();
        xts_encrypt(master, &mut payload, 512, 0);

        let payload_byte = L1_PAYLOAD_OFF as usize * 512;
        let mut img = vec![0u8; payload_byte + payload.len()];
        img[0..6].copy_from_slice(&LUKS_MAGIC);
        img[6..8].copy_from_slice(&1u16.to_be_bytes());
        img[8..11].copy_from_slice(b"aes");
        img[40..51].copy_from_slice(b"xts-plain64");
        img[72..78].copy_from_slice(b"sha256");
        img[104..108].copy_from_slice(&L1_PAYLOAD_OFF.to_be_bytes());
        img[108..112].copy_from_slice(&L1_KEY_BYTES.to_be_bytes());
        img[112..132].copy_from_slice(&mk_digest);
        img[132..164].copy_from_slice(&mk_salt);
        img[164..168].copy_from_slice(&L1_MK_ITER.to_be_bytes());
        img[168..204].copy_from_slice(b"b22690e1-a392-4ecc-83b1-c1cf21200116");

        for i in 0..LUKS_NUM_KEYS {
            let base = 208 + i * KEYSLOT_LEN;
            if i == 0 {
                img[base..base + 4].copy_from_slice(&KEY_ENABLED.to_be_bytes());
                img[base + 4..base + 8].copy_from_slice(&L1_ITERS.to_be_bytes());
                img[base + 8..base + 40].copy_from_slice(&salt);
                img[base + 40..base + 44].copy_from_slice(&L1_KM_OFF.to_be_bytes());
                img[base + 44..base + 48].copy_from_slice(&L1_STRIPES.to_be_bytes());
            } else {
                img[base..base + 4].copy_from_slice(&KEY_DISABLED.to_be_bytes());
            }
        }

        let km_byte = L1_KM_OFF as usize * 512;
        img[km_byte..km_byte + material.len()].copy_from_slice(&material);
        img[payload_byte..payload_byte + payload.len()].copy_from_slice(&payload);
        assert!(
            LUKS1_PHDR_LEN <= km_byte,
            "phdr must not overlap key material"
        );
        (img, plain)
    }

    #[test]
    fn luks1_roundtrip_via_autodetect() {
        let master: Vec<u8> = (0..64u8)
            .map(|x| x.wrapping_mul(7).wrapping_add(3))
            .collect();
        let (img, plain) = build_luks1(&master);
        let mut vol = LuksVolume::unlock_with_passphrase(Cursor::new(img), PASS).unwrap();

        assert_eq!(vol.info().version, 1);
        assert_eq!(vol.info().cipher_name, "aes");
        assert_eq!(vol.info().cipher_mode, "xts-plain64");
        assert_eq!(vol.info().key_bytes, 64);
        assert_eq!(vol.info().sector_size, 512);
        assert_eq!(vol.master_key(), &master[..]);
        assert_eq!(vol.payload_size(), 3 * 512);

        for lba in 0..3u64 {
            let mut buf = [0u8; 512];
            vol.read_at(lba * 512, &mut buf).unwrap();
            assert_eq!(
                &buf[..],
                &plain[lba as usize * 512..lba as usize * 512 + 512]
            );
        }
    }

    #[test]
    fn luks1_read_seek_traits() {
        let master = vec![0xABu8; 64];
        let (img, plain) = build_luks1(&master);
        let mut vol = LuksVolume::unlock1_with_passphrase(Cursor::new(img), PASS).unwrap();

        // Seek variants.
        assert_eq!(vol.seek(SeekFrom::Start(512)).unwrap(), 512);
        assert_eq!(vol.seek(SeekFrom::Current(-256)).unwrap(), 256);
        let end = vol.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(end, 3 * 512);
        assert!(vol.seek(SeekFrom::End(-1)).is_ok());
        assert!(vol.seek(SeekFrom::Start(0)).is_ok());

        // Read the whole payload back through Read.
        let mut got = Vec::new();
        std::io::Read::read_to_end(&mut vol, &mut got).unwrap();
        assert_eq!(got, plain);
        // At EOF a further read yields 0.
        let mut z = [0u8; 16];
        assert_eq!(std::io::Read::read(&mut vol, &mut z).unwrap(), 0);
    }

    #[test]
    fn luks1_seek_before_start_errors() {
        let master = vec![0x01u8; 64];
        let (img, _) = build_luks1(&master);
        let mut vol = LuksVolume::unlock1_with_passphrase(Cursor::new(img), PASS).unwrap();
        assert!(vol.seek(SeekFrom::Start(0)).is_ok());
        assert!(vol.seek(SeekFrom::Current(-1)).is_err());
    }

    /// RED for the fuzz timeout (issue #10). `DERIVATION_BUDGET` bounds ONE
    /// derivation at 30s, but `recover_master_key1` calls `derive_key` twice per
    /// active keyslot and LUKS permits 8 of them — all attacker-controlled. Each
    /// call passes its own budget while the total is unbounded, which is how the
    /// `unlock` fuzz target reached a 1778-second timeout.
    ///
    /// The invariant: an unlock must be bounded as a WHOLE, not per derivation.
    /// Expressed with a zero budget so the test costs nothing to run — if any
    /// aggregate bound exists, no derivation may start.
    #[test]
    fn unlock_is_bounded_in_aggregate_not_just_per_derivation() {
        let master = [0x33u8; L1_KEY_BYTES as usize];
        let (img, _plain) = build_luks1(&master);

        // `DecryptedPayload` is deliberately not Debug (it holds key material),
        // so match rather than `expect_err`.
        match LuksVolume::unlock1_with_passphrase_within(Cursor::new(img), PASS, Duration::ZERO) {
            Err(LuksError::UnlockBudgetExceeded { .. }) => {}
            Err(other) => panic!("expected UnlockBudgetExceeded, got {other:?}"), // cov:unreachable: a zero budget cannot reach any other error
            Ok(_) => panic!("a zero total budget must refuse before doing derivation work"), // cov:unreachable: unreachable while the deadline is checked before the first derive
        }
    }

    #[test]
    fn luks1_wrong_passphrase() {
        let master = vec![0x5Cu8; 64];
        let (img, _) = build_luks1(&master);
        assert!(matches!(
            LuksVolume::unlock1_with_passphrase(Cursor::new(img), b"wrong"),
            Err(LuksError::AuthenticationFailed)
        ));
    }

    #[test]
    fn luks1_no_active_keyslot() {
        let master = vec![0x5Cu8; 64];
        let (mut img, _) = build_luks1(&master);
        // Disable keyslot 0.
        img[208..212].copy_from_slice(&KEY_DISABLED.to_be_bytes());
        assert!(matches!(
            LuksVolume::unlock1_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::NoActiveKeyslot)
        ));
    }

    #[test]
    fn luks1_short_key_material_is_skipped() {
        // Active keyslot whose key material lies past EOF: read_available returns
        // short, the slot is skipped, and unlock ends in AuthenticationFailed.
        let mut img = vec![0u8; LUKS1_PHDR_LEN];
        img[0..6].copy_from_slice(&LUKS_MAGIC);
        img[6..8].copy_from_slice(&1u16.to_be_bytes());
        img[8..11].copy_from_slice(b"aes");
        img[40..51].copy_from_slice(b"xts-plain64");
        img[72..78].copy_from_slice(b"sha256");
        img[104..108].copy_from_slice(&4u32.to_be_bytes());
        img[108..112].copy_from_slice(&64u32.to_be_bytes());
        let base = 208;
        img[base..base + 4].copy_from_slice(&KEY_ENABLED.to_be_bytes());
        img[base + 4..base + 8].copy_from_slice(&5u32.to_be_bytes());
        img[base + 40..base + 44].copy_from_slice(&1000u32.to_be_bytes()); // sector 1000 >> EOF
        img[base + 44..base + 48].copy_from_slice(&8u32.to_be_bytes());
        assert!(matches!(
            LuksVolume::unlock1_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::AuthenticationFailed)
        ));
    }

    // ---- version dispatch / bad headers -----------------------------------

    #[test]
    fn not_a_luks_container() {
        assert!(matches!(
            LuksVolume::unlock_with_passphrase(Cursor::new(vec![0u8; 8]), PASS),
            Err(LuksError::NotLuks { .. })
        ));
    }

    #[test]
    fn unsupported_version() {
        let mut img = vec![0u8; 8];
        img[0..6].copy_from_slice(&LUKS_MAGIC);
        img[6..8].copy_from_slice(&3u16.to_be_bytes());
        assert!(matches!(
            LuksVolume::unlock_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::UnsupportedVersion { version: 3 })
        ));
    }

    #[test]
    fn split_encryption_without_dash() {
        assert_eq!(split_encryption("aes"), ("aes".to_string(), String::new()));
        assert_eq!(
            split_encryption("aes-xts-plain64"),
            ("aes".to_string(), "xts-plain64".to_string())
        );
    }

    // ---- LUKS2 ------------------------------------------------------------

    const L2_AREA_OFF: u64 = 8192;
    const L2_SEG_OFF: u64 = 12288;
    const L2_STRIPES: usize = 8;
    const L2_DIG_ITER: u32 = 5;

    /// Assemble a LUKS2 container from a keyslot key + KDF JSON fragment, unlocking
    /// to `master`. Returns (image, plain payload).
    fn build_luks2(
        master: &[u8],
        slot_key: &[u8],
        kdf_json: &str,
        sector_size: usize,
    ) -> (Vec<u8>, Vec<u8>) {
        let b64 = base64::engine::general_purpose::STANDARD;
        let dig_salt = [0x22u8; 16];
        let digest = pbkdf2_sha256(master, &dig_salt, L2_DIG_ITER, 32);

        let mut material = af::af_split::<sha2::Sha256>(master, 64, L2_STRIPES, 0x5a);
        xts_encrypt(slot_key, &mut material, 512, 0);

        let mut payload = known_plain(sector_size); // one data unit
        let plain = payload.clone();
        xts_encrypt(master, &mut payload, sector_size, 0);

        let json = format!(
            concat!(
                "{{\"keyslots\":{{\"0\":{{\"key_size\":64,",
                "\"af\":{{\"type\":\"luks1\",\"stripes\":{stripes},\"hash\":\"sha256\"}},",
                "\"area\":{{\"type\":\"raw\",\"offset\":\"{area}\",\"size\":\"512\",\"encryption\":\"aes-xts-plain64\",\"key_size\":64}},",
                "\"kdf\":{kdf}}}}},",
                "\"segments\":{{\"0\":{{\"type\":\"crypt\",\"offset\":\"{seg}\",\"size\":\"dynamic\",\"iv_tweak\":\"0\",\"encryption\":\"aes-xts-plain64\",\"sector_size\":{ss}}}}},",
                "\"digests\":{{\"0\":{{\"type\":\"pbkdf2\",\"keyslots\":[\"0\"],\"segments\":[\"0\"],\"hash\":\"sha256\",\"iterations\":{diter},\"salt\":\"{dsalt}\",\"digest\":\"{dig}\"}}}}}}"
            ),
            stripes = L2_STRIPES,
            area = L2_AREA_OFF,
            kdf = kdf_json,
            seg = L2_SEG_OFF,
            ss = sector_size,
            diter = L2_DIG_ITER,
            dsalt = b64.encode(dig_salt),
            dig = b64.encode(&digest),
        );

        let hdr_size = LUKS2_BIN_HDR_LEN + 4096; // 8192
        assert!(json.len() < 4096);
        let total = L2_SEG_OFF as usize + payload.len();
        let mut img = vec![0u8; total];
        img[0..6].copy_from_slice(&LUKS_MAGIC);
        img[6..8].copy_from_slice(&2u16.to_be_bytes());
        img[8..16].copy_from_slice(&(hdr_size as u64).to_be_bytes());
        img[LUKS2_BIN_HDR_LEN..LUKS2_BIN_HDR_LEN + json.len()].copy_from_slice(json.as_bytes());
        let a = L2_AREA_OFF as usize;
        img[a..a + material.len()].copy_from_slice(&material);
        let s = L2_SEG_OFF as usize;
        img[s..s + payload.len()].copy_from_slice(&payload);
        (img, plain)
    }

    #[test]
    fn luks2_argon2id_roundtrip() {
        let master = vec![0xC3u8; 64];
        let salt = [0x77u8; 16];
        let slot_key = derive_key_argon2_within(
            &Argon2Params {
                kind: "argon2id",
                time: 1,
                memory: 32,
                cpus: 1,
                salt: &salt,
            },
            PASS,
            64,
            DERIVATION_BUDGET,
        )
        .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let kdf = format!(
            "{{\"type\":\"argon2id\",\"time\":1,\"memory\":32,\"cpus\":1,\"salt\":\"{}\"}}",
            b64.encode(salt)
        );
        let (img, plain) = build_luks2(&master, &slot_key, &kdf, 4096);

        let mut vol = LuksVolume::unlock_with_passphrase(Cursor::new(img), PASS).unwrap();
        assert_eq!(vol.info().version, 2);
        assert_eq!(vol.info().cipher_name, "aes");
        assert_eq!(vol.info().cipher_mode, "xts-plain64");
        assert_eq!(vol.info().sector_size, 4096);
        assert_eq!(vol.info().key_bytes, 64);
        assert_eq!(vol.master_key(), &master[..]);

        let mut buf = [0u8; 512];
        vol.read_at(0, &mut buf).unwrap();
        assert_eq!(&buf[..], &plain[..512]);
    }

    #[test]
    fn luks2_pbkdf2_roundtrip() {
        let master = vec![0x1Eu8; 64];
        let salt = [0x88u8; 16];
        let slot_key = derive_key_within("sha256", PASS, &salt, 5, 64, DERIVATION_BUDGET).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let kdf = format!(
            "{{\"type\":\"pbkdf2\",\"hash\":\"sha256\",\"iterations\":5,\"salt\":\"{}\"}}",
            b64.encode(salt)
        );
        let (img, plain) = build_luks2(&master, &slot_key, &kdf, 512);

        let mut vol = LuksVolume::unlock2_with_passphrase(Cursor::new(img), PASS).unwrap();
        assert_eq!(vol.info().sector_size, 512);
        let mut buf = [0u8; 512];
        vol.read_at(0, &mut buf).unwrap();
        assert_eq!(&buf[..], &plain[..512]);
    }

    /// RED for the LUKS2 half of issue #10. `recover_master_key1` is now bounded
    /// in aggregate; `recover_master_key2` is not. It derives once per keyslot
    /// (Argon2 or PBKDF2) and once more per digest that references the slot,
    /// each against its own fresh per-derivation budget — so the same
    /// multiplication that took the `unlock` fuzz target to 1778 seconds on
    /// LUKS1 applies here unchanged, on a header that chooses both the slot
    /// count and each slot's cost.
    ///
    /// Expressed with a zero budget, like the LUKS1 twin: if any aggregate bound
    /// exists then no derivation may start, so the test costs nothing to run.
    #[test]
    fn luks2_pbkdf2_unlock_is_bounded_in_aggregate_not_just_per_derivation() {
        let master = vec![0x44u8; 64];
        let salt = [0x88u8; 16];
        let slot_key = derive_key_within("sha256", PASS, &salt, 5, 64, DERIVATION_BUDGET).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let kdf = format!(
            "{{\"type\":\"pbkdf2\",\"hash\":\"sha256\",\"iterations\":5,\"salt\":\"{}\"}}",
            b64.encode(salt)
        );
        let (img, _plain) = build_luks2(&master, &slot_key, &kdf, 512);

        // `DecryptedPayload` is deliberately not Debug (it holds key material),
        // so match rather than `expect_err`.
        match LuksVolume::unlock2_with_passphrase_within(Cursor::new(img), PASS, Duration::ZERO) {
            Err(LuksError::UnlockBudgetExceeded { slots_tried, .. }) => assert_eq!(slots_tried, 0),
            Err(other) => panic!("expected UnlockBudgetExceeded, got {other:?}"), // cov:unreachable: a zero budget cannot reach any other error
            Ok(_) => panic!("a zero total budget must refuse before doing derivation work"), // cov:unreachable: unreachable while the deadline is checked before the first derive
        }
    }

    /// The Argon2 keyslot branch needs the same aggregate bound as the PBKDF2
    /// one. Argon2's own time projection is per-derivation, so without the
    /// deadline a header full of Argon2 slots multiplies exactly as PBKDF2 ones
    /// do — and the memory cap does nothing about that, it bounds the other axis.
    #[test]
    fn luks2_argon2_unlock_is_bounded_in_aggregate_too() {
        let master = vec![0xC3u8; 64];
        let salt = [0x77u8; 16];
        let slot_key = derive_key_argon2_within(
            &Argon2Params {
                kind: "argon2id",
                time: 1,
                memory: 32,
                cpus: 1,
                salt: &salt,
            },
            PASS,
            64,
            DERIVATION_BUDGET,
        )
        .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let kdf = format!(
            "{{\"type\":\"argon2id\",\"time\":1,\"memory\":32,\"cpus\":1,\"salt\":\"{}\"}}",
            b64.encode(salt)
        );
        let (img, _plain) = build_luks2(&master, &slot_key, &kdf, 512);

        match LuksVolume::unlock2_with_passphrase_within(Cursor::new(img), PASS, Duration::ZERO) {
            Err(LuksError::UnlockBudgetExceeded { slots_tried, .. }) => assert_eq!(slots_tried, 0),
            Err(other) => panic!("expected UnlockBudgetExceeded, got {other:?}"), // cov:unreachable: a zero budget cannot reach any other error
            Ok(_) => panic!("a zero total budget must refuse before running Argon2"), // cov:unreachable: unreachable while the deadline is checked before the first derive
        }
    }

    /// The auto-detecting entry point must carry the budget too, or a caller
    /// that does not know the container's version silently loses the bound —
    /// which is the whole population the fuzz target and the VFS layer come
    /// from. Both versions are asserted, since the dispatch is where a budget
    /// gets dropped.
    #[test]
    fn autodetect_unlock_within_bounds_both_versions() {
        let (img1, _) = build_luks1(&[0x33u8; L1_KEY_BYTES as usize]);
        match LuksVolume::unlock_with_passphrase_within(Cursor::new(img1), PASS, Duration::ZERO) {
            Err(LuksError::UnlockBudgetExceeded { .. }) => {}
            Err(other) => panic!("LUKS1: expected UnlockBudgetExceeded, got {other:?}"), // cov:unreachable: a zero budget cannot reach any other error
            Ok(_) => panic!("LUKS1: a zero total budget must refuse"), // cov:unreachable: unreachable while the deadline is checked before the first derive
        }

        let salt = [0x88u8; 16];
        let slot_key = derive_key_within("sha256", PASS, &salt, 5, 64, DERIVATION_BUDGET).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let kdf = format!(
            "{{\"type\":\"pbkdf2\",\"hash\":\"sha256\",\"iterations\":5,\"salt\":\"{}\"}}",
            b64.encode(salt)
        );
        let (img2, _) = build_luks2(&[0x44u8; 64], &slot_key, &kdf, 512);
        match LuksVolume::unlock_with_passphrase_within(Cursor::new(img2), PASS, Duration::ZERO) {
            Err(LuksError::UnlockBudgetExceeded { .. }) => {}
            Err(other) => panic!("LUKS2: expected UnlockBudgetExceeded, got {other:?}"), // cov:unreachable: a zero budget cannot reach any other error
            Ok(_) => panic!("LUKS2: a zero total budget must refuse"), // cov:unreachable: unreachable while the deadline is checked before the first derive
        }
    }

    #[test]
    fn luks2_no_crypt_segment() {
        let json = r#"{"keyslots":{"0":{"key_size":64,
          "af":{"stripes":8,"hash":"sha256"},
          "area":{"offset":"8192","size":"512","encryption":"aes-xts-plain64"},
          "kdf":{"type":"pbkdf2","hash":"sha256","iterations":5,"salt":"AAAA"}}},
          "segments":{},"digests":{}}"#;
        let img = build_luks2_json(json);
        assert!(matches!(
            LuksVolume::unlock2_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::NoActiveKeyslot)
        ));
    }

    #[test]
    fn luks2_no_keyslots() {
        let json = r#"{"keyslots":{},
          "segments":{"0":{"type":"crypt","offset":"12288","encryption":"aes-xts-plain64","sector_size":512}},
          "digests":{}}"#;
        let img = build_luks2_json(json);
        assert!(matches!(
            LuksVolume::unlock2_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::NoActiveKeyslot)
        ));
    }

    #[test]
    fn luks2_short_key_material_is_skipped() {
        // Keyslot area past EOF: material read is short, slot skipped -> auth fail.
        let json = r#"{"keyslots":{"0":{"key_size":64,
          "af":{"stripes":8,"hash":"sha256"},
          "area":{"offset":"9999999","size":"512","encryption":"aes-xts-plain64"},
          "kdf":{"type":"pbkdf2","hash":"sha256","iterations":5,"salt":"AAAA"}}},
          "segments":{"0":{"type":"crypt","offset":"12288","encryption":"aes-xts-plain64","sector_size":512}},
          "digests":{"0":{"type":"pbkdf2","keyslots":["0"],"hash":"sha256","iterations":5,"salt":"AAAA","digest":"AAAA"}}}"#;
        let img = build_luks2_json(json);
        assert!(matches!(
            LuksVolume::unlock2_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::AuthenticationFailed)
        ));
    }

    /// Minimal LUKS2 image carrying an arbitrary JSON metadata document (no valid
    /// key material) — for the header-level error-path tests above.
    fn build_luks2_json(json: &str) -> Vec<u8> {
        let hdr_size = LUKS2_BIN_HDR_LEN + 4096;
        let total = 16384usize;
        let mut img = vec![0u8; total];
        img[0..6].copy_from_slice(&LUKS_MAGIC);
        img[6..8].copy_from_slice(&2u16.to_be_bytes());
        img[8..16].copy_from_slice(&(hdr_size as u64).to_be_bytes());
        img[LUKS2_BIN_HDR_LEN..LUKS2_BIN_HDR_LEN + json.len()].copy_from_slice(json.as_bytes());
        img
    }

    // ---- reachable KDF/hash error propagation -----------------------------

    #[test]
    fn luks1_unsupported_hash_errors() {
        // An unsupported hash spec makes the very first slot-key derive_key fail,
        // and unlock surfaces a loud Unsupported error (fail-loud, never panic).
        let mut img = vec![0u8; 4096];
        img[0..6].copy_from_slice(&LUKS_MAGIC);
        img[6..8].copy_from_slice(&1u16.to_be_bytes());
        img[8..11].copy_from_slice(b"aes");
        img[40..51].copy_from_slice(b"xts-plain64");
        img[72..75].copy_from_slice(b"md5");
        img[104..108].copy_from_slice(&4u32.to_be_bytes());
        img[108..112].copy_from_slice(&64u32.to_be_bytes());
        let base = 208;
        img[base..base + 4].copy_from_slice(&KEY_ENABLED.to_be_bytes());
        img[base + 4..base + 8].copy_from_slice(&5u32.to_be_bytes());
        img[base + 40..base + 44].copy_from_slice(&1u32.to_be_bytes());
        img[base + 44..base + 48].copy_from_slice(&8u32.to_be_bytes());
        assert!(matches!(
            LuksVolume::unlock1_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::Unsupported { what: "hash", .. })
        ));
    }

    #[test]
    fn luks2_bad_argon2_params_errors() {
        // memory cost 0 fails Argon2's Params::new inside recover_master_key2.
        let json = r#"{"keyslots":{"0":{"key_size":64,
          "af":{"stripes":8,"hash":"sha256"},
          "area":{"offset":"8192","size":"512","encryption":"aes-xts-plain64"},
          "kdf":{"type":"argon2id","time":1,"memory":0,"cpus":1,"salt":"AAAAAAAAAAA="}}},
          "segments":{"0":{"type":"crypt","offset":"12288","encryption":"aes-xts-plain64","sector_size":512}},
          "digests":{}}"#;
        let img = build_luks2_json(json);
        assert!(matches!(
            LuksVolume::unlock2_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::Unsupported { .. })
        ));
    }

    /// The PBKDF2 keyslot branch's own error propagation, which the Argon2 branch
    /// has (`luks2_bad_argon2_params_errors`) and this one did not: an
    /// unsupported hash in the SLOT's KDF, distinct from the AF hash and the
    /// digest hash the neighbouring tests cover. Fails at the first derivation,
    /// before any key material is read.
    #[test]
    fn luks2_unsupported_keyslot_kdf_hash_errors() {
        let json = r#"{"keyslots":{"0":{"key_size":64,
          "af":{"stripes":8,"hash":"sha256"},
          "area":{"offset":"8192","size":"512","encryption":"aes-xts-plain64"},
          "kdf":{"type":"pbkdf2","hash":"md5","iterations":5,"salt":"AAAA"}}},
          "segments":{"0":{"type":"crypt","offset":"12288","encryption":"aes-xts-plain64","sector_size":512}},
          "digests":{}}"#;
        let img = build_luks2_json(json);
        assert!(matches!(
            LuksVolume::unlock2_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::Unsupported { what: "hash", .. })
        ));
    }

    #[test]
    fn luks2_unsupported_af_hash_errors() {
        // Valid KDF, readable material, but an unsupported AF hash makes af::merge
        // fail after the keyslot decrypt.
        let json = r#"{"keyslots":{"0":{"key_size":64,
          "af":{"stripes":8,"hash":"md5"},
          "area":{"offset":"8192","size":"512","encryption":"aes-xts-plain64"},
          "kdf":{"type":"pbkdf2","hash":"sha256","iterations":5,"salt":"AAAA"}}},
          "segments":{"0":{"type":"crypt","offset":"12288","encryption":"aes-xts-plain64","sector_size":512}},
          "digests":{}}"#;
        let img = build_luks2_json(json);
        assert!(matches!(
            LuksVolume::unlock2_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::Unsupported { what: "hash", .. })
        ));
    }

    #[test]
    fn luks2_unsupported_digest_hash_errors() {
        // Reaches the digest loop, where an unsupported digest hash errors.
        let json = r#"{"keyslots":{"0":{"key_size":64,
          "af":{"stripes":8,"hash":"sha256"},
          "area":{"offset":"8192","size":"512","encryption":"aes-xts-plain64"},
          "kdf":{"type":"pbkdf2","hash":"sha256","iterations":5,"salt":"AAAA"}}},
          "segments":{"0":{"type":"crypt","offset":"12288","encryption":"aes-xts-plain64","sector_size":512}},
          "digests":{"0":{"type":"pbkdf2","keyslots":["0"],"hash":"md5","iterations":5,"salt":"AAAA","digest":"AAAA"}}}"#;
        let img = build_luks2_json(json);
        assert!(matches!(
            LuksVolume::unlock2_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::Unsupported { what: "hash", .. })
        ));
    }

    #[test]
    fn luks2_digest_mismatch_falls_through() {
        // Two digests exercise both digest-loop branches: digest "0" references the
        // keyslot but its bytes don't match (the wrong-value fall-through), while
        // digest "1" references another keyslot (the skip branch). Neither
        // verifies, so unlock ends in AuthenticationFailed.
        let json = r#"{"keyslots":{"0":{"key_size":64,
          "af":{"stripes":8,"hash":"sha256"},
          "area":{"offset":"8192","size":"512","encryption":"aes-xts-plain64"},
          "kdf":{"type":"pbkdf2","hash":"sha256","iterations":5,"salt":"AAAA"}}},
          "segments":{"0":{"type":"crypt","offset":"12288","encryption":"aes-xts-plain64","sector_size":512}},
          "digests":{"0":{"type":"pbkdf2","keyslots":["0"],"hash":"sha256","iterations":5,"salt":"AAAA","digest":"AAAA"},
                     "1":{"type":"pbkdf2","keyslots":["9"],"hash":"sha256","iterations":5,"salt":"AAAA","digest":"AAAA"}}}"#;
        let img = build_luks2_json(json);
        assert!(matches!(
            LuksVolume::unlock2_with_passphrase(Cursor::new(img), PASS),
            Err(LuksError::AuthenticationFailed)
        ));
    }

    // ---- read_available I/O arms ------------------------------------------

    struct InterruptOnce(bool);
    impl Read for InterruptOnce {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            if self.0 {
                Ok(0)
            } else {
                self.0 = true;
                Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
            }
        }
    }

    struct AlwaysError;
    impl Read for AlwaysError {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }
    }

    #[test]
    fn read_available_retries_on_interrupted() {
        let mut r = InterruptOnce(false);
        let mut buf = [0xFFu8; 4];
        assert_eq!(read_available(&mut r, &mut buf).unwrap(), 0);
        assert_eq!(buf, [0u8; 4]); // zero-filled on EOF
    }

    #[test]
    fn read_available_propagates_hard_error() {
        let mut r = AlwaysError;
        let mut buf = [0u8; 4];
        assert!(read_available(&mut r, &mut buf).is_err());
    }
}
