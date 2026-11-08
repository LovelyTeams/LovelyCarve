//! Validate the supported LUKS layout before running any password derivation.

use std::io::{Read, Seek, SeekFrom};

use anyhow::{bail, ensure, Context};
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha512};

fn number(value: &Value) -> anyhow::Result<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .context("invalid LUKS numeric metadata")
}

pub fn validate(reader: &mut (impl Read + Seek), length: u64) -> anyhow::Result<()> {
    let mut prefix = [0; 592];
    reader.read_exact(&mut prefix)?;
    ensure!(&prefix[..6] == b"LUKS\xba\xbe", "not a LUKS container");
    let version = u16::from_be_bytes([prefix[6], prefix[7]]);
    match version {
        1 => {
            let header = luks::Luks1Header::parse(&prefix)?;
            luks::validate_cipher(
                &header.cipher_name,
                &header.cipher_mode,
                header.key_bytes as usize,
            )?;
            ensure!(header.mk_digest_iter > 0, "invalid LUKS1 digest iterations");
            let offset = header.payload_byte_offset();
            ensure!(
                offset >= 592 && offset < length && (length - offset).is_multiple_of(512),
                "invalid LUKS1 payload range"
            );
            for slot in header.active_keyslots() {
                let start = u64::from(slot.key_material_offset) * 512;
                let bytes =
                    (u64::from(slot.stripes) * u64::from(header.key_bytes)).div_ceil(512) * 512;
                ensure!(
                    bytes > 0
                        && bytes <= 64 * 1024 * 1024
                        && slot.stripes > 0
                        && slot.iterations > 0
                        && start >= 592
                        && start.checked_add(bytes).is_some_and(|end| end <= offset),
                    "invalid LUKS1 keyslot range"
                );
            }
        }
        2 => validate_luks2(reader, &prefix, length)?,
        _ => bail!("unsupported LUKS version {version}"),
    }
    Ok(())
}

fn validate_luks2(
    reader: &mut (impl Read + Seek),
    prefix: &[u8],
    length: u64,
) -> anyhow::Result<()> {
    let declared = u64::from_be_bytes(prefix[8..16].try_into()?);
    ensure!(
        (8192..=4 * 1024 * 1024).contains(&declared)
            && declared.is_power_of_two()
            && declared <= length,
        "invalid LUKS2 header length"
    );
    let mut header = vec![0; declared as usize];
    reader.seek(SeekFrom::Start(0))?;
    reader.read_exact(&mut header)?;
    let checksum = header[448..512].to_vec();
    header[448..512].fill(0);
    let algorithm = header[72..104]
        .split(|byte| *byte == 0)
        .next()
        .context("missing LUKS2 checksum algorithm")?;
    let digest = match algorithm {
        b"sha256" => Sha256::digest(&header).to_vec(),
        b"sha512" => Sha512::digest(&header).to_vec(),
        _ => bail!("unsupported LUKS2 header checksum"),
    };
    ensure!(
        digest == checksum[..digest.len()],
        "LUKS2 primary header checksum mismatch; backup recovery is not supported"
    );
    let json = header[4096..]
        .split(|byte| *byte == 0)
        .next()
        .context("missing LUKS2 JSON")?;
    let metadata: Value = serde_json::from_slice(json)?;
    ensure!(
        metadata["config"]["requirements"]["mandatory"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "LUKS2 mandatory requirements are unsupported"
    );
    let segments = metadata["segments"]
        .as_object()
        .context("missing LUKS2 segments")?;
    ensure!(
        segments.len() == 1,
        "LUKS2 multi-segment/reencryption layouts are unsupported"
    );
    let (segment_id, segment) = segments.iter().next().context("missing LUKS2 segment")?;
    ensure!(
        segment["type"] == "crypt" && segment["size"] == "dynamic",
        "only a single dynamic LUKS2 crypt segment is supported"
    );
    ensure!(
        segment["integrity"].is_null() && segment["flags"].as_array().is_none_or(Vec::is_empty),
        "LUKS2 integrity/reencryption flags are unsupported"
    );
    let offset = number(&segment["offset"])?;
    let encryption = segment["encryption"]
        .as_str()
        .context("missing LUKS2 encryption")?;
    let (cipher, mode) = encryption
        .split_once('-')
        .context("invalid LUKS2 encryption")?;
    let sector = number(&segment["sector_size"])?;
    ensure!(
        matches!(sector, 512 | 4096)
            && offset >= declared * 2
            && offset < length
            && offset % sector == 0
            && (length - offset).is_multiple_of(sector),
        "invalid LUKS2 payload geometry"
    );
    let tweak = number(&segment["iv_tweak"])?;
    ensure!(
        tweak
            .checked_add((length - offset - sector) / 512)
            .is_some(),
        "LUKS2 IV sector range overflow"
    );
    let slots = metadata["keyslots"]
        .as_object()
        .context("missing LUKS2 keyslots")?;
    ensure!(slots.len() <= 32, "invalid LUKS2 keyslot count");
    for slot in slots.values() {
        let kdf = &slot["kdf"];
        let salt = base64::engine::general_purpose::STANDARD
            .decode(kdf["salt"].as_str().context("missing LUKS2 KDF salt")?)?;
        ensure!((16..=64).contains(&salt.len()), "invalid LUKS2 KDF salt");
        let fields: &[&str] = match kdf["type"].as_str() {
            Some("pbkdf2") => &["iterations"],
            Some("argon2i" | "argon2id") => &["time", "memory", "cpus"],
            _ => bail!("unsupported LUKS2 KDF"),
        };
        for field in fields {
            ensure!(
                (1..=u32::MAX as u64).contains(&number(&kdf[*field])?),
                "invalid LUKS2 KDF cost"
            );
        }
        let key_size = number(&slot["key_size"])?;
        let area = &slot["area"];
        let start = number(&area["offset"])?;
        let area_size = number(&area["size"])?;
        let stripes = number(&slot["af"]["stripes"])?;
        let material = stripes
            .checked_mul(key_size)
            .and_then(|bytes| bytes.checked_add(511))
            .context("LUKS2 keyslot size overflow")?
            / 512
            * 512;
        ensure!(
            slot["type"] == "luks2" && slot["af"]["type"] == "luks1" && area["type"] == "raw",
            "unsupported LUKS2 keyslot layout"
        );
        luks::validate_cipher(cipher, mode, usize::try_from(key_size)?)?;
        let area_encryption = area["encryption"]
            .as_str()
            .context("missing LUKS2 area encryption")?;
        let (area_cipher, area_mode) = area_encryption
            .split_once('-')
            .context("invalid LUKS2 area encryption")?;
        luks::validate_cipher(
            area_cipher,
            area_mode,
            usize::try_from(number(&area["key_size"])?)?,
        )?;
        ensure!(
            material > 0
                && stripes > 0
                && material <= 64 * 1024 * 1024
                && material <= area_size
                && start % 512 == 0
                && start >= declared * 2
                && start
                    .checked_add(area_size)
                    .is_some_and(|end| end <= offset),
            "invalid LUKS2 keyslot area"
        );
    }
    let digests = metadata["digests"]
        .as_object()
        .context("missing LUKS2 digests")?;
    ensure!(!digests.is_empty(), "missing LUKS2 key digest");
    for digest in digests.values() {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(digest["digest"].as_str().context("missing LUKS2 digest")?)?;
        let salt = base64::engine::general_purpose::STANDARD.decode(
            digest["salt"]
                .as_str()
                .context("missing LUKS2 digest salt")?,
        )?;
        ensure!(
            (20..=64).contains(&decoded.len())
                && (16..=64).contains(&salt.len())
                && (1..=u32::MAX as u64).contains(&number(&digest["iterations"])?),
            "invalid LUKS2 key digest"
        );
        ensure!(
            digest["type"] == "pbkdf2"
                && digest["segments"]
                    .as_array()
                    .is_some_and(|ids| ids.len() == 1 && ids[0].as_str() == Some(segment_id)),
            "unbound LUKS2 key digests are unsupported"
        );
    }
    Ok(())
}
