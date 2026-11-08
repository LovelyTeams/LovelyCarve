//! Bounded descriptor discovery shared by opening and dependency enumeration.
use crate::{descriptor::decode_descriptor, VmdkError};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub(crate) fn read_text(path: &Path) -> Result<Option<String>, VmdkError> {
    let mut file = File::open(path)?;
    let mut magic = [0; 4];
    file.read_exact(&mut magic)?;
    if matches!(&magic, b"KDMV" | b"COWD" | b"\xbe\xba\xfe\xca") {
        return Ok(None);
    }
    const LIMIT: u64 = 1024 * 1024;
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(VmdkError::MalformedDescriptor("descriptor exceeds 1 MiB"));
    }
    let text = decode_descriptor(&bytes);
    Ok(Some(
        text.trim_start_matches('\u{feff}').trim_start().to_owned(),
    ))
}

#[cfg(all(test, feature = "full-encoding"))]
mod tests {
    use super::*;
    use crate::VmdkFileReader;

    #[test]
    fn gbk_descriptor_discovery_and_reads_use_the_same_filenames() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("disk.vmdk");
        let text = "\n# Disk DescriptorFile\ncreateType = \"monolithicFlat\"\nddb.encoding = \"GBK\"\nRW 1 FLAT \"数据.bin\" 1 # offset\n";
        let (bytes, _, errors) = encoding_rs::GBK.encode(text);
        assert!(!errors);
        std::fs::write(&path, bytes).unwrap();
        let extent = directory.path().join("数据.bin");
        std::fs::write(&extent, [vec![0; 512], vec![7; 512]].concat()).unwrap();
        assert_eq!(
            VmdkFileReader::extent_dependencies(&path).unwrap(),
            vec![extent]
        );
        let mut reader = VmdkFileReader::open_path(&path).unwrap();
        let mut result = [0; 512];
        reader.read_exact(&mut result).unwrap();
        assert_eq!(result, [7; 512]);
    }

    #[test]
    fn descriptor_size_and_malformed_metadata_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("disk.vmdk");
        std::fs::write(&path, vec![b'#'; 1024 * 1024 + 1]).unwrap();
        assert!(read_text(&path).is_err());
        for text in [
            "parentCID=broken",
            "RW 1 FLAT \"a\" invalid",
            "RW 1 MISSING \"a\"",
        ] {
            assert!(crate::descriptor::parse_text_descriptor(text).is_err());
        }
    }
}
