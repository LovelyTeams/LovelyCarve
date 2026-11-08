//! 原始盘符还原结果的持久化(案件台账 `drive_letter` 表)。
//!
//! 还原盘符要读镜像上的 SYSTEM 配置单元并解析 `MountedDevices`(见
//! [`crate::analysis::identify::win_drive_letters`]),每次浏览都做一遍代价不小;证据不可变,
//! 结论是确定值,所以有案件时缓存进台账,下次直接返回。只缓存**非空**报告:
//! 空结果(不是 Windows、BitLocker 仍上锁、没有 `MountedDevices`)要保持可
//! 重算,解锁后才能得到真正的盘符。没有案件时只做会话内实时解析。

use std::path::Path;

use crate::analysis::identify::win_drive_letters::DriveLetterReport;
use crate::casework::case_ledger;
use crate::casework::case_ledger_evidence;
use crate::casework::case_paths;

/// 已缓存的报告;没有缓存或检材不在案件中时为 `None`。
pub fn load(case_dir: &Path, image_id: &str) -> anyhow::Result<Option<DriveLetterReport>> {
    let conn = case_ledger::open(&case_paths::ledger_path(case_dir))?;
    case_ledger_evidence::drive_letters(&conn, image_id)
}

/// 缓存一份报告(整体替换);空报告清除缓存。检材不在案件中时返回 `false`。
pub fn save(case_dir: &Path, image_id: &str, report: &DriveLetterReport) -> anyhow::Result<bool> {
    let conn = case_ledger::open(&case_paths::ledger_path(case_dir))?;
    let tx = case_ledger::write_transaction(&conn)?;
    let saved = case_ledger_evidence::replace_drive_letters(&tx, image_id, report)?;
    tx.commit()?;
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::identify::win_drive_letters::DriveLetterAssignment;
    use crate::casework::case_ledger::test_support;
    use crate::casework::case_ledger_evidence::test_support::add;

    fn report(letter: &str, ambiguous: bool) -> DriveLetterReport {
        DriveLetterReport {
            assignments: vec![DriveLetterAssignment {
                partition_id: 1,
                drive_letter: letter.to_string(),
            }],
            ambiguous,
        }
    }

    #[test]
    fn save_replaces_and_empty_clears() {
        let dir = tempfile::tempdir().expect("tempdir");
        add(&test_support::ledger(dir.path()), "img");
        assert!(load(dir.path(), "img").expect("load").is_none());
        assert!(save(dir.path(), "img", &report("C:", false)).expect("save"));
        assert!(save(dir.path(), "img", &report("D:", true)).expect("replace"));
        let loaded = load(dir.path(), "img").expect("load").expect("cached");
        assert_eq!(loaded.assignments.len(), 1);
        assert_eq!(loaded.assignments[0].drive_letter, "D:");
        assert!(loaded.ambiguous);
        let empty = DriveLetterReport {
            assignments: Vec::new(),
            ambiguous: false,
        };
        assert!(save(dir.path(), "img", &empty).expect("clear"));
        assert!(load(dir.path(), "img").expect("load").is_none());
        assert!(!save(dir.path(), "other", &report("C:", false)).expect("unknown"));
    }
}
