//! 源读故障审计的持久化(案件台账 `read_fault` 表)。
//!
//! 容错读取会为读不出的扇区合成零字节;镜像在内存里登记这些区间,本模块把
//! 它们合并进台账,关闭再打开案件后这份事实仍在。区间按偏移归一、合并。

use std::path::Path;

use crate::casework::case_ledger;
use crate::casework::case_ledger_evidence;
use crate::casework::case_paths;
use crate::evidence::image::{BadRegion, ReadFaultReport};

/// 检材已持久化的读故障报告。
pub fn load(case_dir: &Path, image_id: &str) -> anyhow::Result<ReadFaultReport> {
    let conn = case_ledger::open(&case_paths::ledger_path(case_dir))?;
    Ok(ReadFaultReport::from_regions(
        case_ledger_evidence::read_faults(&conn, image_id)?,
    ))
}

/// 把新观察到的区间并入已持久化的记录并返回合并后的报告。检材不在案件中时
/// 不落库,只返回合并结果。
pub fn merge_save(
    case_dir: &Path,
    image_id: &str,
    new_regions: Vec<BadRegion>,
) -> anyhow::Result<ReadFaultReport> {
    let conn = case_ledger::open(&case_paths::ledger_path(case_dir))?;
    let tx = case_ledger::write_transaction(&conn)?;
    let mut regions = case_ledger_evidence::read_faults(&tx, image_id)?;
    regions.extend(new_regions);
    let report = ReadFaultReport::from_regions(regions);
    case_ledger_evidence::replace_read_faults(&tx, image_id, &report.bad_regions)?;
    tx.commit()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::casework::case_ledger::test_support;
    use crate::casework::case_ledger_evidence::test_support::add;

    #[test]
    fn merge_save_round_trips_and_coalesces() {
        let dir = tempfile::tempdir().expect("tempdir");
        add(&test_support::ledger(dir.path()), "img");
        merge_save(
            dir.path(),
            "img",
            vec![BadRegion {
                offset: 0,
                len: 512,
            }],
        )
        .expect("first save");
        let report = merge_save(
            dir.path(),
            "img",
            vec![BadRegion {
                offset: 512,
                len: 512,
            }],
        )
        .expect("second save");
        assert_eq!(report.synthetic_bytes, 1024);
        assert_eq!(report.bad_regions.len(), 1);
        assert_eq!(load(dir.path(), "img").expect("load"), report);
    }
}
