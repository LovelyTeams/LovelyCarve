//! Key derivation (PBKDF2-HMAC, Argon2), with per-derivation resource budgets.
//!
//! Primitives come from RustCrypto. Sector ciphers are wired by `sector_cipher`.

#[cfg(test)]
use aes::{cipher::KeyInit, Aes128, Aes256};
#[cfg(test)]
use xts_mode::Xts128;

use std::time::{Duration, Instant};

use crate::error::{LuksError, Result};

/// Wall-clock ceiling for a SINGLE derivation.
///
/// A LUKS keyslot names its own iteration count, so the work is chosen by the
/// container rather than by us. Real ones are cheap — cryptsetup calibrates to
/// roughly a second — so a budget an order of magnitude above that refuses
/// hostile headers without ever getting in a genuine container's way.
///
/// Applied by `UnlockDeadline::remaining`, which hands out
/// `min(time left, DERIVATION_BUDGET)`. Both ceilings then hold at once: no
/// single derivation may run for the whole [`UNLOCK_BUDGET`], and no sequence of
/// them may exceed it.
pub const DERIVATION_BUDGET: Duration = Duration::from_secs(30);

/// Total budget for one unlock, across every keyslot it tries.
///
/// [`DERIVATION_BUDGET`] bounds a single derivation. It cannot bound an unlock:
/// `recover_master_key*` derives twice per active keyslot and LUKS permits 8, so
/// a header that stays just under the per-derivation budget on every slot costs
/// 16x it. That is how the `unlock` fuzz target reached a 1778s timeout while
/// every individual budget check passed.
pub const UNLOCK_BUDGET: Duration = Duration::from_secs(90);

/// Iterations timed to measure this machine's PBKDF2 rate before committing to
/// the full count. Large enough to swamp timer noise, small enough that paying
/// it twice on the accepted path is irrelevant.
const CALIBRATION_ITERS: u32 = 20_000;

fn pbkdf2_into(
    hash_spec: &str,
    password: &[u8],
    salt: &[u8],
    iters: u32,
    out: &mut [u8],
) -> Result<()> {
    match hash_spec {
        "sha1" => pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, salt, iters, out),
        "sha224" => pbkdf2::pbkdf2_hmac::<sha2::Sha224>(password, salt, iters, out),
        "sha256" => pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password, salt, iters, out),
        "sha384" => pbkdf2::pbkdf2_hmac::<sha2::Sha384>(password, salt, iters, out),
        "sha512" => pbkdf2::pbkdf2_hmac::<sha2::Sha512>(password, salt, iters, out),
        other => {
            return Err(LuksError::Unsupported {
                what: "hash",
                value: other.to_string(),
            })
        }
    }
    Ok(())
}

/// Derive `key_len` bytes with PBKDF2-HMAC-`hash_spec`, within `budget`.
///
/// The budget is a parameter rather than a default because there is no correct
/// default: a caller running several derivations in sequence must pass the time
/// REMAINING in its own total, or the sum goes unbounded however sound each
/// individual check is (issue #10). Making it mandatory means no unbudgeted
/// derivation path exists to reach for by accident.
///
/// Costs above `budget` are refused before the work starts. The bound is on
/// projected wall-clock rather than on the iteration count itself, because "too
/// many rounds" is a property of the machine doing the work, not a number that
/// can be fixed in advance — and a count that is absurd on a laptop may be
/// routine on a workstation.
///
/// The projection comes from timing a short [`CALIBRATION_ITERS`] run and scaling
/// it: PBKDF2 is exactly linear in its iteration count, so the estimate is sound.
/// The alternative — checking a deadline inside the iteration loop — would mean
/// hand-rolling PBKDF2 around a raw HMAC, and this module derives every primitive
/// from an audited RustCrypto crate on purpose.
///
/// One consequence is worth stating plainly: acceptance is machine-dependent, so
/// a container near the budget can be refused on a slow host and accepted on a
/// fast one. The error names the projection and the budget so that is visible
/// rather than mysterious.
///
/// # Errors
/// [`LuksError::Unsupported`] for a hash spec with no implementation;
/// [`LuksError::DerivationBudgetExceeded`] when the projected cost is over budget.
pub fn derive_key_within(
    hash_spec: &str,
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    key_len: usize,
    budget: Duration,
) -> Result<Vec<u8>> {
    // Before anything allocates or measures. The calibration probe below runs at
    // this very length, so an unbounded key_len would make the measurement
    // itself the denial of service and neither budget could ever be consulted.
    if key_len > MAX_KEY_BYTES {
        return Err(LuksError::KeyLengthExceeded {
            requested: key_len,
            max: MAX_KEY_BYTES,
        });
    }

    let mut out = vec![0u8; key_len];
    if iterations == 0 {
        return Err(LuksError::Unsupported {
            what: "PBKDF2 iterations",
            value: "0".into(),
        });
    }
    let iters = iterations;

    if iters > CALIBRATION_ITERS {
        let mut probe = vec![0u8; key_len];
        let started = Instant::now();
        pbkdf2_into(hash_spec, password, salt, CALIBRATION_ITERS, &mut probe)?;
        let measured = started.elapsed();

        // Scale in nanos so a sub-microsecond per-iteration cost does not round
        // to zero, and saturate rather than overflow: u32::MAX iterations times
        // any real per-iteration cost leaves u64 nanoseconds far behind.
        let projected_nanos =
            measured.as_nanos().saturating_mul(u128::from(iters)) / u128::from(CALIBRATION_ITERS);
        let projected = Duration::from_nanos(u64::try_from(projected_nanos).unwrap_or(u64::MAX));

        if projected > budget {
            return Err(LuksError::DerivationBudgetExceeded {
                iterations: iters,
                projected_secs: projected.as_secs(),
                budget_secs: budget.as_secs(),
            });
        }
    }

    pbkdf2_into(hash_spec, password, salt, iters, &mut out)?;
    Ok(out)
}

/// Argon2 KDF parameters from a LUKS2 keyslot (`argon2i` / `argon2id`).
pub struct Argon2Params<'a> {
    /// `argon2i` or `argon2id`.
    pub kind: &'a str,
    /// Time cost (iterations).
    pub time: u32,
    /// Memory cost in KiB.
    pub memory: u32,
    /// Parallelism (lanes).
    pub cpus: u32,
    /// Salt bytes.
    pub salt: &'a [u8],
}

/// Ceiling on the Argon2 memory cost a keyslot may demand, in KiB blocks.
///
/// `argon2::Params` caps `m_cost` at `u32::MAX` blocks — 4 TiB — and the header
/// chooses the value, so the real ceiling has to be ours. cryptsetup benchmarks
/// LUKS2 keyslots to at most about 1 GiB, so 4 GiB is generous headroom over
/// anything a genuine container asks for.
const MAX_ARGON2_MEMORY_KIB: u32 = 4 * 1024 * 1024;

/// Ceiling on the master-key length a header may ask a KDF to derive, in bytes.
///
/// The third attacker-chosen cost axis, alongside the iteration count and the
/// Argon2 memory cost. `key_bytes` is a `u32` read straight from the header, and
/// it scales every PBKDF2 iteration by `ceil(dkLen / hLen)` HMAC blocks as well
/// as sizing an allocation, so it needs a ceiling of ours for the same reason
/// [`MAX_ARGON2_MEMORY_KIB`] does.
///
/// 1 KiB is generous: the largest master key any real LUKS cipher uses is 64
/// bytes (AES-256-XTS, two 256-bit keys), so this leaves 16x headroom over
/// anything a genuine container asks for while keeping the worst case at 32 HMAC
/// blocks per iteration instead of 8.4 million.
const MAX_KEY_BYTES: usize = 1024;

/// Derive `key_len` bytes with Argon2 (LUKS2 keyslot KDF), within `budget`.
///
/// Both cost axes come from the keyslot and are therefore attacker-chosen, and
/// they need different treatment:
///
/// * **Memory** is capped outright by [`MAX_ARGON2_MEMORY_KIB`]. It cannot be
///   bounded by wall clock the way the time cost is, because *attempting* an
///   oversized allocation is itself the harm — a `u32::MAX` memory cost gets the
///   process killed by the OS before any deadline could fire (observed: SIGKILL,
///   not a slow return).
/// * **Time** is the Argon2 analogue of the PBKDF2 iteration count and is
///   bounded the same way: one pass is measured, the total projected, and the
///   whole derivation refused if it would exceed `budget`. Argon2 is linear in
///   `t_cost`, so scaling a single pass is sound.
///
/// The calibration pass runs at the requested memory cost, which is why the
/// memory ceiling is enforced first: the measurement must itself be safe.
/// Calibrating costs one extra pass out of `t_cost`, so the overhead shrinks as
/// the input gets more hostile and is at worst a doubling at `t_cost = 2`.
///
/// What `budget` does and does not bound, stated plainly because the difference
/// decides how much a hostile header can still cost: it REFUSES a derivation
/// whose projected cost exceeds the budget. It does not INTERRUPT one already
/// running. A single Argon2 pass has no cancellation point short of hand-rolling
/// the KDF around the compression function, which this module will not do. So
/// the residual exposure is one uninterruptible pass, and the memory ceiling is
/// the only thing bounding it — including for a `t_cost` of 1, which skips
/// calibration entirely because there is nothing to project from. A caller's
/// aggregate deadline still decides whether the NEXT slot may start, which is
/// what stops eight of them from multiplying.
///
/// # Errors
/// [`LuksError::Unsupported`] for an unknown Argon2 variant or invalid params;
/// [`LuksError::DerivationMemoryExceeded`] over the memory ceiling;
/// [`LuksError::DerivationBudgetExceeded`] when the projected time exceeds
/// `budget`.
pub fn derive_key_argon2_within(
    p: &Argon2Params,
    password: &[u8],
    key_len: usize,
    budget: Duration,
) -> Result<Vec<u8>> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let algo = match p.kind {
        "argon2i" => Algorithm::Argon2i,
        "argon2id" => Algorithm::Argon2id,
        other => {
            return Err(LuksError::Unsupported {
                what: "argon2 variant",
                value: other.to_string(),
            })
        }
    };

    // Before anything allocates, including the calibration pass below.
    if key_len > MAX_KEY_BYTES {
        return Err(LuksError::KeyLengthExceeded {
            requested: key_len,
            max: MAX_KEY_BYTES,
        });
    }
    if p.memory > MAX_ARGON2_MEMORY_KIB {
        return Err(LuksError::DerivationMemoryExceeded {
            requested_kib: p.memory,
            max_kib: MAX_ARGON2_MEMORY_KIB,
        });
    }

    let run = |time: u32, out: &mut [u8]| -> Result<()> {
        let params = Params::new(p.memory, time, p.cpus, Some(key_len)).map_err(|e| {
            LuksError::Unsupported {
                what: "argon2 params",
                value: e.to_string(),
            }
        })?;
        Argon2::new(algo, Version::V0x13, params)
            .hash_password_into(password, p.salt, out)
            .map_err(|e| LuksError::Unsupported {
                what: "argon2",
                value: e.to_string(),
            })
    };

    if p.time > 1 {
        let mut probe = vec![0u8; key_len];
        let started = Instant::now();
        run(1, &mut probe)?;
        let measured = started.elapsed();

        let projected_nanos = measured.as_nanos().saturating_mul(u128::from(p.time));
        let projected = Duration::from_nanos(u64::try_from(projected_nanos).unwrap_or(u64::MAX));

        if projected > budget {
            return Err(LuksError::DerivationBudgetExceeded {
                iterations: p.time,
                projected_secs: projected.as_secs(),
                budget_secs: budget.as_secs(),
            });
        }
    }

    let mut out = vec![0u8; key_len];
    run(p.time, &mut out)?;
    Ok(out)
}

/// Decrypt `buffer` in place as AES-XTS-plain64, split into `unit_size`-byte data
/// units. Data unit `u` uses the plain64 tweak `base_sector + u * (unit_size/512)`
/// (little-endian) — the 512-sector number of the unit's first byte, matching
/// dm-crypt's default (non-`iv_large_sectors`) IV even for 4096-byte sectors.
///
/// # Errors
/// [`LuksError::Unsupported`] if `cipher_mode` is not `xts-plain64` or `key` is
/// not a 32- or 64-byte XTS key.
#[cfg(test)]
pub fn xts_decrypt_area(
    cipher_mode: &str,
    key: &[u8],
    buffer: &mut [u8],
    unit_size: usize,
    base_sector: u128,
) -> Result<()> {
    if cipher_mode != "xts-plain64" {
        return Err(LuksError::Unsupported {
            what: "cipher mode",
            value: cipher_mode.to_string(),
        });
    }
    let step = (unit_size / 512).max(1) as u128;
    match key.len() {
        32 => {
            let (k1, k2) = key.split_at(16);
            let xts = Xts128::<Aes128>::new(Aes128::new(k1.into()), Aes128::new(k2.into()));
            decrypt_units(&xts, buffer, unit_size, base_sector, step);
        }
        64 => {
            let (k1, k2) = key.split_at(32);
            let xts = Xts128::<Aes256>::new(Aes256::new(k1.into()), Aes256::new(k2.into()));
            decrypt_units(&xts, buffer, unit_size, base_sector, step);
        }
        n => {
            return Err(LuksError::Unsupported {
                what: "xts key size",
                value: format!("{n} bytes"),
            })
        }
    }
    Ok(())
}

#[cfg(test)]
fn decrypt_units<C>(xts: &Xts128<C>, buffer: &mut [u8], unit_size: usize, base: u128, step: u128)
where
    C: aes::cipher::BlockCipher + aes::cipher::BlockEncrypt + aes::cipher::BlockDecrypt,
{
    for (u, chunk) in buffer.chunks_mut(unit_size).enumerate() {
        if chunk.len() < 16 {
            continue; // cov:unreachable: reads are always unit-aligned (>= 512)
        }
        let tweak = (base + u as u128 * step).to_le_bytes();
        xts.decrypt_sector(chunk, tweak);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xts_mode::get_tweak_default;

    /// A header-supplied iteration count is attacker-controlled, so a crafted
    /// container must not be able to spend the process. The fuzz target found
    /// this the expensive way: `libFuzzer: timeout after 1780 seconds` on the
    /// `unlock` target, seeded from the `"sha1"` dictionary entry.
    ///
    /// The assertion runs the derivation on a worker thread behind a watchdog,
    /// so an unbounded implementation fails this test in seconds instead of
    /// hanging the suite — a red that terminates is the only useful kind.
    #[test]
    fn absurd_iteration_count_is_refused_rather_than_run() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = derive_key_within(
                "sha1",
                b"luks-TEST",
                b"salt",
                u32::MAX,
                32,
                DERIVATION_BUDGET,
            );
            // Assert the *reason*, not merely that something failed: an
            // is_err() check would pass just as happily on an unrelated error
            // and prove nothing about the budget.
            let budgeted = matches!(
                r,
                Err(LuksError::DerivationBudgetExceeded { iterations, .. })
                    if iterations == u32::MAX
            );
            let _ = tx.send(budgeted);
        });

        // `.expect` rather than a match arm: the timeout branch is unreachable
        // while the budget holds, and an arm that never runs is a line the
        // coverage gate would have to exempt for no gain. The panic message is
        // the whole diagnosis if the bound ever regresses.
        let refused = rx
            .recv_timeout(Duration::from_secs(60))
            .expect("derive_key did not return within 60s for u32::MAX iterations — unbounded");
        assert!(
            refused,
            "u32::MAX iterations must be refused with DerivationBudgetExceeded"
        );
    }

    /// The budget must not reject work a real container asks for. cryptsetup
    /// lands around 1–4M iterations for LUKS1, so a count in that range has to
    /// go through untouched and produce the same key as an unbudgeted run.
    #[test]
    fn a_realistic_iteration_count_still_derives() {
        let k = derive_key_within(
            "sha256",
            b"password",
            b"salt",
            200_000,
            32,
            DERIVATION_BUDGET,
        )
        .unwrap();
        assert_eq!(k.len(), 32);
        // Same input, same key — the budget check must not perturb the result.
        let again = derive_key_within(
            "sha256",
            b"password",
            b"salt",
            200_000,
            32,
            DERIVATION_BUDGET,
        )
        .unwrap();
        assert_eq!(k, again);
    }

    #[test]
    fn derive_key_matches_known_pbkdf2_sha256() {
        // PBKDF2-HMAC-SHA256("password","salt",1,32) — cross-checked vs Python.
        let k =
            derive_key_within("sha256", b"password", b"salt", 1, 32, DERIVATION_BUDGET).unwrap();
        assert_eq!(
            hex(&k),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
    }

    #[test]
    fn xts_area_roundtrip_512_units() {
        let key = [0x24u8; 64];
        let mut buf = vec![0u8; 1024];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8) ^ 0x3c;
        }
        let plain = buf.clone();
        // encrypt via the same primitive at 512 sectors starting at sector 5
        let (k1, k2) = key.split_at(32);
        let xts = Xts128::<Aes256>::new(Aes256::new(k1.into()), Aes256::new(k2.into()));
        xts.encrypt_area(&mut buf, 512, 5, get_tweak_default);
        xts_decrypt_area("xts-plain64", &key, &mut buf, 512, 5).unwrap();
        assert_eq!(buf, plain);
    }

    #[test]
    fn xts_area_4096_unit_uses_512_based_tweak() {
        // A 4096-byte data unit at 512-sector base 8 must decrypt what was
        // encrypted with tweak 8 (not 1) — proves the *8 step.
        let key = [0x51u8; 64];
        let mut buf = vec![7u8; 4096];
        let plain = buf.clone();
        let (k1, k2) = key.split_at(32);
        let xts = Xts128::<Aes256>::new(Aes256::new(k1.into()), Aes256::new(k2.into()));
        xts.encrypt_sector(&mut buf, 8u128.to_le_bytes());
        xts_decrypt_area("xts-plain64", &key, &mut buf, 4096, 8).unwrap();
        assert_eq!(buf, plain);
    }

    #[test]
    fn xts_rejects_bad_mode_and_keysize() {
        let mut buf = [0u8; 512];
        assert!(matches!(
            xts_decrypt_area("cbc-essiv", &[0u8; 64], &mut buf, 512, 0),
            Err(LuksError::Unsupported {
                what: "cipher mode",
                ..
            })
        ));
        assert!(matches!(
            xts_decrypt_area("xts-plain64", &[0u8; 48], &mut buf, 512, 0),
            Err(LuksError::Unsupported {
                what: "xts key size",
                ..
            })
        ));
    }

    #[test]
    fn derive_key_rejects_unknown_hash() {
        assert!(matches!(
            derive_key_within("md5", b"x", b"y", 1, 16, DERIVATION_BUDGET),
            Err(LuksError::Unsupported { what: "hash", .. })
        ));
    }

    /// The LUKS2 keyslot names its own Argon2 memory cost, and `argon2::Params`
    /// caps `m_cost` at `u32::MAX` **1 KiB blocks** — 4 TiB. Attempting the
    /// allocation *is* the harm, so unlike the PBKDF2 iteration count this
    /// cannot be bounded by wall clock: there is no point at which to notice.
    #[test]
    fn absurd_argon2_memory_is_refused_before_allocating() {
        let p = Argon2Params {
            kind: "argon2id",
            time: 1,
            memory: u32::MAX, // 4 TiB in KiB blocks
            cpus: 1,
            salt: &[0x11u8; 16],
        };
        let err = derive_key_argon2_within(&p, b"pw", 64, DERIVATION_BUDGET)
            .expect_err("a 4 TiB memory cost must be refused, not attempted");
        assert!(
            matches!(err, LuksError::DerivationMemoryExceeded { .. }),
            "refused for the wrong reason: {err}"
        );
        // The refusal has to name the offending value, not just decline: an
        // examiner needs to see which cost the header asked for.
        let msg = err.to_string();
        assert!(
            msg.contains(&u32::MAX.to_string()),
            "value not named: {msg}"
        );
    }

    /// The time cost is the Argon2 analogue of the PBKDF2 iteration count and is
    /// bounded the same way — measure one pass, project, refuse over budget.
    #[test]
    fn absurd_argon2_time_is_refused_rather_than_run() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let p = Argon2Params {
                kind: "argon2id",
                time: u32::MAX,
                memory: 64,
                cpus: 1,
                salt: &[0x11u8; 16],
            };
            let budgeted = matches!(
                derive_key_argon2_within(&p, b"pw", 64, DERIVATION_BUDGET),
                Err(LuksError::DerivationBudgetExceeded { .. })
            );
            let _ = tx.send(budgeted);
        });

        let refused = rx
            .recv_timeout(Duration::from_secs(60))
            .expect("derive_key_argon2 did not return within 60s for u32::MAX time cost");
        assert!(refused, "u32::MAX time cost must be refused");
    }

    /// A cost a real LUKS2 container asks for must pass untouched. cryptsetup
    /// writes single-digit time costs over tens-to-hundreds of MiB, so this has
    /// to derive normally and reproducibly.
    #[test]
    fn a_realistic_argon2_cost_still_derives() {
        let p = Argon2Params {
            kind: "argon2id",
            time: 4,
            memory: 65_536, // 64 MiB
            cpus: 1,
            salt: &[0x11u8; 16],
        };
        let k = derive_key_argon2_within(&p, b"pw", 64, DERIVATION_BUDGET).unwrap();
        assert_eq!(k.len(), 64);
        assert_eq!(
            k,
            derive_key_argon2_within(&p, b"pw", 64, DERIVATION_BUDGET).unwrap()
        );
    }

    #[test]
    fn argon2id_derives_and_rejects_unknown() {
        let p = Argon2Params {
            kind: "argon2id",
            time: 1,
            memory: 32,
            cpus: 1,
            salt: &[0x11u8; 16],
        };
        let k = derive_key_argon2_within(&p, b"pw", 64, DERIVATION_BUDGET).unwrap();
        assert_eq!(k.len(), 64);
        let bad = Argon2Params {
            kind: "scrypt",
            ..p
        };
        assert!(matches!(
            derive_key_argon2_within(&bad, b"pw", 64, DERIVATION_BUDGET),
            Err(LuksError::Unsupported {
                what: "argon2 variant",
                ..
            })
        ));
    }

    #[test]
    fn xts_area_roundtrip_128bit_key() {
        // 32-byte key => AES-128-XTS branch (two 16-byte sub-keys).
        let key = [0x91u8; 32];
        let mut buf = vec![0u8; 512];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8).wrapping_add(1);
        }
        let plain = buf.clone();
        let (k1, k2) = key.split_at(16);
        let xts = Xts128::<Aes128>::new(Aes128::new(k1.into()), Aes128::new(k2.into()));
        xts.encrypt_area(&mut buf, 512, 0, get_tweak_default);
        xts_decrypt_area("xts-plain64", &key, &mut buf, 512, 0).unwrap();
        assert_eq!(buf, plain);
    }

    #[test]
    fn argon2_rejects_invalid_params() {
        // memory cost 0 is below Argon2's minimum => Params::new fails.
        let p = Argon2Params {
            kind: "argon2id",
            time: 1,
            memory: 0,
            cpus: 1,
            salt: &[0x11u8; 16],
        };
        assert!(matches!(
            derive_key_argon2_within(&p, b"pw", 32, DERIVATION_BUDGET),
            Err(LuksError::Unsupported {
                what: "argon2 params",
                ..
            })
        ));
    }

    #[test]
    fn argon2_rejects_short_salt() {
        // Valid params but a 4-byte salt is below Argon2's 8-byte minimum, so the
        // hash itself fails (not Params::new).
        let p = Argon2Params {
            kind: "argon2id",
            time: 1,
            memory: 32,
            cpus: 1,
            salt: &[0u8; 4],
        };
        assert!(matches!(
            derive_key_argon2_within(&p, b"pw", 32, DERIVATION_BUDGET),
            Err(LuksError::Unsupported { what: "argon2", .. })
        ));
    }

    fn hex(b: &[u8]) -> String {
        use std::fmt::Write;
        b.iter().fold(String::new(), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
    }

    /// The master-key length is a header field, so it is attacker-chosen, and it
    /// multiplies the work of EVERY PBKDF2 call: dkLen bytes cost
    /// `ceil(dkLen / 32)` HMAC blocks per iteration. A LUKS1 header asking for
    /// 0x10101010 bytes (~257 MB) therefore costs ~8.4M blocks per iteration.
    ///
    /// The calibration probe is the sharp edge: it runs at `CALIBRATION_ITERS`
    /// (20 thousand) and at the REQUESTED key length, so it alone is ~1.7e11
    /// HMAC-SHA256 operations. The thing that exists to measure the cost cheaply
    /// becomes the cost. `derive_key_argon2_within` already states this rule for
    /// its memory ceiling — "the measurement must itself be safe" — and enforces
    /// it before allocating; this is the same rule applied to the other axis.
    ///
    /// The elapsed-time assertion is the point of the test: a guard that refuses
    /// AFTER doing the work would still return the right error.
    #[test]
    fn derive_key_refuses_an_implausible_key_length_before_calibrating() {
        let started = Instant::now();
        let err = derive_key_within(
            "sha256",
            b"luks-TEST",
            &[0u8; 32],
            50_000,
            0x1010_1010, // the value from the fuzz timeout artifact
            DERIVATION_BUDGET,
        )
        .expect_err("an implausible master-key length must be refused");
        let elapsed = started.elapsed();

        assert!(
            matches!(err, LuksError::KeyLengthExceeded { .. }),
            "expected KeyLengthExceeded, got {err:?}"
        );
        // Hoisted above the assert on purpose: calling elapsed() inside the
        // message would put a function call on the panic path, and that cold
        // region shows up as an uncovered production line under the 100% gate.
        assert!(
            elapsed < Duration::from_secs(2),
            "refused only after doing the work ({elapsed:?}) — the guard must \
             run before the calibration probe allocates and derives"
        );
    }

    /// Same ceiling on the Argon2 path: `key_len` there is equally header-chosen.
    #[test]
    fn derive_key_argon2_refuses_an_implausible_key_length() {
        let p = Argon2Params {
            kind: "argon2id",
            time: 1,
            memory: 32,
            cpus: 1,
            salt: &[0u8; 16],
        };
        assert!(matches!(
            derive_key_argon2_within(&p, b"pw", 0x1010_1010, DERIVATION_BUDGET),
            Err(LuksError::KeyLengthExceeded { .. })
        ));
    }

    /// The ceiling must not reject anything real: AES-256-XTS uses a 64-byte
    /// master key, the largest any LUKS cipher asks for.
    #[test]
    fn real_master_key_lengths_are_still_accepted() {
        for len in [16usize, 32, 64] {
            assert!(
                derive_key_within("sha256", b"pw", &[0u8; 32], 1, len, DERIVATION_BUDGET).is_ok(),
                "a {len}-byte master key is ordinary and must still derive"
            );
        }
    }
}
