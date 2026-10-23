use super::*;
use crate::core::evidence::image::{DiskImage, ImageFormat};
use crate::core::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{stream_file_chunks, FsSession},
};

const ROOTFS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test_data/rom-images/synthetic-le.cramfs"
));
const CONTENT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test_data/rom-images/payload.bin"
));

fn uimage(payload: &[u8], kind: u8) -> Vec<u8> {
    let mut bytes = vec![0; 64];
    bytes[..4].copy_from_slice(segments::UIMAGE_MAGIC);
    bytes[12..16].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes[24..28].copy_from_slice(&crc32fast::hash(payload).to_be_bytes());
    bytes[28..32].copy_from_slice(&[5, 5, kind, 0]);
    let crc = crc32fast::hash(&bytes);
    bytes[4..8].copy_from_slice(&crc.to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

fn ngr_checksum(bytes: &[u8]) -> u32 {
    // Independent oracle: sum bytes and each byte's multiplicity in c1.
    let first: u64 = bytes.iter().map(|byte| u64::from(*byte)).sum();
    let second: u64 = bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| (bytes.len() - index) as u64 * u64::from(*byte))
        .sum();
    let fold = |sum: u64| {
        let value = sum as u32;
        let mut out = (value >> 16) + (value & 65535);
        out = (out >> 16) + (out & 65535);
        out & 65535
    };
    (fold(second) << 16) | fold(first)
}

fn chk(kernel: &[u8], root: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 48];
    bytes[..4].copy_from_slice(segments::CHK_MAGIC);
    bytes[4..8].copy_from_slice(&48u32.to_be_bytes());
    bytes[40..48].copy_from_slice(b"SYNTH-ID");
    bytes[16..20].copy_from_slice(&ngr_checksum(kernel).to_be_bytes());
    bytes[20..24].copy_from_slice(&ngr_checksum(root).to_be_bytes());
    bytes[24..28].copy_from_slice(&(kernel.len() as u32).to_be_bytes());
    bytes[28..32].copy_from_slice(&(root.len() as u32).to_be_bytes());
    let payload: Vec<u8> = kernel.iter().chain(root).copied().collect();
    bytes[32..36].copy_from_slice(&ngr_checksum(&payload).to_be_bytes());
    let crc = ngr_checksum(&bytes);
    bytes[36..40].copy_from_slice(&crc.to_be_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

fn refresh_safe_md5(bytes: &mut [u8]) {
    use md5::{Digest, Md5};
    let mut hash = Md5::new();
    hash.update([
        0x7a, 0x2b, 0x15, 0xed, 0x9b, 0x98, 0x59, 0x6d, 0xe5, 0x04, 0xab, 0x44, 0xac, 0x2a, 0x9f,
        0x4e,
    ]);
    hash.update(&bytes[20..]);
    bytes[4..20].copy_from_slice(&hash.finalize());
}

fn safe_image() -> Vec<u8> {
    let mut bytes = vec![0xff; 0x1814];
    let entries: [(&str, &[u8]); 4] = [
        ("partition-table", b"synthetic flash map"),
        ("support-list", b"synthetic device"),
        ("rootfs-a", ROOTFS),
        ("rootfs-b", ROOTFS),
    ];
    let mut table = String::new();
    for (name, data) in entries {
        table.push_str(&format!(
            "fwup-ptn {name} base 0x{:08x} size 0x{:08x}\t\r\n",
            bytes.len() - 0x1014,
            data.len()
        ));
        bytes.extend_from_slice(data);
    }
    bytes[0x1014..0x1014 + table.len()].copy_from_slice(table.as_bytes());
    bytes[0x1014 + table.len()] = 0;
    let length = bytes.len() as u32;
    bytes[..4].copy_from_slice(&length.to_be_bytes());
    refresh_safe_md5(&mut bytes);
    bytes
}

#[test]
fn firmware_wrappers_browse_range_stream_and_preserve_source() {
    let mut combined = uimage(b"synthetic kernel", 2);
    combined.extend_from_slice(ROOTFS);
    let mut multi = vec![0; 12];
    multi[..4].copy_from_slice(&4u32.to_be_bytes());
    multi[4..8].copy_from_slice(&(ROOTFS.len() as u32).to_be_bytes());
    multi.extend_from_slice(b"kern");
    multi.extend_from_slice(ROOTFS);
    for (bytes, expected_count) in [
        (combined.clone(), 1),
        (chk(&combined, &[]), 1),
        (chk(&uimage(b"kernel", 2), ROOTFS), 1),
        (safe_image(), 2),
        (uimage(&multi, 4), 1),
        (uimage(ROOTFS, 7), 1),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("firmware.bin");
        std::fs::write(&path, &bytes).unwrap();
        let image = Arc::new(EvidenceSession::from(DiskImage::open(path).unwrap()));
        assert_eq!(image.info().source_format, ImageFormat::Firmware);
        assert_eq!(image.sector_size, 1);
        assert_eq!(image.partitions.len(), expected_count);
        assert_eq!(image.read_range(0, bytes.len()).unwrap(), bytes);
        let mut fs = FsSession::new(&image);
        for partition in &image.partitions {
            let root = fs.children(&format!("part:{}", partition.id)).unwrap();
            let node = root.iter().find(|node| node.name == "payload.bin").unwrap();
            assert_eq!(fs.read_file_limited(&node.id, usize::MAX).unwrap(), CONTENT);
            assert_eq!(
                fs.read_file_range(&node.id, 8180, 100).unwrap(),
                CONTENT[8180..8280]
            );
            let mut streamed = Vec::new();
            stream_file_chunks(&image, &node.id, 4100, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .unwrap();
            assert_eq!(streamed, CONTENT);
        }
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(FirmwareImage::open(&bytes, &cancel).is_err());
    }
}

#[test]
fn firmware_wrappers_reject_truncation_crc_and_nested_corruption() {
    let mut combined = uimage(b"kernel", 2);
    combined.extend_from_slice(ROOTFS);
    for bytes in [combined.clone(), chk(&combined, &[]), safe_image()] {
        for end in [0, 5, 20, 40] {
            assert!(FirmwareImage::open(&bytes[..end], &CancellationToken::new()).is_err());
        }
        let mut corrupt = bytes.clone();
        corrupt[12] ^= 1;
        assert!(FirmwareImage::open(&corrupt, &CancellationToken::new()).is_err());
    }
    // CHK/SafeLoader cover the rootfs length; bare uImage's CRC covers only
    // its kernel. The native filesystem must reject a truncated trailer.
    for bytes in [chk(&combined, &[]), safe_image()] {
        assert!(FirmwareImage::open(&bytes[..bytes.len() - 1], &CancellationToken::new()).is_err());
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("truncated.bin");
    std::fs::write(&path, &combined[..combined.len() - 1]).unwrap();
    let evidence = Arc::new(EvidenceSession::open(path).unwrap());
    assert!(FsSession::new(&evidence).children("part:0").is_err());
    let mut bad_uimage = combined.clone();
    bad_uimage[64] ^= 1;
    // Recompute the outer CHK checksum; the inner uImage CRC must still fail.
    let error = FirmwareImage::open(&chk(&bad_uimage, &[]), &CancellationToken::new()).unwrap_err();
    assert!(error.to_string().contains("uImage data CRC"));
    let mut bad_chk = chk(&combined, &[]);
    *bad_chk.last_mut().unwrap() ^= 1;
    assert!(FirmwareImage::open(&bad_chk, &CancellationToken::new())
        .unwrap_err()
        .to_string()
        .contains("checksum"));
    let mut nesting = uimage(ROOTFS, 7);
    for _ in 0..5 {
        nesting = chk(&nesting, &[]);
    }
    assert!(FirmwareImage::open(&nesting, &CancellationToken::new())
        .unwrap_err()
        .to_string()
        .contains("nesting"));
}

#[test]
fn firmware_wrappers_component_and_filesystem_limits_do_not_wrap_ids() {
    let make_multi = |components: &[Vec<u8>]| {
        let mut data = Vec::new();
        for component in components {
            data.extend_from_slice(&(component.len() as u32).to_be_bytes());
        }
        data.extend_from_slice(&0u32.to_be_bytes());
        for (index, component) in components.iter().enumerate() {
            data.extend_from_slice(component);
            if index + 1 != components.len() {
                data.resize(data.len().div_ceil(4) * 4, 0);
            }
        }
        uimage(&data, 4)
    };
    let bytes = make_multi(&vec![ROOTFS.to_vec(); 32]);
    let image = FirmwareImage::open(&bytes, &CancellationToken::new()).unwrap();
    assert_eq!(image.partitions.len(), 32);
    assert_eq!(image.partitions.last().unwrap().id, 31);
    let excessive = make_multi(&vec![bytes; 9]);
    assert!(FirmwareImage::open(&excessive, &CancellationToken::new())
        .unwrap_err()
        .to_string()
        .contains("count limit"));
    let too_many = make_multi(&vec![ROOTFS.to_vec(); 33]);
    assert!(FirmwareImage::open(&too_many, &CancellationToken::new())
        .unwrap_err()
        .to_string()
        .contains("component limit"));
}

#[test]
fn firmware_safeloader_valid_checksum_does_not_hide_bad_spans_or_names() {
    let original = safe_image();
    for (from, to) in [
        (b"rootfs-b".as_slice(), b"rootfs-a".as_slice()),
        (b"base 0x00000800".as_slice(), b"base 0x00000000".as_slice()),
    ] {
        let mut bytes = original.clone();
        let position = bytes
            .windows(from.len())
            .position(|window| window == from)
            .unwrap();
        bytes[position..position + from.len()].copy_from_slice(to);
        refresh_safe_md5(&mut bytes);
        assert!(FirmwareImage::open(&bytes, &CancellationToken::new()).is_err());
    }
    let mut bytes = original.clone();
    let table = &bytes[0x1014..0x1814];
    let lines = std::str::from_utf8(&table[..table.iter().position(|b| *b == 0).unwrap()]).unwrap();
    let start = lines
        .lines()
        .find(|line| line.contains("rootfs-a"))
        .unwrap()
        .split_ascii_whitespace()
        .nth(3)
        .unwrap()
        .to_string();
    let row = lines
        .lines()
        .find(|line| line.contains("rootfs-b"))
        .unwrap();
    let prior = row.split_ascii_whitespace().nth(3).unwrap();
    let position = bytes
        .windows(prior.len())
        .position(|window| window == prior.as_bytes())
        .unwrap();
    bytes[position..position + start.len()].copy_from_slice(start.as_bytes());
    refresh_safe_md5(&mut bytes);
    assert!(FirmwareImage::open(&bytes, &CancellationToken::new())
        .unwrap_err()
        .to_string()
        .contains("overlap"));
}

#[test]
#[ignore = "set LOVELYCARVE_WRAPPED_FIRMWARE to semicolon-separated public SafeLoader/uImage/CHK inputs"]
fn firmware_wrappers_external_samples() {
    let paths = std::env::var("LOVELYCARVE_WRAPPED_FIRMWARE").unwrap();
    for path in paths.split(';') {
        let original = std::fs::read(path).unwrap();
        let image = Arc::new(EvidenceSession::open(path).unwrap());
        assert_eq!(image.info().source_format, ImageFormat::Firmware);
        assert_eq!(image.read_range(0, original.len()).unwrap(), original);
        let mut fs = FsSession::new(&image);
        let entries = fs.children("part:0").unwrap();
        let etc = entries.iter().find(|entry| entry.name == "etc").unwrap();
        let entries = fs.children(&etc.id).unwrap();
        let release = entries
            .iter()
            .find(|entry| entry.name == "openwrt_release")
            .unwrap();
        let bytes = fs.read_file_limited(&release.id, 4096).unwrap();
        assert!(std::str::from_utf8(&bytes).unwrap().contains("24.10.1"));
        assert_eq!(
            fs.read_file_range(&release.id, 3, 37).unwrap(),
            bytes[3..40]
        );
    }
}
