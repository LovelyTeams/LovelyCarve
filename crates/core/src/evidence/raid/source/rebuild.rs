//! Rebuilding one data column of a RAID4 / RAID5 / RAID6 stripe row.
//!
//! The other columns of the row are read over the same member span. A column
//! counts as erased where it is absent or where its bytes are synthesized, and
//! each byte span is solved with the columns that are authentic there: XOR
//! with P for one erased data column, the Q syndrome when P is erased too, and
//! the two-erasure RAID6 solution for two erased data columns. A span the row's
//! redundancy cannot cover stays synthesized (zero) instead of being computed
//! from synthesized zeros.

use std::ops::Range;

use anyhow::{bail, Context};

use super::gf256;
use super::parity_layout::RowPlacement;
use super::traced::TracedBytes;

/// How the wanted data column is computed from the rest of its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plan {
    /// P ⊕ every other data column.
    FromP,
    /// The Q syndrome over the other data columns, scaled by the wanted
    /// column's coefficient.
    FromQ,
    /// Two erased data columns (`other` is the second), solved with P and Q.
    FromPq { other: usize },
}

/// The plan that rebuilds data column `wanted` when the columns `erased`
/// accepts are unavailable (`wanted` itself always is). `None` when the row's
/// redundancy cannot cover them.
pub(crate) fn plan(
    row: &RowPlacement,
    wanted: usize,
    erased: impl Fn(usize) -> bool,
) -> Option<Plan> {
    let mut others = row
        .data
        .iter()
        .copied()
        .filter(|&column| column != wanted && erased(column));
    let other = others.next();
    if others.next().is_some() {
        return None;
    }
    let p = !erased(row.p);
    let q = row.q.is_some_and(|q| !erased(q));
    match (other, p, q) {
        (None, true, _) => Some(Plan::FromP),
        (None, false, true) => Some(Plan::FromQ),
        (Some(other), true, true) => Some(Plan::FromPq { other }),
        (None, false, false) | (Some(_), false, _) | (Some(_), true, false) => None,
    }
}

/// Rebuild data column `wanted` of `row` (an `n`-column array) over `len`
/// bytes. `columns[c]` holds column `c` over the same member span, `None`
/// where it was not read (absent, or `wanted` itself).
pub(crate) fn rebuild(
    row: &RowPlacement,
    n: usize,
    wanted: usize,
    columns: &[Option<TracedBytes>],
    len: usize,
) -> anyhow::Result<TracedBytes> {
    // Cut the span wherever some column switches between authentic and
    // synthesized, so every piece has one erasure set.
    let mut cuts = vec![0, len];
    for column in columns.iter().flatten() {
        for range in &column.synthetic {
            cuts.push(range.start.min(len));
            cuts.push(range.end.min(len));
        }
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut bytes = vec![0u8; len];
    let mut synthetic: Vec<Range<usize>> = Vec::new();
    for pair in cuts.windows(2) {
        let &[start, end] = pair else {
            continue;
        };
        let erased = |column: usize| {
            column == wanted
                || match columns.get(column) {
                    Some(Some(traced)) => traced
                        .synthetic
                        .iter()
                        .any(|range| range.start <= start && start < range.end),
                    Some(None) | None => true,
                }
        };
        match plan(row, wanted, erased) {
            Some(plan) => {
                let span = start..end;
                let out = bytes
                    .get_mut(span.clone())
                    .context("RAID rebuild span outside the chunk")?;
                solve(plan, row, n, wanted, columns, span, out)?;
            }
            None => match synthetic.last_mut() {
                Some(last) if last.end == start => last.end = end,
                _ => synthetic.push(start..end),
            },
        }
    }
    Ok(TracedBytes { bytes, synthetic })
}

/// Fill `out` with the wanted column over `span` according to `plan`.
fn solve(
    plan: Plan,
    row: &RowPlacement,
    n: usize,
    wanted: usize,
    columns: &[Option<TracedBytes>],
    span: Range<usize>,
    out: &mut [u8],
) -> anyhow::Result<()> {
    let column = |index: usize| -> anyhow::Result<&[u8]> {
        match columns.get(index) {
            Some(Some(traced)) => traced
                .bytes
                .get(span.clone())
                .with_context(|| format!("RAID column {index} is shorter than the rebuilt span")),
            Some(None) | None => bail!("RAID column {index} was not read for the rebuild"),
        }
    };
    // P ⊕ the data columns outside `skip`.
    let p_delta = |target: &mut [u8], skip: &[usize]| -> anyhow::Result<()> {
        target.copy_from_slice(column(row.p)?);
        for &data in row.data.iter().filter(|data| !skip.contains(data)) {
            xor_into(target, column(data)?);
        }
        Ok(())
    };
    // Q ⊕ Σ g^e·D over the data columns outside `skip`.
    let q_delta = |target: &mut [u8], skip: &[usize]| -> anyhow::Result<()> {
        let Some(q) = row.q else {
            bail!("stripe row has no Q column");
        };
        target.copy_from_slice(column(q)?);
        for &data in row.data.iter().filter(|data| !skip.contains(data)) {
            let coefficient = gf256::pow2(row.q_exponent(data, n));
            gf256::mul_xor_into(target, column(data)?, coefficient);
        }
        Ok(())
    };

    match plan {
        Plan::FromP => p_delta(out, &[wanted]),
        Plan::FromQ => {
            q_delta(out, &[wanted])?;
            let coefficient = gf256::pow2(row.q_exponent(wanted, n));
            let Some(inverse) = gf256::inverse(coefficient) else {
                bail!("RAID6 Q coefficient has no inverse");
            };
            gf256::scale(out, inverse);
            Ok(())
        }
        Plan::FromPq { other } => {
            // With x the wanted column and y the other:
            //   p = Dx ⊕ Dy,  q = g^ex·Dx ⊕ g^ey·Dy
            //   Dx = (q ⊕ g^ey·p) / (g^ex ⊕ g^ey)
            let skip = [wanted, other];
            let mut p = vec![0u8; out.len()];
            p_delta(&mut p, &skip)?;
            q_delta(out, &skip)?;
            let g_wanted = gf256::pow2(row.q_exponent(wanted, n));
            let g_other = gf256::pow2(row.q_exponent(other, n));
            gf256::mul_xor_into(out, &p, g_other);
            let Some(inverse) = gf256::inverse(g_wanted ^ g_other) else {
                bail!("RAID6 columns {wanted} and {other} share a Q coefficient");
            };
            gf256::scale(out, inverse);
            Ok(())
        }
    }
}

pub(crate) fn xor_into(target: &mut [u8], source: &[u8]) {
    for (byte, &value) in target.iter_mut().zip(source) {
        *byte ^= value;
    }
}
