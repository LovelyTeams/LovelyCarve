//! DD-WRT's per-block pb/lc/lp prefix, documented by Firmware Mod Kit's
//! squashfs-3.0-lzma-damn-small-variant/LZMA_Lib/ZLib.cpp. No parameter search.

use std::io::Read;

use crate::BackhandError;

pub(crate) fn decompress(
    bytes: &[u8],
    out: &mut Vec<u8>,
    maximum: usize,
) -> Result<(), BackhandError> {
    if bytes.len() < 5 || bytes[0] > 4 || bytes[1] > 8 || bytes[2] > 4 {
        return Err(BackhandError::CorruptedOrInvalidSquashfs);
    }
    let mut reader = lzma_rust2::LzmaReader::new(
        &bytes[4..],
        u64::MAX,
        u32::from(bytes[1]),
        u32::from(bytes[2]),
        u32::from(bytes[0]),
        1 << 23,
        None,
    )?;
    let mut decoded = Vec::new();
    reader
        .by_ref()
        .take(maximum as u64 + 1)
        .read_to_end(&mut decoded)?;
    if decoded.len() > maximum {
        return Err(BackhandError::CorruptedOrInvalidSquashfs);
    }
    out.extend_from_slice(&decoded);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::filesystem::BackhandInnerNode;

    #[test]
    fn ddwrt_independent_fixture_reads_different_block_properties() {
        let fixture = include_bytes!("../../test_data/ddwrt-v3.squashfs");
        let kind = crate::kind::Kind::from_const(crate::kind::LE_V3_0_DDWRT).unwrap();
        let filesystem =
            crate::create_squashfs_from_kind(std::io::Cursor::new(fixture), 0, kind).unwrap();
        let file = filesystem
            .files()
            .find(|node| node.fullpath == std::path::Path::new("/hello"))
            .unwrap();
        let BackhandInnerNode::File(file) = file.inner else {
            panic!("expected file");
        };
        let expected = b"Independent DD-WRT fixture.\n".repeat(64);
        assert_eq!(filesystem.file_data(&file).unwrap(), expected);
        assert_eq!(
            filesystem.file_data_range(&file, 5, 31).unwrap(),
            expected[5..36]
        );
        assert_eq!(
            filesystem.file_data_range(&file, u64::MAX, 20).unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn ddwrt_rejects_bad_properties_truncated_stream_and_output_overflow() {
        let fixture = include_bytes!("../../test_data/ddwrt-v3.squashfs");
        // The first payload block starts immediately after the 119-byte superblock.
        let inode_start = u64::from_le_bytes(fixture[87..95].try_into().unwrap()) as usize;
        let block = &fixture[119..inode_start];
        assert!(decompress(block, &mut Vec::new(), 1).is_err());
        assert!(decompress(&block[..block.len() / 2], &mut Vec::new(), 8192).is_err());
        assert!(decompress(&[9, 0, 0, 0, 0], &mut Vec::new(), 8192).is_err());
    }

    #[test]
    fn ddwrt_preserves_32_bit_uid_and_gid_inheritance() {
        let mut fixture = include_bytes!("../../test_data/ddwrt-v3.squashfs").to_vec();
        let end = fixture.len();
        fixture[end - 4..].copy_from_slice(&100_000u32.to_le_bytes());
        let kind = crate::kind::Kind::from_const(crate::kind::LE_V3_0_DDWRT).unwrap();
        let filesystem =
            crate::create_squashfs_from_kind(std::io::Cursor::new(fixture), 0, kind).unwrap();
        assert!(
            filesystem
                .files()
                .all(|node| node.header.uid == 100_000 && node.header.gid == 100_000)
        );
    }
}
