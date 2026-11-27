use super::*;
use crate::core::evidence::partitions::{detect_partitions_with_meta, SectorSource};

struct Disk {
    size: u64,
    volume_headers: Vec<(u64, Vec<u8>)>,
}

impl SectorSource for Disk {
    fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let end = self.size.min(offset + len as u64);
        let mut bytes = vec![0; end.saturating_sub(offset) as usize];
        for (base, data) in &self.volume_headers {
            let begin = offset.max(*base);
            let limit = end.min(*base + data.len() as u64);
            if begin < limit {
                bytes[(begin - offset) as usize..(limit - offset) as usize]
                    .copy_from_slice(&data[(begin - base) as usize..(limit - base) as usize]);
            }
        }
        Ok(bytes)
    }

    fn size(&self) -> u64 {
        self.size
    }
}

fn header(format: Format) -> Vec<u8> {
    let mut bytes = Vec::new();
    match format {
        Format::Fatx => {
            bytes.extend(b"FATX");
            for value in [42u32, 32, 1] {
                bytes.extend(value.to_le_bytes());
            }
        }
        Format::Xtaf => {
            bytes.extend(b"XTAF");
            for value in [42u32, 32, 1] {
                bytes.extend(value.to_be_bytes());
            }
        }
    }
    bytes
}

#[test]
fn xbox_partition_retail_landmarks_and_full_absolute_lengths() {
    let xbox = Disk {
        size: 0x1dd156000,
        volume_headers: vec![
            (0x80000, header(Format::Fatx)),
            (0xabe80000, header(Format::Fatx)),
        ],
    };
    let result = detect_partitions_with_meta(&xbox, 512).unwrap();
    assert_eq!(result.scheme, Some(TableScheme::Xbox));
    assert_eq!(result.partitions.len(), 2);
    assert_eq!(result.partitions[0].start_lba * 512, 0x80000);
    assert_eq!(result.partitions[0].length_lba * 512, 0x2ee00000);
    assert_eq!(result.partitions[1].start_lba * 512, 0xabe80000);
    assert_eq!(result.partitions[1].length_lba * 512, 0x1312d6000);

    let xbox360 = Disk {
        size: 0x160000000,
        volume_headers: vec![
            (0x10c080000, header(Format::Xtaf)),
            (0x130eb0000, header(Format::Xtaf)),
        ],
    };
    let result = detect_partitions_with_meta(&xbox360, 512).unwrap();
    assert_eq!(result.scheme, Some(TableScheme::Xbox));
    assert_eq!(result.partitions.len(), 2);
    assert_eq!(result.partitions[0].start_lba * 512, 0x10c080000);
    assert_eq!(result.partitions[0].length_lba * 512, 0x0ce30000);
    assert_eq!(result.partitions[1].start_lba * 512, 0x130eb0000);
    assert_eq!(
        (result.partitions[1].start_lba + result.partitions[1].length_lba) * 512,
        xbox360.size
    );
}
