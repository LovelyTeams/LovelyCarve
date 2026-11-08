//! The shepherd's native fs-verity Merkle (SHA-256) — `no_std` + `alloc`.
//!
//! This is the integrity differentiator (`the lamfold design spec` §7): turn a read-only
//! reader into a *trusted* one. fs-verity is the per-file Merkle measurement
//! that composefs uses to content-address blobs and that dm-verity generalises
//! to a whole image. Rather than depend on the `fs-verity` crate (only partial
//! `no_std`), the shepherd carries a thin native Merkle over RustCrypto `sha2`
//! — the open question (`the lamfold design spec` §11 #5), resolved toward
//! native so it builds in the UEFI target unchanged.
//!
//! Two primitives:
//!
//! * [`fsverity_digest_sha256`] — the file *measurement*: byte-identical to
//!   `fsverity digest` (validated against the real tool as an oracle). This is
//!   the trust anchor composefs/bootc sign.
//! * [`MerkleVerifier`] — a [`Verifier`] built over trusted content; its
//!   [`Verifier::verify_block`] checks each materialised data block against the
//!   trusted leaf layer as a frontend reads, refusing tampered media.
//!
//! The fs-verity Merkle algorithm is the published format (kernel
//! `Documentation/filesystems/fsverity.rst`), not GPL driver code — implemented
//! from the spec and confirmed against the userspace tool's output.

use alloc::vec::Vec;

use sha2::{Digest, Sha256};

use crate::error::{FoldError, Result};
use crate::frontend::NodeId;
use crate::verify::Verifier;

/// fs-verity SHA-256 digest (32 bytes).
pub type Sha256Digest = [u8; 32];

/// fs-verity's default block size is 4 KiB (`log2 = 12`).
pub const DEFAULT_BLOCK_LOG: u8 = 12;

const HASH_LEN: usize = 32;
const SHA256_INPUT_BLOCK: usize = 64; // salt is padded to a multiple of this

/// Compute the fs-verity file digest ("measurement") of `data` with SHA-256, the
/// given `block_log` (log2 of the block size, e.g. 12 for 4 KiB), and an optional
/// `salt`. Byte-identical to `fsverity digest --hash-alg=sha256`.
pub fn fsverity_digest_sha256(data: &[u8], block_log: u8, salt: &[u8]) -> Sha256Digest {
    let bs = 1usize << block_log;
    let root = merkle_root(data, bs, &padded_salt(salt));

    // struct fsverity_descriptor (256 bytes), then SHA-256 over it.
    let mut desc = [0u8; 256];
    desc[0] = 1; // version
    desc[1] = 1; // hash_algorithm = SHA-256
    desc[2] = block_log;
    desc[3] = salt.len() as u8;
    // [4..8] reserved (was sig_size) = 0
    desc[8..16].copy_from_slice(&(data.len() as u64).to_le_bytes());
    desc[16..16 + HASH_LEN].copy_from_slice(&root); // root_hash field is 64 B; high 32 stay 0
    let sl = core::cmp::min(salt.len(), 32);
    desc[80..80 + sl].copy_from_slice(&salt[..sl]); // salt field (32 B)
                                                    // [112..256] reserved = 0

    Sha256::digest(desc).into()
}

/// fs-verity pads the salt to a multiple of the hash's input block (64 B for
/// SHA-256) and prepends it to every hashed block. No salt ⇒ no prefix.
fn padded_salt(salt: &[u8]) -> Vec<u8> {
    if salt.is_empty() {
        return Vec::new();
    }
    let mut v = salt.to_vec();
    let rem = v.len() % SHA256_INPUT_BLOCK;
    if rem != 0 {
        v.resize(v.len() + (SHA256_INPUT_BLOCK - rem), 0);
    }
    v
}

/// Hash one block: `salt_prefix || block`, zero-padded up to the block size.
fn hash_block(salt_prefix: &[u8], block: &[u8], bs: usize) -> Sha256Digest {
    let mut h = Sha256::new();
    h.update(salt_prefix);
    h.update(block);
    let mut pad = bs - block.len();
    let zeros = [0u8; SHA256_INPUT_BLOCK];
    while pad > 0 {
        let n = core::cmp::min(pad, zeros.len());
        h.update(&zeros[..n]);
        pad -= n;
    }
    h.finalize().into()
}

/// The fs-verity Merkle root: hash the data into block digests, pack them into
/// blocks, and hash level-by-level until a single block remains. An empty file
/// has an all-zero root by definition.
fn merkle_root(data: &[u8], bs: usize, salt_prefix: &[u8]) -> Sha256Digest {
    if data.is_empty() {
        return [0u8; HASH_LEN];
    }
    let mut level: Vec<u8> = data.to_vec();
    loop {
        let nblocks = level.len().div_ceil(bs);
        if nblocks <= 1 {
            return hash_block(salt_prefix, &level, bs);
        }
        let mut next = Vec::with_capacity(nblocks * HASH_LEN);
        for i in 0..nblocks {
            let start = i * bs;
            let end = core::cmp::min(start + bs, level.len());
            next.extend_from_slice(&hash_block(salt_prefix, &level[start..end], bs));
        }
        level = next;
    }
}

/// A [`Verifier`] over a single trusted file's content (the dm-verity/composefs
/// block-level model). Holds the trusted leaf-hash layer and the fs-verity
/// measurement; [`Verifier::verify_block`] recomputes each block's hash as it is
/// read and refuses any mismatch — so tampered media fails the read instead of
/// being trusted.
pub struct MerkleVerifier {
    block_size: usize,
    salt_prefix: Vec<u8>,
    leaves: Vec<Sha256Digest>,
    measurement: Sha256Digest,
}

impl MerkleVerifier {
    /// Build a verifier over `data` (the genuine content) with the given block
    /// size and salt. The leaf hashes become the trusted reference; the
    /// fs-verity measurement is the signable trust anchor.
    pub fn over(data: &[u8], block_log: u8, salt: &[u8]) -> Self {
        let block_size = 1usize << block_log;
        let salt_prefix = padded_salt(salt);
        let mut leaves = Vec::with_capacity(data.len().div_ceil(block_size));
        let mut i = 0;
        while i < data.len() {
            let end = core::cmp::min(i + block_size, data.len());
            leaves.push(hash_block(&salt_prefix, &data[i..end], block_size));
            i += block_size;
        }
        Self {
            block_size,
            salt_prefix,
            leaves,
            measurement: fsverity_digest_sha256(data, block_log, salt),
        }
    }

    /// The fs-verity measurement (the trust anchor a signer would attest).
    pub fn measurement(&self) -> Sha256Digest {
        self.measurement
    }

    /// The Merkle block size this verifier checks against.
    pub fn block_size(&self) -> usize {
        self.block_size
    }
}

impl Verifier for MerkleVerifier {
    fn verify_block(&self, _node: NodeId, offset: u64, data: &[u8]) -> Result<()> {
        if !offset.is_multiple_of(self.block_size as u64) {
            return Err(FoldError::VerifyFailed("unaligned verify offset"));
        }
        let idx = (offset / self.block_size as u64) as usize;
        let want = self
            .leaves
            .get(idx)
            .ok_or(FoldError::VerifyFailed("block past Merkle tree"))?;
        if hash_block(&self.salt_prefix, data, self.block_size) == *want {
            Ok(())
        } else {
            Err(FoldError::VerifyFailed("block hash mismatch"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hexs(d: &Sha256Digest) -> alloc::string::String {
        use core::fmt::Write;
        let mut s = alloc::string::String::new();
        for b in d {
            write!(s, "{b:02x}").unwrap();
        }
        s
    }

    fn ramp(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 256) as u8).collect()
    }

    /// Oracle values captured from the real `fsverity digest` tool (SHA-256,
    /// 4 KiB blocks, no salt) — see lamfold-erofs fixture generation.
    #[test]
    fn matches_fsverity_oracle() {
        let cases: [(usize, &str); 5] = [
            (
                0,
                "3d248ca542a24fc62d1c43b916eae5016878e2533c88238480b26128a1f1af95",
            ),
            (
                100,
                "9f37bd4e8c0d50d81d76ec71e6cccc6696f4a196e730c8b2019deaa103a6350a",
            ),
            (
                4096,
                "15a0095100272ab90a2209e97f8a2c54dff6f84d2b29524f95d92fe23b6ef25b",
            ),
            (
                4097,
                "eaf219cbd8f40c7424e41b1034906a8d70b7a9ae42f0eca54393b965866f5932",
            ),
            (
                528384,
                "c9263d6ca4271250c808937ec888e17b08546dc97634f6e98f30b3436ac2e3da",
            ),
        ];
        for (n, want) in cases {
            let got = fsverity_digest_sha256(&ramp(n), DEFAULT_BLOCK_LOG, b"");
            assert_eq!(hexs(&got), want, "size {n}");
        }
    }

    #[test]
    fn verifier_accepts_good_blocks_and_rejects_tampered() {
        let data = ramp(4097); // two blocks: a full block + a 1-byte tail
        let v = MerkleVerifier::over(&data, DEFAULT_BLOCK_LOG, b"");
        // The measurement equals the standalone digest.
        assert_eq!(
            v.measurement(),
            fsverity_digest_sha256(&data, DEFAULT_BLOCK_LOG, b"")
        );
        // Genuine blocks verify.
        assert!(v.verify_block(7, 0, &data[..4096]).is_ok());
        assert!(v.verify_block(7, 4096, &data[4096..]).is_ok());
        // A flipped byte is refused.
        let mut bad = data[..4096].to_vec();
        bad[10] ^= 0x01;
        assert!(matches!(
            v.verify_block(7, 0, &bad),
            Err(FoldError::VerifyFailed(_))
        ));
        // Unaligned / out-of-range offsets are refused.
        assert!(v.verify_block(7, 1, &data[..10]).is_err());
        assert!(v.verify_block(7, 8192, &[0u8; 1]).is_err());
    }

    #[test]
    fn salt_changes_the_measurement() {
        let data = ramp(5000);
        let plain = fsverity_digest_sha256(&data, DEFAULT_BLOCK_LOG, b"");
        let salted = fsverity_digest_sha256(&data, DEFAULT_BLOCK_LOG, b"pepper");
        assert_ne!(plain, salted);
    }
}
