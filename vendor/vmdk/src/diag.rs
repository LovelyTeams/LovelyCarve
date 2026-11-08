//! Internal diagnostics — a single, greppable home for every trace event.
//!
//! Default-off and zero-cost: without the `trace` feature these are `#[inline]`
//! no-ops and the crate carries no logging dependency. Enable `trace` to forward
//! every event to the [`tracing`](https://docs.rs/tracing) ecosystem for
//! structured, level-filtered debugging of reads over untrusted images.
//!
//! Keeping all diagnostics here (rather than scattered macros) means the full set
//! of observable events is discoverable at a glance, and the read/recovery code
//! reads as plain control flow.

/// An image was opened: detected format and geometry.
#[cfg(feature = "trace")]
pub(crate) fn opened(
    format: &str,
    version: u32,
    virtual_size: u64,
    grain_size: u64,
    compressed: bool,
) {
    tracing::debug!(
        format,
        version,
        virtual_size,
        grain_size,
        compressed,
        "vmdk opened"
    );
}
#[cfg(not(feature = "trace"))]
#[inline]
pub(crate) fn opened(
    _format: &str,
    _version: u32,
    _virtual_size: u64,
    _grain_size: u64,
    _compressed: bool,
) {
}

/// A virtual offset was resolved to a physical location (`kind`: sparse / file / compressed).
#[cfg(feature = "trace")]
pub(crate) fn grain_resolved(virtual_offset: u64, kind: &'static str) {
    tracing::trace!(virtual_offset, kind, "grain resolved");
}
#[cfg(not(feature = "trace"))]
#[inline]
pub(crate) fn grain_resolved(_virtual_offset: u64, _kind: &'static str) {}

/// A damaged primary grain-table pointer was recovered via the redundant grain directory.
#[cfg(feature = "trace")]
pub(crate) fn pointer_recovered(gd_idx: usize, primary: u32, via_rgd: u32) {
    tracing::debug!(
        gd_idx,
        primary,
        via_rgd,
        "grain-table pointer recovered via RGD"
    );
}
#[cfg(not(feature = "trace"))]
#[inline]
pub(crate) fn pointer_recovered(_gd_idx: usize, _primary: u32, _via_rgd: u32) {}

/// A lost primary grain-table entry was recovered from the redundant grain table.
#[cfg(feature = "trace")]
pub(crate) fn entry_recovered(gd_idx: usize, gte_idx: u64) {
    tracing::debug!(gd_idx, gte_idx, "grain-table entry recovered via RGD");
}
#[cfg(not(feature = "trace"))]
#[inline]
pub(crate) fn entry_recovered(_gd_idx: usize, _gte_idx: u64) {}

/// A snapshot/delta chain layer was opened.
#[cfg(feature = "trace")]
pub(crate) fn chain_layer(depth: usize, cid: u32, parent_cid: u32) {
    tracing::debug!(depth, cid, parent_cid, "chain layer opened");
}
#[cfg(not(feature = "trace"))]
#[inline]
pub(crate) fn chain_layer(_depth: usize, _cid: u32, _parent_cid: u32) {}
