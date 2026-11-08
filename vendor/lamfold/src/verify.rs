//! The shepherd — the integrity-verification seam (L2↔L3).
//!
//! "Nothing enters the fold unverified." A [`Verifier`] checks read-only data
//! against a trusted root as it is read, turning a read-only reader into a
//! *trusted* one. Block-level verification is the primitive; file-level and
//! whole-image (dm-verity / fs-verity / composefs) verification compose on top.
//!
//! The substrate ships the no-op [`NoVerifier`] (for SB-off / unverified media); the
//! real fs-verity / dm-verity / composefs verifiers land with `lamfold-erofs`
//! built on a native Merkle over RustCrypto `sha2`. The seam exists from
//! day one so they drop in without re-architecting the frontends.

use crate::error::Result;
use crate::frontend::NodeId;

/// Verifies a block of read-only data against a trusted integrity root.
///
/// A frontend calls [`Verifier::verify_block`] for each data/metadata block it
/// surfaces; `Ok(())` means the bytes match the trusted Merkle/hash tree, an
/// `Err(FoldError::VerifyFailed)` means tampering and the read must be refused.
pub trait Verifier {
    fn verify_block(&self, node: NodeId, offset: u64, data: &[u8]) -> Result<()>;
}

/// The no-op verifier: accepts everything. Used when the medium has no trust
/// root (Secure Boot off, or a frontend without an integrity story yet).
pub struct NoVerifier;

impl Verifier for NoVerifier {
    fn verify_block(&self, _node: NodeId, _offset: u64, _data: &[u8]) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_verifier_accepts() {
        let v = NoVerifier;
        assert!(v.verify_block(1, 0, &[1, 2, 3]).is_ok());
    }
}
