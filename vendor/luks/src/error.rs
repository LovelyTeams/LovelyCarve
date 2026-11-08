//! Error type for LUKS parsing and unlocking.

use std::io;

/// Result alias for `luks-core`.
pub type Result<T> = std::result::Result<T, LuksError>;

/// A LUKS parse or unlock failure. Every variant names the offending value so an
/// investigator can act on it (never a bare "invalid").
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LuksError {
    /// The `LUKS\xba\xbe` magic is absent — not a LUKS container.
    #[error("not a LUKS container: magic is {found:02x?}, expected 4c554b53babe")]
    NotLuks {
        /// The first six bytes actually found.
        found: [u8; 6],
    },

    /// The header version is neither 1 nor 2.
    #[error("unsupported LUKS version {version} (only 1 and 2 are supported)")]
    UnsupportedVersion {
        /// The version field value.
        version: u16,
    },

    /// The cipher/mode/hash combination has no validated decrypt path.
    #[error("unsupported {what}: {value:?}")]
    Unsupported {
        /// Which axis is unsupported (cipher, mode, hash).
        what: &'static str,
        /// The offending value verbatim.
        value: String,
    },

    /// The keyslot demands more key-derivation work than the wall-clock budget
    /// allows. The iteration count comes from the header, so a crafted container
    /// can ask for billions of rounds; refusing is the only way the tool stays
    /// responsive on hostile input.
    ///
    /// A *refusal*, never a silent reduction: deriving with fewer rounds than the
    /// header specifies produces a different key, which would report a wrong
    /// An unlock exhausted its TOTAL time budget across all keyslots.
    ///
    /// Distinct from [`Self::DerivationBudgetExceeded`], which bounds a single
    /// derivation. LUKS permits 8 keyslots and the header decides how many are
    /// active and what each costs, so a crafted header can stay under the
    /// per-derivation budget on every slot while the sum runs for hours. Each
    /// individual check passes; nothing owns the total. This variant is that
    /// total.
    #[error(
        "unlock exceeded its total budget of {budget_secs}s after {slots_tried} keyslot(s); \
         the header's cumulative key-derivation cost is not satisfiable"
    )]
    UnlockBudgetExceeded {
        /// How many keyslots had been attempted when the budget ran out.
        slots_tried: usize,
        /// The total budget for the whole unlock.
        budget_secs: u64,
    },

    /// passphrase for a container that would in fact have opened.
    #[error(
        "key derivation would take about {projected_secs}s for {iterations} iterations, \
         over the {budget_secs}s budget — refusing rather than clamping, because a \
         reduced iteration count derives a different key"
    )]
    DerivationBudgetExceeded {
        /// The iteration count the header asked for, verbatim.
        iterations: u32,
        /// Projected cost on this machine, measured from a calibration run.
        projected_secs: u64,
        /// The budget that was exceeded.
        budget_secs: u64,
    },

    /// The keyslot's anti-forensic stripe count is not plausible.
    ///
    /// `stripes` is a header field, so the container chooses it. It multiplies
    /// the key size to give the key-material length the caller then allocates,
    /// and it is the trip count of the AF merge loop — so an unbounded value is
    /// both an allocation and a spin, on the container's say-so.
    ///
    /// LUKS1 fixes the count at 4000 and cryptsetup writes the same for LUKS2,
    /// so the ceiling sits far above anything genuine.
    #[error(
        "keyslot declares {stripes} anti-forensic stripes, over the {max} ceiling \
         — refusing before sizing a {} byte buffer",
        .stripes.saturating_mul(*.block_size)
    )]
    ImplausibleStripes {
        /// The stripe count the header asked for, verbatim.
        stripes: usize,
        /// The ceiling that was exceeded.
        max: usize,
        /// The key size the count would have been multiplied by.
        block_size: usize,
    },

    /// The Argon2 keyslot demands more memory than the ceiling allows.
    ///
    /// Separate from [`Self::DerivationBudgetExceeded`] because memory cannot be
    /// bounded the same way: a time cost can be measured and projected, but
    /// *attempting* an oversized allocation is itself the harm — the process is
    /// killed by the OS before any deadline could fire. `argon2::Params` caps
    /// `m_cost` at `u32::MAX` 1 KiB blocks (4 TiB) and the header chooses the
    /// value, so the ceiling has to be ours.
    #[error(
        "argon2 keyslot asks for {requested_kib} KiB of memory, over the \
         {max_kib} KiB ceiling — refusing before allocating"
    )]
    DerivationMemoryExceeded {
        /// The memory cost the header asked for, verbatim, in KiB blocks.
        requested_kib: u32,
        /// The ceiling that was exceeded, in KiB blocks.
        max_kib: u32,
    },

    /// The header asks to derive a master key longer than the ceiling allows.
    ///
    /// A third cost axis, independent of iterations and of Argon2 memory, and
    /// bounded for the same reason as the memory one: the derived-key length
    /// multiplies the work of every PBKDF2 iteration (`ceil(dkLen / hLen)` HMAC
    /// blocks each), and it sizes an allocation. Crucially it also sizes the
    /// CALIBRATION probe used to project the iteration cost, so leaving it
    /// unbounded makes the measurement itself the denial of service — the guard
    /// has to run before anything is measured or allocated.
    #[error(
        "header asks to derive a {requested} byte master key, over the {max} \
         byte ceiling — refusing before allocating or calibrating"
    )]
    KeyLengthExceeded {
        /// The master-key length the header asked for, verbatim, in bytes.
        requested: usize,
        /// The ceiling that was exceeded, in bytes.
        max: usize,
    },

    /// The header is structurally malformed (a field runs past the buffer).
    #[error("malformed LUKS header: {what} (need {need} bytes, have {got})")]
    MalformedHeader {
        /// What was being read.
        what: &'static str,
        /// Bytes needed.
        need: usize,
        /// Bytes available.
        got: usize,
    },

    /// No keyslot could be unlocked with the supplied passphrase, or the derived
    /// master key failed the mk-digest check (wrong passphrase).
    #[error("authentication failed: no keyslot matched the passphrase")]
    AuthenticationFailed,

    /// The container carries no active (enabled) keyslot.
    #[error("no active keyslot in the LUKS header")]
    NoActiveKeyslot,

    /// An I/O error reading the container.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}
