//! FVE metadata block, header, and entry parsing.
//!
//! A BitLocker volume carries three copies of an FVE metadata block. Each block
//! is a `-FVE-FS-` block header, a 48-byte metadata header (cipher, volume GUID,
//! creation time), and a recursive array of metadata entries — the key
//! protectors (VMK), the FVEK, and the volume-header-block descriptor. See
//! `docs/RESEARCH.md`.

use crate::bytes::{le_u16, le_u32, le_u64, read_guid, slice_owned};

const FVE_SIGNATURE: &[u8; 8] = b"-FVE-FS-";

/// Metadata-entry type: a Volume Master Key protector.
pub const ENTRY_TYPE_VMK: u16 = 0x0002;
/// Metadata-entry type: the Full Volume Encryption Key.
pub const ENTRY_TYPE_FVEK: u16 = 0x0003;
/// Metadata-entry type / value type: the volume-header block descriptor.
pub const ENTRY_TYPE_VOLUME_HEADER: u16 = 0x000f;

/// Value type: a key (`method(u32)@0` then the raw key data). Holds the clear
/// key inside a clear-key VMK protector, and the external key inside a `.BEK`.
pub const VALUE_TYPE_KEY: u16 = 0x0001;
/// Value type: an AES-CCM encrypted key.
pub const VALUE_TYPE_AES_CCM: u16 = 0x0005;
/// Value type: an external key (the payload of a startup-key `.BEK` file: a
/// 16-byte key GUID, an 8-byte FILETIME, then nested entries holding the key).
pub const VALUE_TYPE_EXTERNAL_KEY: u16 = 0x0009;
/// Value type: a stretch key (salt + nested AES-CCM key).
pub const VALUE_TYPE_STRETCH: u16 = 0x0003;
/// Value type: a Volume Master Key protector.
pub const VALUE_TYPE_VMK: u16 = 0x0008;

/// Key-protection type: password.
pub const PROTECTION_PASSWORD: u16 = 0x2000;
/// Key-protection type: recovery password (the 48-digit numeric key).
pub const PROTECTION_RECOVERY: u16 = 0x0800;
/// Key-protection type: startup key (an external `.BEK` key on removable media).
/// The VMK is AES-CCM-wrapped directly by the 256-bit external key — no stretch.
pub const PROTECTION_STARTUP_KEY: u16 = 0x0200;
/// Key-protection type: clear key. The VMK is stored unprotected (BitLocker adds
/// this when protection is *suspended*), so the volume needs no credential.
pub const PROTECTION_CLEAR: u16 = 0x0000;

/// One FVE metadata entry (`entry_type`, `value_type`, `version`, and the raw
/// value data that follows the 8-byte entry header).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataEntry {
    /// Entry type (e.g. `ENTRY_TYPE_VMK`).
    pub entry_type: u16,
    /// Value type (e.g. `VALUE_TYPE_AES_CCM`).
    pub value_type: u16,
    /// Entry version (typically 1).
    pub version: u16,
    /// The value data following the 8-byte entry header.
    pub data: Vec<u8>,
}

impl MetadataEntry {
    /// Parse a flat sequence of metadata entries from `data`.
    ///
    /// Each entry is `size(u16) | entry_type(u16) | value_type(u16) |
    /// version(u16) | value_data`. Parsing stops on a size below the 8-byte
    /// header, an entry that would run past the buffer, or the end of `data` —
    /// never looping forever on a lying size.
    #[must_use]
    pub fn parse_sequence(data: &[u8]) -> Vec<MetadataEntry> {
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos + 8 <= data.len() {
            let size = le_u16(data, pos) as usize;
            if size < 8 || pos + size > data.len() {
                break;
            }
            out.push(MetadataEntry {
                entry_type: le_u16(data, pos + 2),
                value_type: le_u16(data, pos + 4),
                version: le_u16(data, pos + 6),
                data: slice_owned(data, pos + 8, size - 8),
            });
            pos += size;
        }
        out
    }

    /// Parse this entry's value data (from `offset`) as a nested entry sequence.
    #[must_use]
    pub fn nested(&self, offset: usize) -> Vec<MetadataEntry> {
        let start = offset.min(self.data.len());
        Self::parse_sequence(&self.data[start..])
    }

    /// Whether this entry is a VMK protector.
    #[must_use]
    pub fn is_vmk(&self) -> bool {
        self.entry_type == ENTRY_TYPE_VMK && self.value_type == VALUE_TYPE_VMK
    }

    /// The key-protection type of a VMK entry (protector type at value offset 26),
    /// or `None` for a non-VMK entry.
    #[must_use]
    pub fn protection_type(&self) -> Option<u16> {
        (self.is_vmk() && self.data.len() >= 28).then(|| le_u16(&self.data, 26))
    }
}

/// A parsed FVE metadata block: cipher, identity, and the entry array, plus the
/// block-header fields the read path needs (encrypted size and the relocated
/// volume-header region).
#[derive(Debug, Clone)]
pub struct FveMetadata {
    /// Volume encryption method (metadata header offset 36).
    pub encryption_method: u16,
    /// Volume identifier GUID (metadata header offset 16).
    pub volume_guid: [u8; 16],
    /// Volume creation time as a Windows FILETIME (metadata header offset 40).
    pub creation_time: u64,
    /// The metadata entries.
    pub entries: Vec<MetadataEntry>,
    /// Number of still-encrypted bytes from the front (block header, v2 offset 16).
    /// Zero means "whole volume".
    pub encrypted_volume_size: u64,
    /// Byte offset where the original volume header is stored, relocated.
    pub volume_header_offset: u64,
    /// Size in bytes of the relocated volume-header region.
    pub volume_header_size: u64,
    /// Byte offsets of the three metadata blocks (read back as zeros).
    pub metadata_offsets: [u64; 3],
    /// Size of the metadata region (metadata header offset 0).
    pub metadata_size: u32,
    /// Block layout version (1 = Vista, 2 = Windows 7 and later).
    pub version: u16,
    /// Vista's original NTFS MFT mirror cluster, overwritten in the boot sector.
    pub mft_mirror_cluster: u64,
}

impl FveMetadata {
    /// Parse an FVE metadata block from bytes beginning at its block header.
    ///
    /// Returns `None` when the `-FVE-FS-` block-header signature is absent, so
    /// the caller can try the next candidate block offset.
    #[must_use]
    pub fn parse(block: &[u8], bytes_per_sector: u16) -> Option<FveMetadata> {
        if block.get(0..8) != Some(FVE_SIGNATURE.as_slice()) {
            return None;
        }
        let version = le_u16(block, 10);
        if block.len() < 112 || !matches!(version, 1 | 2) {
            return None;
        }

        let encrypted_volume_size = le_u64(block, 16);
        let num_volume_header_sectors = le_u32(block, 28);
        let metadata_offsets = [le_u64(block, 32), le_u64(block, 40), le_u64(block, 48)];
        let block_volume_header_offset = le_u64(block, 56);

        // FVE metadata header starts at block offset 64.
        let mh = 64usize;
        let metadata_size = le_u32(block, mh);
        if metadata_size < 48 || mh.checked_add(metadata_size as usize)? > block.len() {
            return None;
        }
        let volume_guid = read_guid(block, mh + 16);
        let encryption_method = le_u16(block, mh + 36);
        let creation_time = le_u64(block, mh + 40);

        // Entries follow the 48-byte metadata header, bounded by metadata_size.
        let entries_start = mh + 48;
        let entries_end = mh + metadata_size as usize;
        let entries = if entries_end > entries_start {
            MetadataEntry::parse_sequence(&block[entries_start..entries_end])
        } else {
            Vec::new()
        };

        if entries
            .iter()
            .map(|entry| entry.data.len() + 8)
            .sum::<usize>()
            != entries_end - entries_start
        {
            return None;
        }
        // Resolve the relocated volume-header region: prefer the dedicated
        // volume-header-block entry (type 0x000f), else the block-header fields.
        let mut volume_header_offset = block_volume_header_offset;
        let mut volume_header_size =
            u64::from(num_volume_header_sectors) * u64::from(bytes_per_sector);
        if let Some(e) = entries
            .iter()
            .find(|e| e.entry_type == ENTRY_TYPE_VOLUME_HEADER)
        {
            let bo = le_u64(&e.data, 0);
            let bs = le_u64(&e.data, 8);
            if bo != 0 {
                volume_header_offset = bo;
            }
            if bs != 0 {
                volume_header_size = bs;
            }
        }

        Some(FveMetadata {
            version,
            mft_mirror_cluster: if version == 1 { le_u64(block, 56) } else { 0 },
            encryption_method,
            volume_guid,
            creation_time,
            entries,
            encrypted_volume_size: if version == 1 {
                0
            } else {
                encrypted_volume_size
            },
            volume_header_offset: if version == 1 {
                0
            } else {
                volume_header_offset
            },
            volume_header_size: if version == 1 { 0 } else { volume_header_size },
            metadata_offsets,
            metadata_size,
        })
    }

    /// Iterate the VMK protector entries.
    pub fn vmk_entries(&self) -> impl Iterator<Item = &MetadataEntry> {
        self.entries.iter().filter(|e| e.is_vmk())
    }

    /// The key-protection types present, in metadata order.
    #[must_use]
    pub fn protector_types(&self) -> Vec<u16> {
        self.vmk_entries()
            .filter_map(MetadataEntry::protection_type)
            .collect()
    }

    /// Volume description (entry 7, UTF-16LE), without losing invalid encoding.
    pub fn description(&self) -> crate::Result<Option<String>> {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.entry_type == 7 && entry.value_type == 2)
        else {
            return Ok(None);
        };
        if entry.data.len() % 2 != 0 {
            return Err(crate::BdeError::InvalidData(
                "odd UTF-16 description length",
            ));
        }
        let units: Vec<u16> = entry
            .data
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect();
        String::from_utf16(&units)
            .map(Some)
            .map_err(|_| crate::BdeError::InvalidData("invalid UTF-16 volume description"))
    }

    /// The top-level FVEK entry (an AES-CCM encrypted key wrapped by the VMK).
    #[must_use]
    pub fn fvek_entry(&self) -> Option<&MetadataEntry> {
        self.entries
            .iter()
            .find(|e| e.entry_type == ENTRY_TYPE_FVEK && e.value_type == VALUE_TYPE_AES_CCM)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_bytes(entry_type: u16, value_type: u16, version: u16, data: &[u8]) -> Vec<u8> {
        let size = (8 + data.len()) as u16;
        let mut v = Vec::new();
        v.extend_from_slice(&size.to_le_bytes());
        v.extend_from_slice(&entry_type.to_le_bytes());
        v.extend_from_slice(&value_type.to_le_bytes());
        v.extend_from_slice(&version.to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn parse_sequence_splits_entries() {
        let mut buf = Vec::new();
        buf.extend(entry_bytes(0x000f, 0x000f, 1, &[1, 2, 3, 4]));
        buf.extend(entry_bytes(ENTRY_TYPE_VMK, VALUE_TYPE_VMK, 1, &[9; 20]));
        let entries = MetadataEntry::parse_sequence(&buf);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry_type, 0x000f);
        assert_eq!(entries[0].data, vec![1, 2, 3, 4]);
        assert!(entries[1].is_vmk());
    }

    #[test]
    fn parse_sequence_stops_on_lying_size() {
        // size field claims 8 (empty) then a size of 0 — must not loop forever.
        let mut buf = entry_bytes(1, 2, 1, &[]);
        buf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // size=0 -> stop
        let entries = MetadataEntry::parse_sequence(&buf);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn parse_sequence_stops_on_oversize() {
        // size claims 100 but only 8 bytes present.
        let mut buf = Vec::new();
        buf.extend_from_slice(&100u16.to_le_bytes());
        buf.extend_from_slice(&[0u8; 6]);
        assert!(MetadataEntry::parse_sequence(&buf).is_empty());
    }

    fn build_block(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut entry_bytes = Vec::new();
        for e in entries {
            entry_bytes.extend_from_slice(e);
        }
        let metadata_size = 48 + entry_bytes.len();
        let mut block = vec![0u8; 64 + metadata_size];
        block[0..8].copy_from_slice(FVE_SIGNATURE);
        block[10..12].copy_from_slice(&2u16.to_le_bytes()); // version 2
        block[16..24].copy_from_slice(&0x0400_0000u64.to_le_bytes()); // encrypted size
        block[28..32].copy_from_slice(&16u32.to_le_bytes()); // vol header sectors
        block[56..64].copy_from_slice(&0x0211_0800u64.to_le_bytes()); // vol header offset
                                                                      // metadata header @64
        block[64..68].copy_from_slice(&(metadata_size as u32).to_le_bytes());
        block[64 + 16..64 + 32].copy_from_slice(&[0xAB; 16]); // volume guid
        block[64 + 36..64 + 38].copy_from_slice(&0x8000u16.to_le_bytes()); // method
        block[64 + 40..64 + 48].copy_from_slice(&130_461_864_497_281_120u64.to_le_bytes());
        block[64 + 48..].copy_from_slice(&entry_bytes);
        block
    }

    #[test]
    fn parse_full_block() {
        // A volume-header-block entry (0x000f) + a password VMK.
        let mut vh_data = Vec::new();
        vh_data.extend_from_slice(&0x0211_0800u64.to_le_bytes()); // block offset
        vh_data.extend_from_slice(&0x0051_5a00u64.to_le_bytes()); // block size
        let vh = entry_bytes(
            ENTRY_TYPE_VOLUME_HEADER,
            ENTRY_TYPE_VOLUME_HEADER,
            1,
            &vh_data,
        );

        let mut vmk_data = vec![0u8; 28];
        vmk_data[26..28].copy_from_slice(&PROTECTION_PASSWORD.to_le_bytes());
        let vmk = entry_bytes(ENTRY_TYPE_VMK, VALUE_TYPE_VMK, 1, &vmk_data);

        let block = build_block(&[vh, vmk]);
        let m = FveMetadata::parse(&block, 512).unwrap();
        assert_eq!(m.encryption_method, 0x8000);
        assert_eq!(m.volume_guid, [0xAB; 16]);
        assert_eq!(m.creation_time, 130_461_864_497_281_120);
        assert_eq!(m.encrypted_volume_size, 0x0400_0000);
        assert_eq!(m.volume_header_offset, 0x0211_0800);
        assert_eq!(m.volume_header_size, 0x0051_5a00); // from the 0x000f entry
        assert_eq!(m.entries.len(), 2);
        assert_eq!(m.protector_types(), vec![PROTECTION_PASSWORD]);
    }

    #[test]
    fn parse_returns_none_without_signature() {
        let block = vec![0u8; 128];
        assert!(FveMetadata::parse(&block, 512).is_none());
    }

    #[test]
    fn volume_header_size_falls_back_to_sector_count() {
        // No 0x000f entry: size = num_volume_header_sectors * bytes_per_sector.
        let block = build_block(&[]);
        let m = FveMetadata::parse(&block, 512).unwrap();
        assert_eq!(m.volume_header_size, 16 * 512);
        assert_eq!(m.volume_header_offset, 0x0211_0800);
    }

    #[test]
    fn truncated_block_does_not_panic() {
        let mut block = build_block(&[]);
        block.truncate(70);
        let _ = FveMetadata::parse(&block, 512);
    }

    #[test]
    fn description_preserves_unicode_and_rejects_invalid_encoding() {
        let bytes: Vec<_> = "取证卷\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let block = build_block(&[entry_bytes(7, 2, 1, &bytes)]);
        assert_eq!(
            FveMetadata::parse(&block, 512)
                .unwrap()
                .description()
                .unwrap(),
            Some("取证卷".into())
        );
        for bytes in [&[0xff][..], &[0x00, 0xd8][..]] {
            let block = build_block(&[entry_bytes(7, 2, 1, bytes)]);
            assert!(FveMetadata::parse(&block, 512)
                .unwrap()
                .description()
                .is_err());
        }
    }
}
