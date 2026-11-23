//! Header key derivation: which PRF and iteration count VeraCrypt and
//! TrueCrypt used, and the derivations themselves.
//!
//! VeraCrypt derives 192 bytes of header key material (enough for a
//! three-cipher cascade in XTS mode) with PBKDF2 over one of its PRFs, or with
//! Argon2id since 1.26.29. TrueCrypt used far fewer PBKDF2 iterations and no
//! PIM. System (pre-boot) headers use smaller counts for some PRFs.

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac, SimpleHmac};
use zeroize::{Zeroize, Zeroizing};

use crate::error::{Result, VeraError};

/// Bytes of header key material every derivation produces.
pub const HEADER_KEY_LEN: usize = 192;

/// The largest PIM VeraCrypt accepts for a volume header: the iteration
/// count `15000 + PIM * 1000` must stay a positive 32-bit integer.
pub const MAX_PIM: u32 = 2_147_468;

/// The largest PIM VeraCrypt's pre-boot authentication accepts.
pub const MAX_BOOT_PIM: u32 = 65_535;

/// How often a PBKDF2 loop checks whether it should stop.
const STOP_POLL_ITERATIONS: u32 = 1024;

/// A header key derivation function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prf {
    /// PBKDF2-HMAC-SHA-512.
    Sha512,
    /// PBKDF2-HMAC-SHA-256.
    Sha256,
    /// PBKDF2-HMAC-BLAKE2s-256 (VeraCrypt 1.26 and later).
    Blake2s,
    /// PBKDF2-HMAC-Whirlpool.
    Whirlpool,
    /// PBKDF2-HMAC-Streebog-512.
    Streebog,
    /// PBKDF2-HMAC-RIPEMD-160 (TrueCrypt, and VeraCrypt up to 1.25.9).
    Ripemd160,
    /// Argon2id (VeraCrypt 1.26.29 and later, never for system encryption).
    Argon2id,
}

impl Prf {
    /// Every PRF, in the order the search tries them.
    pub const ALL: [Prf; 7] = [
        Prf::Sha512,
        Prf::Sha256,
        Prf::Blake2s,
        Prf::Whirlpool,
        Prf::Streebog,
        Prf::Ripemd160,
        Prf::Argon2id,
    ];

    /// The display name VeraCrypt uses.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Prf::Sha512 => "SHA-512",
            Prf::Sha256 => "SHA-256",
            Prf::Blake2s => "BLAKE2s-256",
            Prf::Whirlpool => "Whirlpool",
            Prf::Streebog => "Streebog",
            Prf::Ripemd160 => "RIPEMD-160",
            Prf::Argon2id => "Argon2id",
        }
    }

    /// The stable lowercase identifier (`sha512`, `argon2id` …).
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Prf::Sha512 => "sha512",
            Prf::Sha256 => "sha256",
            Prf::Blake2s => "blake2s",
            Prf::Whirlpool => "whirlpool",
            Prf::Streebog => "streebog",
            Prf::Ripemd160 => "ripemd160",
            Prf::Argon2id => "argon2id",
        }
    }

    /// The PRF an identifier from [`Self::id`] names.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Prf> {
        Prf::ALL.into_iter().find(|prf| prf.id() == id)
    }

    /// Bytes one PBKDF2 block yields; `None` for Argon2id.
    #[must_use]
    pub fn block_len(self) -> Option<usize> {
        match self {
            Prf::Sha512 | Prf::Whirlpool | Prf::Streebog => Some(64),
            Prf::Sha256 | Prf::Blake2s => Some(32),
            Prf::Ripemd160 => Some(20),
            Prf::Argon2id => None,
        }
    }
}

/// Whose iteration rules produced a derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// VeraCrypt's counts, scaled by the PIM.
    VeraCrypt,
    /// TrueCrypt 5.0–7.2's counts (no PIM).
    TrueCrypt,
}

/// Which iteration rules apply to a header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KdfTarget {
    /// A file container, partition or hidden volume header.
    Volume,
    /// A system (pre-boot authentication) header.
    System,
}

/// One derivation the header search tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Kdf {
    /// The PRF.
    pub prf: Prf,
    /// Whose rules gave the cost.
    pub family: Family,
    /// PBKDF2 iterations, or Argon2id passes.
    pub iterations: u32,
    /// Argon2id memory in KiB; zero for PBKDF2.
    pub memory_kib: u32,
}

impl Kdf {
    /// Independent jobs the derivation splits into: one per PBKDF2 block,
    /// one for Argon2id.
    pub(crate) fn block_count(&self) -> usize {
        self.prf
            .block_len()
            .map_or(1, |len| HEADER_KEY_LEN.div_ceil(len))
    }
}

/// Argon2id passes and memory (KiB) for a PIM, as VeraCrypt's
/// `get_argon2_params`: PIM 0 means 12; memory grows by 32 MiB per PIM up to
/// 1 GiB, passes by one per three PIMs up to PIM 31 and by one per PIM after.
#[must_use]
pub fn argon2_cost(pim: u32) -> (u32, u32) {
    let pim = if pim == 0 { 12 } else { pim };
    let memory_mib = (64 + (u64::from(pim) - 1) * 32).min(1024);
    let passes = if pim <= 31 {
        3 + (pim - 1) / 3
    } else {
        13 + (pim - 31)
    };
    (passes, memory_mib as u32 * 1024)
}

fn volume_iterations(pim: u32, default: u32) -> Result<u32> {
    if pim == 0 {
        return Ok(default);
    }
    pim.checked_mul(1000)
        .and_then(|scaled| scaled.checked_add(15_000))
        .ok_or_else(|| VeraError::InvalidRequest(format!("PIM {pim} is out of range")))
}

fn boot_iterations(pim: u32, default: u32) -> Result<u32> {
    if pim == 0 {
        return Ok(default);
    }
    pim.checked_mul(2048)
        .ok_or_else(|| VeraError::InvalidRequest(format!("PIM {pim} is out of range")))
}

fn pbkdf2(prf: Prf, family: Family, iterations: u32) -> Kdf {
    Kdf {
        prf,
        family,
        iterations,
        memory_kib: 0,
    }
}

/// The derivations a header of `target` may use with `pim`, in try order,
/// keeping only `prf` when given. Argon2id is left out above
/// `argon2_max_passes` passes, since a running Argon2id cannot be stopped.
///
/// # Errors
/// [`VeraError::InvalidRequest`] for a PIM above VeraCrypt's limit for the
/// target, or Argon2id requested for a system header.
pub fn candidates(
    target: KdfTarget,
    pim: u32,
    prf: Option<Prf>,
    argon2_max_passes: u32,
) -> Result<Vec<Kdf>> {
    let limit = match target {
        KdfTarget::Volume => MAX_PIM,
        KdfTarget::System => MAX_BOOT_PIM,
    };
    if pim > limit {
        return Err(VeraError::InvalidRequest(format!(
            "PIM {pim} exceeds VeraCrypt's limit of {limit}"
        )));
    }
    if target == KdfTarget::System && prf == Some(Prf::Argon2id) {
        return Err(VeraError::InvalidRequest(
            "system encryption never uses Argon2id".into(),
        ));
    }
    let mut kdfs = Vec::new();
    // TrueCrypt has no PIM; its counts are cheap enough to always try first.
    if pim == 0 {
        match target {
            KdfTarget::Volume => kdfs.extend([
                pbkdf2(Prf::Ripemd160, Family::TrueCrypt, 2000),
                pbkdf2(Prf::Sha512, Family::TrueCrypt, 1000),
                pbkdf2(Prf::Whirlpool, Family::TrueCrypt, 1000),
            ]),
            KdfTarget::System => kdfs.push(pbkdf2(Prf::Ripemd160, Family::TrueCrypt, 1000)),
        }
    }
    let vera = Family::VeraCrypt;
    match target {
        KdfTarget::Volume => {
            let iterations = volume_iterations(pim, 500_000)?;
            kdfs.extend([
                pbkdf2(Prf::Sha512, vera, iterations),
                pbkdf2(Prf::Sha256, vera, iterations),
                pbkdf2(Prf::Blake2s, vera, iterations),
                pbkdf2(Prf::Whirlpool, vera, iterations),
                pbkdf2(Prf::Streebog, vera, iterations),
                pbkdf2(Prf::Ripemd160, vera, volume_iterations(pim, 655_331)?),
            ]);
            let (passes, memory_kib) = argon2_cost(pim);
            if passes <= argon2_max_passes || prf == Some(Prf::Argon2id) {
                kdfs.push(Kdf {
                    prf: Prf::Argon2id,
                    family: vera,
                    iterations: passes,
                    memory_kib,
                });
            }
        }
        KdfTarget::System => {
            let boot = boot_iterations(pim, 200_000)?;
            let full = volume_iterations(pim, 500_000)?;
            kdfs.extend([
                pbkdf2(Prf::Sha256, vera, boot),
                pbkdf2(Prf::Blake2s, vera, boot),
                pbkdf2(Prf::Sha512, vera, full),
                pbkdf2(Prf::Whirlpool, vera, full),
                // Windows derives system Streebog headers with the boot count;
                // VeraCrypt's portable code reads them with the volume count.
                pbkdf2(Prf::Streebog, vera, boot),
                pbkdf2(Prf::Streebog, vera, full),
                pbkdf2(Prf::Ripemd160, vera, boot_iterations(pim, 327_661)?),
            ]);
        }
    }
    if let Some(prf) = prf {
        kdfs.retain(|kdf| kdf.prf == prf);
    }
    Ok(kdfs)
}

/// The derivation was stopped before it finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stopped;

/// Derive PBKDF2 block `index` (1-based) of `prf` into `out`, which takes the
/// block's first `out.len()` bytes. `stop` is polled every 1024 iterations.
pub(crate) fn pbkdf2_block(
    prf: Prf,
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    index: u32,
    out: &mut [u8],
    stop: &dyn Fn() -> bool,
) -> std::result::Result<(), Stopped> {
    match prf {
        Prf::Sha512 => block::<Hmac<sha2::Sha512>>(password, salt, iterations, index, out, stop),
        Prf::Sha256 => block::<Hmac<sha2::Sha256>>(password, salt, iterations, index, out, stop),
        Prf::Blake2s => {
            block::<SimpleHmac<blake2::Blake2s256>>(password, salt, iterations, index, out, stop)
        }
        Prf::Whirlpool => {
            block::<Hmac<whirlpool::Whirlpool>>(password, salt, iterations, index, out, stop)
        }
        Prf::Streebog => {
            block::<Hmac<streebog::Streebog512>>(password, salt, iterations, index, out, stop)
        }
        Prf::Ripemd160 => {
            block::<Hmac<ripemd::Ripemd160>>(password, salt, iterations, index, out, stop)
        }
        // Argon2id has no PBKDF2 blocks; the search never asks for one.
        Prf::Argon2id => Err(Stopped),
    }
}

fn block<M>(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    index: u32,
    out: &mut [u8],
    stop: &dyn Fn() -> bool,
) -> std::result::Result<(), Stopped>
where
    M: Mac + KeyInit + Clone,
{
    // HMAC takes keys of any length, so keying cannot fail.
    let keyed = <M as KeyInit>::new_from_slice(password).map_err(|_| Stopped)?;
    let mut mac = keyed.clone();
    mac.update(salt);
    mac.update(&index.to_be_bytes());
    let mut u = mac.finalize().into_bytes();
    let mut t = u.clone();
    let mut outcome = Ok(());
    for iteration in 1..iterations.max(1) {
        if iteration % STOP_POLL_ITERATIONS == 0 && stop() {
            outcome = Err(Stopped);
            break;
        }
        let mut mac = keyed.clone();
        mac.update(&u);
        u = mac.finalize().into_bytes();
        for (acc, byte) in t.iter_mut().zip(u.iter()) {
            *acc ^= byte;
        }
    }
    if outcome.is_ok() {
        let take = out.len().min(t.len());
        out[..take].copy_from_slice(&t[..take]);
    }
    u.as_mut_slice().zeroize();
    t.as_mut_slice().zeroize();
    outcome
}

/// Derive the full 192-byte header key with Argon2id (one lane, version
/// 0x13), as VeraCrypt's `derive_key_argon2`. This cannot be interrupted.
pub(crate) fn argon2id(
    password: &[u8],
    salt: &[u8],
    passes: u32,
    memory_kib: u32,
) -> Result<Zeroizing<[u8; HEADER_KEY_LEN]>> {
    let params = argon2::Params::new(memory_kib, passes, 1, Some(HEADER_KEY_LEN))
        .map_err(|error| VeraError::InvalidRequest(format!("Argon2id parameters: {error}")))?;
    let blocks = params.block_count();
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    // Reserve fallibly: a failed 1 GiB allocation must not abort the process.
    let mut memory: Vec<argon2::Block> = Vec::new();
    memory.try_reserve_exact(blocks).map_err(|_| {
        VeraError::InvalidRequest(format!(
            "Argon2id needs {} MiB of memory",
            memory_kib / 1024
        ))
    })?;
    memory.resize(blocks, argon2::Block::default());
    let mut key = Zeroizing::new([0u8; HEADER_KEY_LEN]);
    let derived = argon.hash_password_into_with_memory(password, salt, key.as_mut(), &mut memory);
    memory.iter_mut().for_each(Zeroize::zeroize);
    derived.map_err(|error| VeraError::InvalidRequest(format!("Argon2id derivation: {error}")))?;
    Ok(key)
}

/// Whole-key PBKDF2, for tests and callers that do not split blocks.
#[cfg(test)]
pub(crate) fn pbkdf2_full(prf: Prf, password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    let len = prf.block_len().unwrap_or(HEADER_KEY_LEN);
    for (index, chunk) in out.chunks_mut(len).enumerate() {
        pbkdf2_block(
            prf,
            password,
            salt,
            iterations,
            index as u32 + 1,
            chunk,
            &|| false,
        )
        .unwrap_or_else(|_| unreachable!("pbkdf2 test derivation never stops"));
    }
}

#[cfg(test)]
#[path = "kdf_tests.rs"]
mod tests;
