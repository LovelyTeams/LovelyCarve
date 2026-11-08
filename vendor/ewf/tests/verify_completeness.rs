//! A hash over a PARTIAL image must not be presented as a verdict on the
//! evidence.
//!
//! EWF stores each segment's chunk `table` at the END of that segment. A
//! truncated segment therefore loses the index for ALL of its chunks, not
//! merely the bytes lost to truncation — so the reader silently addresses less
//! media than the volume section declares, hashes what it can reach, and
//! reports a mismatch that looks like a finding about the evidence.
//!
//! Observed on a real 478-segment acquisition: segment `EFM` was truncated
//! 394,233,009 bytes short, its `next` pointer ran past EOF, and its `table` /
//! `table2` sections were gone. The chunk table then addressed 1,022,062,886,912
//! of 1,024,209,543,168 declared bytes — 2,146,656,256 short, almost exactly one
//! segment — and `verify` reported only `MD5 match: FAIL`.
//!
//! libewf independently called the same set `Is corrupted: yes`, which is the
//! diagnosis our own output owed the examiner and did not give.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ewf::VerifyResult;

fn result(addressable: u64, declared: u64) -> VerifyResult {
    VerifyResult {
        computed_md5: [0u8; 16],
        computed_sha1: None,
        md5_match: Some(false),
        sha1_match: None,
        addressable_bytes: addressable,
        declared_bytes: declared,
    }
}

/// The real geometry: a short table is reported as incomplete, with the exact
/// shortfall.
#[test]
fn a_short_chunk_table_is_reported_incomplete() {
    let r = result(1_022_062_886_912, 1_024_209_543_168);
    assert!(!r.is_complete(), "the table addresses less than the media");
    assert_eq!(
        r.missing_bytes(),
        2_146_656_256,
        "the shortfall must be exact -- an examiner sizes the gap from it"
    );
}

/// A table covering the whole media is complete, and reports no shortfall.
#[test]
fn a_full_chunk_table_is_complete() {
    let r = result(1_024_209_543_168, 1_024_209_543_168);
    assert!(r.is_complete());
    assert_eq!(r.missing_bytes(), 0);
}

/// Over-coverage is still complete, not a negative shortfall.
///
/// The final chunk is padded to the chunk size, so the table legitimately
/// addresses a little more than the declared media. `saturating_sub` keeps that
/// from underflowing into an enormous "missing" figure.
#[test]
fn trailing_chunk_padding_is_not_a_shortfall() {
    let r = result(1_024_209_567_744, 1_024_209_543_168);
    assert!(r.is_complete(), "padding past the declared size is normal");
    assert_eq!(r.missing_bytes(), 0, "must not underflow");
}
