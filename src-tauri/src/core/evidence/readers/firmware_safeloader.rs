//! TP-Link SafeLoader's salted MD5 and bounded image (not flash) partition table.
use super::{be32, segments};
use crate::core::common::hash::Md5;
use crate::core::evidence::partitions::Partition;
use anyhow::{ensure, Context, Result};
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

const TABLE: usize = 0x1014;
const TABLE_SIZE: usize = 0x800;
const SALT: [u8; 16] = [
    0x7a, 0x2b, 0x15, 0xed, 0x9b, 0x98, 0x59, 0x6d, 0xe5, 0x04, 0xab, 0x44, 0xac, 0x2a, 0x9f, 0x4e,
];

pub(super) fn has_magic(bytes: &[u8]) -> bool {
    bytes.get(TABLE..TABLE + 9) == Some(b"fwup-ptn ") || bytes.get(0x14..0x18) == Some(b"?NEW")
}

pub(super) fn parse(
    bytes: &[u8],
    base: u64,
    cancel: &CancellationToken,
    depth: usize,
) -> Result<Vec<Partition>> {
    ensure!(
        bytes.get(0x14..0x18) != Some(b"?NEW"),
        "unsupported SafeLoader QNEW layout"
    );
    let length = be32(bytes, 0)? as usize;
    ensure!(length == bytes.len(), "SafeLoader declared length mismatch");
    let table = bytes
        .get(TABLE..TABLE + TABLE_SIZE)
        .context("truncated SafeLoader table")?;
    let mut digest = Md5::new();
    digest.update(&SALT);
    for chunk in bytes[0x14..].chunks(64 * 1024) {
        ensure!(!cancel.is_cancelled(), "SafeLoader checksum cancelled");
        digest.update(chunk);
    }
    ensure!(
        digest.finalize().as_slice() == &bytes[4..20],
        "SafeLoader salted MD5 mismatch"
    );
    let terminator = table
        .iter()
        .position(|byte| *byte == 0)
        .context("unterminated SafeLoader table")?;
    ensure!(
        table[terminator + 1..]
            .iter()
            .all(|byte| matches!(byte, 0 | 0xff)),
        "SafeLoader table has data after terminator"
    );
    let text = std::str::from_utf8(&table[..terminator])?;
    let mut names = HashSet::new();
    let mut spans = Vec::new();
    for line in text.split_terminator('\n') {
        ensure!(spans.len() < 32, "SafeLoader segment limit");
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        ensure!(
            fields.len() == 6
                && fields[0] == "fwup-ptn"
                && fields[2] == "base"
                && fields[4] == "size",
            "malformed SafeLoader segment"
        );
        let name = fields[1];
        ensure!(
            !name.is_empty()
                && name.len() <= 32
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                && names.insert(name),
            "duplicate or invalid SafeLoader segment name"
        );
        let number = |text: &str| -> Result<usize> {
            usize::from_str_radix(
                text.strip_prefix("0x")
                    .context("SafeLoader field is not hex")?,
                16,
            )
            .context("invalid SafeLoader hex field")
        };
        let relative = number(fields[3])?;
        let size = number(fields[5])?;
        ensure!(
            relative >= TABLE_SIZE && size != 0,
            "SafeLoader segment overlaps table or is empty"
        );
        let start = TABLE
            .checked_add(relative)
            .context("SafeLoader segment offset overflow")?;
        let end = start
            .checked_add(size)
            .context("SafeLoader segment size overflow")?;
        ensure!(end <= bytes.len(), "SafeLoader segment exceeds source");
        spans.push((start, end, name));
    }
    ensure!(
        names.contains("partition-table") && names.contains("support-list"),
        "SafeLoader required metadata missing"
    );
    let mut ordered = spans.clone();
    ordered.sort_unstable_by_key(|span| span.0);
    ensure!(
        ordered.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "SafeLoader segments overlap"
    );
    let mut partitions = Vec::new();
    for (start, end, name) in spans {
        let mut found = segments::parse_payload(
            &bytes[start..end],
            segments::source_offset(base, start)?,
            cancel,
            depth + 1,
        )?;
        for partition in &mut found {
            partition.label = Some(format!("SafeLoader {name}"));
        }
        segments::append_partitions(&mut partitions, found)?;
    }
    Ok(partitions)
}
