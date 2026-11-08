//! 还原 Windows 源系统给各卷分配的**原始盘符**（C:、D:…）。
//!
//! 盘符不在分区表或文件系统里，而在注册表 `HKLM\SYSTEM\MountedDevices`：每个
//! `\DosDevices\X:` 值是一段 REG_BINARY，把盘符映射到一个卷标识——
//!
//! - **MBR / 基本盘**：恰 12 字节 = 4 字节 MBR 磁盘签名（磁盘偏移 0x1B8）
//!   + 8 字节分区起始字节偏移（小端）。
//! - **GPT 盘**：含 `DMIO:ID:`（**ASCII**，8 字节）前缀，其后是 16 字节
//!   GPT 分区**唯一** GUID（磁盘混合端序）。
//! - `\??\Volume{…}` 等非盘符项跳过。
//!
//! 本模块读 SYSTEM 配置单元、解出这些值，再按「签名 + 偏移」（MBR）或
//! 「唯一 GUID」（GPT）关联到本镜像的分区，返回 `分区 id → 盘符`。配置单元经
//! 与 [`crate::analysis::identify::os_detect`] 同款的
//! `fs_tree` 路径解析 + 限额读取读出，天然覆盖 NTFS / 已解锁 BitLocker——
//! Windows 的 SYSTEM 配置单元只可能落在这类卷上。
//!
//! 关联的**目标**卷可以是镜像里的任意分区（数据盘也有盘符），与能否读取其
//! 文件系统无关——匹配只看签名/偏移/GUID。指向别的物理盘（签名不符 / GUID
//! 不在本镜像）的盘符静默丢弃。

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use anyhow::Context;
use notatin::cell_value::CellValue;
use notatin::filter::FilterBuilder;
use notatin::parser::ParserIterator;
use notatin::parser_builder::ParserBuilder;
use serde::{Deserialize, Serialize};

use crate::evidence::partitions::gpt::uuid_from_disk_bytes;
use crate::evidence::partitions::{PartitionKind, TableScheme};
use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree;

/// SYSTEM 配置单元在真实安装上通常 10–40 MiB；与 `os_detect`
/// 同量级上限，留足让 notatin 跟随 cell 偏移。
const SYSTEM_READ_CAP: usize = 64 * 1024 * 1024;
const SYSTEM_HIVE_PATH: &str = "/Windows/System32/config/SYSTEM";
const MOUNTED_DEVICES_KEY: &str = "MountedDevices";
/// MBR 磁盘签名：磁盘偏移 0x1B8（440）起 4 字节。
const MBR_SIGNATURE_OFFSET: u64 = 0x1B8;
/// GPT 盘的 MountedDevices 值前缀 `DMIO:ID:`（ASCII，8 字节），其后紧跟
/// 16 字节 GPT 分区唯一 GUID（磁盘混合端序）。在真实 GPT 镜像上实测确认。
const DMIO_MARKER: &[u8] = b"DMIO:ID:";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriveLetterAssignment {
    pub partition_id: u8,
    /// 形如 `"C:"`——始终带尾冒号、大写。
    pub drive_letter: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriveLetterReport {
    pub assignments: Vec<DriveLetterAssignment>,
    /// 多个 Windows 安装的 MountedDevices 对同一分区给出不同盘符，或同一盘符
    /// 指向两个分区（双系统 / 重装）时为 true；UI 给盘符加 `?` 提示歧义。
    pub ambiguous: bool,
}

/// 一条 `\DosDevices\X:` 值解出的卷指向。
#[derive(Debug, Clone, PartialEq, Eq)]
enum DecodedBlob {
    /// MBR / 基本盘：磁盘签名 + 分区起始字节偏移。
    Mbr { signature: [u8; 4], start_byte: u64 },
    /// GPT 盘：分区唯一 GUID 的原始 16 字节（字节序在关联时两解释都试）。
    Gpt { raw_guid: [u8; 16] },
}

/// 解析镜像所有 Windows 卷的 SYSTEM 配置单元，还原 `分区 id → 原始盘符`。
/// 失败（无 Windows 卷 / 上锁 / 无 MountedDevices）时返回空 assignments，
/// 调用方据此不显示徽标，不影响浏览。
pub fn read_image_drive_letters(image: &Arc<EvidenceSession>) -> DriveLetterReport {
    // ── 关联键（覆盖镜像内**全部**分区，与可读性无关）──
    let mbr_signature = read_mbr_signature(image);
    let sector = image.sector_size as u64;
    let offset_map: Vec<(u64, u8)> = image
        .partitions
        .iter()
        .map(|p| (p.start_lba.saturating_mul(sector), p.id))
        .collect();
    let mut guid_map: HashMap<[u8; 16], u8> = HashMap::new();
    for p in &image.partitions {
        if let Some(guid) = p.unique_guid {
            guid_map.entry(guid).or_insert(p.id);
        }
    }

    // ── 扫描候选卷上的 SYSTEM 配置单元，合并 MountedDevices 结果 ──
    let mut by_partition: HashMap<u8, String> = HashMap::new();
    let mut by_letter: HashMap<String, u8> = HashMap::new();
    let mut ambiguous = false;

    for partition in &image.partitions {
        if !is_candidate_partition(partition.kind, image, partition.id) {
            continue;
        }
        let root_id = format!("part:{}", partition.id);
        let hive = match read_file(image, &root_id, SYSTEM_HIVE_PATH) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => continue,
            Err(err) => {
                tracing::debug!(
                    "win_drive_letters: SYSTEM read failed on partition {}: {err}",
                    partition.id
                );
                continue;
            }
        };
        let entries = match parse_mounted_devices(&hive) {
            Ok(entries) => entries,
            Err(err) => {
                tracing::debug!(
                    "win_drive_letters: MountedDevices parse failed on partition {}: {err}",
                    partition.id
                );
                continue;
            }
        };
        for (letter, blob) in entries {
            let Some(part_id) = correlate(&blob, mbr_signature, &offset_map, &guid_map) else {
                continue; // 指向别的物理盘或本镜像没有的卷
            };
            // 首个写入者优先；跨配置单元的冲突（同分区不同字母 / 同字母不同
            // 分区）置歧义，但不覆盖已确定的赋值。
            let part_conflict = by_partition.get(&part_id).is_some_and(|l| *l != letter);
            let letter_conflict = by_letter.get(&letter).is_some_and(|p| *p != part_id);
            if part_conflict || letter_conflict {
                ambiguous = true;
                continue;
            }
            by_partition
                .entry(part_id)
                .or_insert_with(|| letter.clone());
            by_letter.entry(letter).or_insert(part_id);
        }
    }

    let mut assignments: Vec<DriveLetterAssignment> = by_partition
        .into_iter()
        .map(|(partition_id, drive_letter)| DriveLetterAssignment {
            partition_id,
            drive_letter,
        })
        .collect();
    assignments.sort_by_key(|a| a.partition_id);
    DriveLetterReport {
        assignments,
        ambiguous,
    }
}

/// MBR 磁盘签名，仅当本盘是 MBR 表时有意义（GPT 的保护性 MBR 在 0x1B8 多为
/// 全 0，无法区分多盘，视为无效）。
fn read_mbr_signature(image: &Arc<EvidenceSession>) -> Option<[u8; 4]> {
    if image.table_scheme != Some(TableScheme::Mbr) {
        return None;
    }
    let bytes = image.read_range(MBR_SIGNATURE_OFFSET, 4).ok()?;
    let arr: [u8; 4] = bytes.as_slice().try_into().ok()?;
    (arr != [0, 0, 0, 0]).then_some(arr)
}

fn is_candidate_partition(
    kind: PartitionKind,
    image: &Arc<EvidenceSession>,
    partition_id: u8,
) -> bool {
    match kind {
        PartitionKind::Ntfs => true,
        PartitionKind::Bitlocker => image.bitlocker_unlocked(partition_id).is_some(),
        _ => false,
    }
}

/// 按 POSIX 路径在卷里逐段解析（大小写不敏感），读出文件内容。与
/// 注册表文件读取限额同 [`SYSTEM_READ_CAP`]。
fn read_file(
    image: &Arc<EvidenceSession>,
    root_id: &str,
    path: &str,
) -> anyhow::Result<Option<Vec<u8>>> {
    let components = path.split('/').filter(|s| !s.is_empty());
    let mut current_id = root_id.to_string();
    let mut current_kind = None;
    for component in components {
        let children = match fs_tree::children_for_image(image, &current_id) {
            Ok(c) => c,
            Err(err) => {
                tracing::debug!("win_drive_letters.children({current_id}) failed: {err}");
                return Ok(None);
            }
        };
        let Some(found) = children
            .into_iter()
            .find(|node| node.name.eq_ignore_ascii_case(component))
        else {
            return Ok(None);
        };
        current_id = found.id;
        current_kind = Some(found.kind);
    }
    if !matches!(current_kind, Some(fs_tree::NodeKind::File)) {
        return Ok(None);
    }
    let bytes = fs_tree::read_file_limited(image, &current_id, SYSTEM_READ_CAP)
        .with_context(|| format!("read {path}"))?;
    Ok(Some(bytes))
}

/// 解析 `MountedDevices` 键，返回每个盘符及其解码后的卷指向。
fn parse_mounted_devices(hive: &[u8]) -> anyhow::Result<Vec<(String, DecodedBlob)>> {
    let filter = FilterBuilder::new()
        .add_key_path(MOUNTED_DEVICES_KEY)
        .return_child_keys(false)
        .build()
        .map_err(|err| anyhow::anyhow!("notatin filter: {err}"))?;
    let mut builder = ParserBuilder::from_file(Cursor::new(hive.to_vec()));
    builder.with_filter(filter);
    let parser = builder
        .build()
        .map_err(|err| anyhow::anyhow!("notatin parse: {err}"))?;

    let needle = MOUNTED_DEVICES_KEY.to_ascii_lowercase();
    let mut out = Vec::new();
    for key in ParserIterator::new(&parser).iter() {
        if !key.path.to_ascii_lowercase().ends_with(&needle) {
            continue;
        }
        for value in key.value_iter() {
            let Some(letter) = dos_device_letter(&value.get_pretty_name()) else {
                continue; // \??\Volume{…} 等非盘符项
            };
            if let (CellValue::Binary(bytes), _) = value.get_content() {
                if let Some(blob) = decode_blob(&bytes) {
                    out.push((letter, blob));
                }
            }
        }
    }
    Ok(out)
}

/// 从 MountedDevices 值名提取盘符：`\DosDevices\C:` → `Some("C:")`（规范成
/// 大写）；`\??\Volume{…}` 等返回 `None`。
fn dos_device_letter(name: &str) -> Option<String> {
    const PREFIX: &str = "\\DosDevices\\";
    let head = name.get(..PREFIX.len())?;
    if !head.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let tail = name.get(PREFIX.len()..)?.as_bytes();
    if tail.len() == 2 && tail[0].is_ascii_alphabetic() && tail[1] == b':' {
        Some(format!("{}:", tail[0].to_ascii_uppercase() as char))
    } else {
        None
    }
}

/// 解码一条 MountedDevices REG_BINARY 值。
fn decode_blob(bytes: &[u8]) -> Option<DecodedBlob> {
    // MBR / 基本盘：恰 12 字节 = 4 字节磁盘签名 + 8 字节起始字节偏移（LE）。
    if bytes.len() == 12 {
        let signature: [u8; 4] = bytes[0..4].try_into().ok()?;
        let start_byte = u64::from_le_bytes(bytes[4..12].try_into().ok()?);
        return Some(DecodedBlob::Mbr {
            signature,
            start_byte,
        });
    }
    // GPT 盘：定位 `DMIO:ID:`，其后 16 字节即分区唯一 GUID。（确切偏移在真实
    // 镜像上以日志核验过；find 容忍前缀填充。）
    if let Some(pos) = find_subslice(bytes, DMIO_MARKER) {
        let start = pos + DMIO_MARKER.len();
        let raw_guid: [u8; 16] = bytes.get(start..start + 16)?.try_into().ok()?;
        return Some(DecodedBlob::Gpt { raw_guid });
    }
    None
}

/// 把解码值关联到本镜像的某个分区 id；无法关联（别的物理盘 / 不存在）→ None。
fn correlate(
    blob: &DecodedBlob,
    mbr_signature: Option<[u8; 4]>,
    offset_map: &[(u64, u8)],
    guid_map: &HashMap<[u8; 16], u8>,
) -> Option<u8> {
    match blob {
        DecodedBlob::Mbr {
            signature,
            start_byte,
        } => {
            // 签名须与本盘一致，否则盘符属于另一物理盘。
            if mbr_signature != Some(*signature) {
                return None;
            }
            offset_map
                .iter()
                .find(|&&(off, _)| off == *start_byte)
                .map(|&(_, id)| id)
        }
        DecodedBlob::Gpt { raw_guid } => {
            // 分区表里存的是 canonical 字节；MountedDevices 多为磁盘混合端序。
            // 两种解释都查，避免字节序约定差异漏配（128 位 GUID 误配概率可忽略）。
            if let Some(id) = guid_map.get(raw_guid) {
                return Some(*id);
            }
            let canonical = uuid_from_disk_bytes(raw_guid)?;
            guid_map.get(canonical.as_bytes()).copied()
        }
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_device_letter_parses_and_uppercases() {
        assert_eq!(dos_device_letter("\\DosDevices\\C:").as_deref(), Some("C:"));
        assert_eq!(dos_device_letter("\\dosdevices\\d:").as_deref(), Some("D:"));
    }

    #[test]
    fn dos_device_letter_skips_volume_guids() {
        assert_eq!(
            dos_device_letter("\\??\\Volume{12345678-0000-0000-0000-000000000000}"),
            None
        );
        assert_eq!(dos_device_letter("\\DosDevices\\CD:"), None); // 非「字母+冒号」
        assert_eq!(dos_device_letter(""), None);
    }

    #[test]
    fn decode_mbr_blob() {
        let mut blob = Vec::new();
        blob.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // 签名
        blob.extend_from_slice(&1_048_576u64.to_le_bytes()); // 偏移 1 MiB
        assert_eq!(
            decode_blob(&blob),
            Some(DecodedBlob::Mbr {
                signature: [0xDE, 0xAD, 0xBE, 0xEF],
                start_byte: 1_048_576,
            })
        );
    }

    #[test]
    fn decode_gpt_blob_finds_guid_after_marker() {
        let guid = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
            0xFF, 0x00,
        ];
        let mut blob = b"DMIO:ID:".to_vec(); // ASCII 前缀（真实 GPT 镜像实测格式）
        blob.extend_from_slice(&guid);
        assert_eq!(blob.len(), 24);
        assert_eq!(
            decode_blob(&blob),
            Some(DecodedBlob::Gpt { raw_guid: guid })
        );
    }

    #[test]
    fn decode_blob_rejects_unknown_shape() {
        assert_eq!(decode_blob(&[0u8; 8]), None);
        assert_eq!(decode_blob(b"random bytes here"), None);
    }

    #[test]
    fn correlate_mbr_requires_matching_signature() {
        let offset_map = vec![(1_048_576u64, 3u8)];
        let guid_map = HashMap::new();
        let blob = DecodedBlob::Mbr {
            signature: [0xDE, 0xAD, 0xBE, 0xEF],
            start_byte: 1_048_576,
        };
        // 签名一致 → 命中。
        assert_eq!(
            correlate(
                &blob,
                Some([0xDE, 0xAD, 0xBE, 0xEF]),
                &offset_map,
                &guid_map
            ),
            Some(3)
        );
        // 签名不符（别的物理盘）→ 丢弃。
        assert_eq!(
            correlate(&blob, Some([0, 0, 0, 1]), &offset_map, &guid_map),
            None
        );
        // 偏移不在本盘 → 丢弃。
        let other = DecodedBlob::Mbr {
            signature: [0xDE, 0xAD, 0xBE, 0xEF],
            start_byte: 999,
        };
        assert_eq!(
            correlate(
                &other,
                Some([0xDE, 0xAD, 0xBE, 0xEF]),
                &offset_map,
                &guid_map
            ),
            None
        );
    }

    #[test]
    fn correlate_gpt_matches_canonical_or_raw() {
        let canonical = [
            0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0x00,
            0x12, 0x34,
        ];
        let mut guid_map = HashMap::new();
        guid_map.insert(canonical, 2u8);
        // 原样匹配。
        assert_eq!(
            correlate(
                &DecodedBlob::Gpt {
                    raw_guid: canonical
                },
                None,
                &[],
                &guid_map
            ),
            Some(2)
        );
        // 磁盘混合端序：原样查不到，但解码成 canonical 后命中。
        let on_disk = [
            0xDD, 0xCC, 0xBB, 0xAA, 0x22, 0x11, 0x44, 0x33, 0x55, 0x66, 0x77, 0x88, 0x99, 0x00,
            0x12, 0x34,
        ];
        assert_eq!(
            correlate(
                &DecodedBlob::Gpt { raw_guid: on_disk },
                None,
                &[],
                &guid_map
            ),
            Some(2)
        );
        // 未知 GUID → 丢弃。
        assert_eq!(
            correlate(
                &DecodedBlob::Gpt {
                    raw_guid: [0u8; 16]
                },
                None,
                &[],
                &guid_map
            ),
            None
        );
    }

    /// Env-gated 实测：把 `LOVELYCARVE_DRIVE_LETTERS_TEST` 设为某个真实 Windows
    /// 镜像（raw/dd/E01/VMDK）路径，断言至少解出一个盘符且包含 `C:`，并打印
    /// 全部 `分区 id → 盘符`（`-- --nocapture`）便于核对 MBR/GPT 解码。
    #[test]
    fn recovers_drive_letters_on_real_image_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_DRIVE_LETTERS_TEST") else {
            return;
        };
        let image = Arc::new(
            crate::filesystem::evidence_session::EvidenceSession::open(path)
                .expect("Windows test image should open"),
        );
        let report = read_image_drive_letters(&image);
        println!("drive letters: {report:#?}");
        assert!(
            report.assignments.iter().any(|a| a.drive_letter == "C:"),
            "expected a C: assignment; got: {report:#?}"
        );
    }
}
