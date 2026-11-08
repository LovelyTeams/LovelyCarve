//! Anti-forensic (AF) information splitter — the LUKS keyslot key-material
//! de-obfuscation, per cryptsetup `lib/luks1/af.c`.
//!
//! A LUKS master key is stored in a keyslot inflated to `stripes` (typically
//! 4000) blocks so that recovering it requires the *whole* keyslot region —
//! erasing any part destroys it. `af_merge` reverses the split:
//!
//! ```text
//!   d = 0
//!   for i in 0..stripes-1:  d = diffuse(d XOR block[i])
//!   master_key = block[stripes-1] XOR d
//! ```
//!
//! `diffuse` hashes each digest-sized chunk prefixed with a big-endian block
//! counter. The hash is the header's `hash-spec` (sha1 / sha256 / sha512).

use sha1::Sha1;
use sha2::{Digest, Sha224, Sha256, Sha384, Sha512};

use crate::error::{LuksError, Result};

/// Diffuse `data` (length `size`) in place-equivalent: hash each `digest`-sized
/// chunk prefixed with its big-endian index; the trailing partial chunk uses the
/// next index and is truncated to the remainder.
fn diffuse<D: Digest>(data: &[u8]) -> Vec<u8> {
    let digest_size = <D as Digest>::output_size();
    let mut out = vec![0u8; data.len()];
    let mut i = 0usize;
    let mut offset = 0usize;
    while offset < data.len() {
        let take = digest_size.min(data.len() - offset);
        let mut hasher = D::new();
        hasher.update((i as u32).to_be_bytes());
        hasher.update(&data[offset..offset + take]);
        let h = hasher.finalize();
        out[offset..offset + take].copy_from_slice(&h[..take]);
        offset += take;
        i += 1;
    }
    out
}

fn xor_into(acc: &mut [u8], other: &[u8]) {
    for (a, b) in acc.iter_mut().zip(other) {
        *a ^= *b;
    }
}

/// Merge `stripes` blocks of `block_size` bytes in `material` back to the
/// `block_size`-byte master key, using hash `D`.
fn af_merge<D: Digest>(material: &[u8], block_size: usize, stripes: usize) -> Vec<u8> {
    let mut acc = vec![0u8; block_size];
    for i in 0..stripes.saturating_sub(1) {
        let start = i * block_size;
        if let Some(block) = material.get(start..start + block_size) {
            xor_into(&mut acc, block);
            acc = diffuse::<D>(&acc);
        }
    }
    let last = (stripes.saturating_sub(1)) * block_size;
    if let Some(block) = material.get(last..last + block_size) {
        xor_into(&mut acc, block);
    }
    acc
}

/// Dispatch `af_merge` on the header `hash_spec` string.
///
/// # Errors
/// [`LuksError::Unsupported`] for a hash spec with no RustCrypto implementation.
pub fn merge(
    hash_spec: &str,
    material: &[u8],
    block_size: usize,
    stripes: usize,
) -> Result<Vec<u8>> {
    // `merge` is re-exported as the public `af_merge`, so it cannot assume a
    // caller reached it through `material_len`: the loop below runs
    // `stripes - 1` times whatever the material length says.
    if stripes > MAX_STRIPES {
        return Err(LuksError::ImplausibleStripes {
            stripes,
            max: MAX_STRIPES,
            block_size,
        });
    }
    match hash_spec {
        "sha1" => Ok(af_merge::<Sha1>(material, block_size, stripes)),
        "sha224" => Ok(af_merge::<Sha224>(material, block_size, stripes)),
        "sha256" => Ok(af_merge::<Sha256>(material, block_size, stripes)),
        "sha384" => Ok(af_merge::<Sha384>(material, block_size, stripes)),
        "sha512" => Ok(af_merge::<Sha512>(material, block_size, stripes)),
        other => Err(LuksError::Unsupported {
            what: "hash",
            value: other.to_string(),
        }),
    }
}

/// Ceiling on the anti-forensic stripe count a keyslot may declare.
///
/// LUKS1 fixes the count at 4000 and cryptsetup writes the same for LUKS2, so
/// this leaves two orders of magnitude of headroom over anything genuine while
/// keeping both the key-material allocation and the merge loop bounded.
pub const MAX_STRIPES: usize = 1 << 20;

/// The size in bytes of the AF key material for a key of `block_size` split into
/// `stripes` (rounded up to the 512-byte sector, as LUKS stores it).
///
/// Fallible because `stripes` comes from the header: the product is the size of
/// a buffer the caller allocates, so an implausible count has to be refused here
/// rather than turned into an allocation request.
///
/// # Errors
/// [`LuksError::ImplausibleStripes`] above [`MAX_STRIPES`], or if the product
/// would overflow.
pub fn material_len(block_size: usize, stripes: usize) -> Result<usize> {
    if stripes > MAX_STRIPES {
        return Err(LuksError::ImplausibleStripes {
            stripes,
            max: MAX_STRIPES,
            block_size,
        });
    }
    let raw = block_size
        .checked_mul(stripes)
        .ok_or(LuksError::ImplausibleStripes {
            stripes,
            max: MAX_STRIPES,
            block_size,
        })?;
    Ok(raw.div_ceil(512) * 512)
}

/// Split `master` into `stripes` blocks with deterministic `filler`, the inverse
/// of `af_merge` — test-only, used to build synthetic keyslots and prove the
/// round-trip. Crate-internal so the volume unlock tests can reach it too.
#[cfg(test)]
pub(crate) fn af_split<D: Digest>(
    master: &[u8],
    block_size: usize,
    stripes: usize,
    filler: u8,
) -> Vec<u8> {
    let mut material = vec![filler; block_size * stripes];
    let mut acc = vec![0u8; block_size];
    for i in 0..stripes - 1 {
        let start = i * block_size;
        xor_into(&mut acc, &material[start..start + block_size]);
        acc = diffuse::<D>(&acc);
    }
    let last = (stripes - 1) * block_size;
    for (j, b) in acc.iter().enumerate() {
        material[last + j] = master[j] ^ b;
    }
    material
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_then_merge_roundtrips_sha256() {
        let master: Vec<u8> = (0..64u8).collect();
        let material = af_split::<Sha256>(&master, 64, 4000, 0x5a);
        let back = merge("sha256", &material, 64, 4000).unwrap();
        assert_eq!(back, master);
    }

    #[test]
    fn split_then_merge_roundtrips_sha1_partial_chunk() {
        // key_bytes=20 with sha1 (digest 20) -> exact; use 32 with sha1 to force a
        // partial trailing chunk in diffuse (20 + 12).
        let master: Vec<u8> = (0..32u8).map(|x| x.wrapping_mul(3)).collect();
        let material = af_split::<Sha1>(&master, 32, 100, 0x33);
        let back = merge("sha1", &material, 32, 100).unwrap();
        assert_eq!(back, master);
    }

    #[test]
    fn split_then_merge_roundtrips_sha512() {
        let master: Vec<u8> = (0..64u8).map(|x| x ^ 0xa5).collect();
        let material = af_split::<Sha512>(&master, 64, 500, 0x77);
        let back = merge("sha512", &material, 64, 500).unwrap();
        assert_eq!(back, master);
    }

    #[test]
    fn unsupported_hash_errors() {
        assert!(matches!(
            merge("md5", &[0u8; 64], 64, 1),
            Err(LuksError::Unsupported { what: "hash", .. })
        ));
    }

    #[test]
    fn material_len_rounds_to_sector() {
        // 256000 already sector-aligned; 64 rounds up to 512.
        assert_eq!(material_len(64, 4000).unwrap(), 256_000);
        assert_eq!(material_len(64, 1).unwrap(), 512);
    }

    /// The stripe count is a header field, so it is chosen by the container. It
    /// sizes the key-material buffer the caller allocates — `vec![0u8; len]` —
    /// so an unbounded value is a request to allocate on demand: 64 bytes times
    /// `u32::MAX` stripes is roughly 275 GB.
    ///
    /// Real containers use 4000 (LUKS1 fixes it there, and cryptsetup writes the
    /// same for LUKS2), so refusing far above that costs nothing real.
    #[test]
    fn an_absurd_stripe_count_is_refused_before_sizing_a_buffer() {
        let err = material_len(64, u32::MAX as usize)
            .expect_err("a 275 GB key-material buffer must be refused, not returned");
        let msg = err.to_string();
        assert!(
            msg.contains(&(u32::MAX as usize).to_string()),
            "the refusal must name the stripe count asked for: {msg}"
        );
    }

    /// `merge` is re-exported as the public `af_merge`, so it cannot rely on a
    /// caller having gone through `material_len` first: it loops `stripes - 1`
    /// times and must refuse an absurd count on its own.
    #[test]
    fn merge_refuses_an_absurd_stripe_count_without_looping() {
        assert!(matches!(
            merge("sha256", &[0u8; 64], 64, u32::MAX as usize),
            Err(LuksError::ImplausibleStripes { .. })
        ));
    }

    /// The bound must not reject what a real container asks for.
    #[test]
    fn a_real_stripe_count_still_merges() {
        let master = [0x5Au8; 64];
        let split = af_split::<Sha256>(&master, 64, 4000, 0xA5);
        assert_eq!(merge("sha256", &split, 64, 4000).unwrap(), master);
    }
}
