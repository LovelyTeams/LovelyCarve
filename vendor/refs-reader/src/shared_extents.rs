//! Resolve CoW gaps with saved stream spans, preserving each version's own blocks.
use crate::{corrupt, Extent, Result};

pub(crate) fn fill_unmapped(owned: &mut Vec<Extent>, shared: &[Extent], size: u64) -> Result<()> {
    let mut additions = Vec::new();
    for extent in shared {
        let Some(physical) = extent.physical else {
            continue;
        };
        let end = extent
            .logical
            .checked_add(extent.length)
            .ok_or_else(|| corrupt("shared extent overflow"))?
            .min(size);
        let mut cursor = extent.logical;
        let start = owned.partition_point(|run| run.logical.saturating_add(run.length) <= cursor);
        for run in &owned[start..] {
            if run.logical >= end {
                break;
            }
            if cursor < run.logical {
                additions.push(Extent {
                    logical: cursor,
                    physical: Some(
                        physical
                            .checked_add(cursor - extent.logical)
                            .ok_or_else(|| corrupt("shared physical offset overflow"))?,
                    ),
                    length: run.logical - cursor,
                });
            }
            cursor = cursor.max(
                run.logical
                    .checked_add(run.length)
                    .ok_or_else(|| corrupt("owned extent overflow"))?,
            );
            if cursor >= end {
                break;
            }
        }
        if cursor < end {
            additions.push(Extent {
                logical: cursor,
                physical: Some(
                    physical
                        .checked_add(cursor - extent.logical)
                        .ok_or_else(|| corrupt("shared physical offset overflow"))?,
                ),
                length: end - cursor,
            });
        }
    }
    owned.extend(additions);
    owned.sort_by_key(|run| run.logical);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_versions_fill_only_gaps_and_preserve_written_and_zero_spans() -> Result<()> {
        let mut owned = vec![
            Extent {
                logical: 4,
                physical: Some(100),
                length: 4,
            },
            Extent {
                logical: 12,
                physical: None,
                length: 4,
            },
        ];
        fill_unmapped(
            &mut owned,
            &[Extent {
                logical: 0,
                physical: Some(200),
                length: 24,
            }],
            20,
        )?;
        fill_unmapped(
            &mut owned,
            &[Extent {
                logical: 0,
                physical: Some(300),
                length: 24,
            }],
            20,
        )?;
        assert_eq!(
            owned,
            vec![
                Extent {
                    logical: 0,
                    physical: Some(200),
                    length: 4
                },
                Extent {
                    logical: 4,
                    physical: Some(100),
                    length: 4
                },
                Extent {
                    logical: 8,
                    physical: Some(208),
                    length: 4
                },
                Extent {
                    logical: 12,
                    physical: None,
                    length: 4
                },
                Extent {
                    logical: 16,
                    physical: Some(216),
                    length: 4
                },
            ]
        );
        assert!(fill_unmapped(
            &mut owned,
            &[Extent {
                logical: u64::MAX,
                physical: Some(0),
                length: 1
            }],
            20
        )
        .is_err());
        Ok(())
    }
}
