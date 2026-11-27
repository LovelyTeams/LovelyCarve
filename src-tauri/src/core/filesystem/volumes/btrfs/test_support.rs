//! Minimal, generated single-device Btrfs tree for navigation regressions.
//! Only the structures consumed by the reader are populated; no evidence bytes.

pub(crate) const CONTENT: &[u8] = b"Btrfs logical volume navigation\n";

fn leaf(items: &[(u64, u8, u64, Vec<u8>)]) -> Vec<u8> {
    let mut block = vec![0; 4096];
    block[96..100].copy_from_slice(&(items.len() as u32).to_le_bytes());
    let mut payload = block.len();
    for (index, (object_id, kind, offset, data)) in items.iter().enumerate() {
        let record = 101 + index * 25;
        payload -= data.len();
        block[record..record + 8].copy_from_slice(&object_id.to_le_bytes());
        block[record + 8] = *kind;
        block[record + 9..record + 17].copy_from_slice(&offset.to_le_bytes());
        block[record + 17..record + 21].copy_from_slice(&((payload - 101) as u32).to_le_bytes());
        block[record + 21..record + 25].copy_from_slice(&(data.len() as u32).to_le_bytes());
        block[payload..payload + data.len()].copy_from_slice(data);
    }
    block
}

pub(crate) fn image() -> Vec<u8> {
    let mut bytes = vec![0; 512 * 1024];
    let size = bytes.len() as u64;
    let superblock = &mut bytes[0x10000..0x11000];
    superblock[48..56].copy_from_slice(&0x10000u64.to_le_bytes());
    superblock[201..209].copy_from_slice(&1u64.to_le_bytes());
    superblock[209..217].copy_from_slice(&size.to_le_bytes());
    superblock[0x40..0x48].copy_from_slice(b"_BHRfS_M");
    superblock[80..88].copy_from_slice(&0x20000u64.to_le_bytes());
    superblock[88..96].copy_from_slice(&0x21000u64.to_le_bytes());
    superblock[112..120].copy_from_slice(&size.to_le_bytes());
    superblock[128..136].copy_from_slice(&256u64.to_le_bytes());
    superblock[136..144].copy_from_slice(&1u64.to_le_bytes());
    superblock[144..148].copy_from_slice(&4096u32.to_le_bytes());
    superblock[148..152].copy_from_slice(&4096u32.to_le_bytes());
    superblock[160..164].copy_from_slice(&97u32.to_le_bytes());
    // One SINGLE system chunk maps the whole logical stream one-to-one.
    superblock[811..819].copy_from_slice(&256u64.to_le_bytes());
    superblock[819] = 228;
    let chunk = 828;
    superblock[chunk..chunk + 8].copy_from_slice(&size.to_le_bytes());
    superblock[chunk + 8..chunk + 16].copy_from_slice(&2u64.to_le_bytes());
    superblock[chunk + 16..chunk + 24].copy_from_slice(&65536u64.to_le_bytes());
    superblock[chunk + 24..chunk + 32].copy_from_slice(&2u64.to_le_bytes());
    superblock[chunk + 40..chunk + 44].copy_from_slice(&4096u32.to_le_bytes());
    superblock[chunk + 44..chunk + 46].copy_from_slice(&1u16.to_le_bytes());
    superblock[chunk + 46..chunk + 48].copy_from_slice(&1u16.to_le_bytes());
    superblock[chunk + 48..chunk + 56].copy_from_slice(&1u64.to_le_bytes());
    let mut root = vec![0; 439];
    root[176..184].copy_from_slice(&0x22000u64.to_le_bytes());
    bytes[0x20000..0x21000].copy_from_slice(&leaf(&[(5, 132, 0, root)]));
    bytes[0x21000..0x22000].copy_from_slice(&leaf(&[]));
    let mut extent = vec![0; 21];
    extent[8..16].copy_from_slice(&(CONTENT.len() as u64).to_le_bytes());
    extent.extend_from_slice(CONTENT);
    bytes[0x22000..0x23000].copy_from_slice(&file_tree(CONTENT.len() as u64, 0, extent));
    bytes
}

fn file_tree(size: u64, file_offset: u64, extent: Vec<u8>) -> Vec<u8> {
    let mut directory = vec![0; 160];
    directory[52..56].copy_from_slice(&0o40755u32.to_le_bytes());
    let mut file = vec![0; 160];
    file[16..24].copy_from_slice(&size.to_le_bytes());
    file[52..56].copy_from_slice(&0o100644u32.to_le_bytes());
    let mut entry = vec![0; 30];
    entry[..8].copy_from_slice(&257u64.to_le_bytes());
    entry[8] = 1;
    entry[27..29].copy_from_slice(&9u16.to_le_bytes());
    entry[29] = 1;
    entry.extend_from_slice(b"hello.txt");
    leaf(&[
        (256, 1, 0, directory),
        (256, 96, 2, entry),
        (257, 1, 0, file),
        (257, 108, file_offset, extent),
    ])
}

/// [`striped_members`] written as two evidence files under `directory`,
/// grouped by `raid detect` and registered as its synthetic evidence item.
pub(crate) fn assembled_striped_set(
    directory: &std::path::Path,
) -> std::sync::Arc<crate::core::filesystem::evidence_session::EvidenceSession> {
    use crate::core::evidence::image::DiskImage;
    use std::sync::Arc;

    let images: Vec<_> = striped_members()
        .iter()
        .enumerate()
        .map(|(index, bytes)| {
            let path = directory.join(format!("stripe-{index}.img"));
            std::fs::write(&path, bytes).unwrap();
            Arc::new(DiskImage::open(path).unwrap())
        })
        .collect();
    let (stack, groups) = crate::core::evidence::raid::detect_with_sources(&images);
    let [group] = groups.as_slice() else {
        panic!("one Btrfs set expected: {:?}", stack.diagnostics);
    };
    let image = DiskImage::from_assembled(
        group.label.clone(),
        Arc::clone(&group.source),
        group.partitions.clone().unwrap(),
        512,
    )
    .unwrap();
    Arc::new(image.into())
}

pub(crate) fn striped_members() -> [Vec<u8>; 2] {
    use crate::core::evidence::partitions::btrfs::superblock_crc32c;

    const STRIPE: usize = 64 * 1024;
    const STRIPE_BASE: usize = 2 * STRIPE;
    let mut logical = image();
    let size = logical.len();
    // The chunk tree lives on device 2. The root and file trees live on device 1.
    logical.copy_within(0x21000..0x22000, 0x31000);
    let mut extent = vec![0; 53];
    extent[8..16].copy_from_slice(&8192u64.to_le_bytes()); // decoded bytes
    extent[20] = 1;
    extent[21..29].copy_from_slice(&0x4f000u64.to_le_bytes()); // crosses stripe boundary
    extent[29..37].copy_from_slice(&8192u64.to_le_bytes()); // allocated bytes
    extent[45..53].copy_from_slice(&8192u64.to_le_bytes());
    logical[0x22000..0x23000].copy_from_slice(&file_tree(12288, 4096, extent));
    logical[0x4f000..0x50000].fill(0x7a);
    logical[0x50000..0x51000].fill(0x6b);

    let mut members = [vec![0; size], vec![0; size]];
    for (stripe, bytes) in logical.chunks_exact(STRIPE).enumerate() {
        let physical = STRIPE_BASE + (stripe / 2) * STRIPE;
        members[stripe % 2][physical..physical + STRIPE].copy_from_slice(bytes);
    }
    let sb = &mut logical[0x10000..0x11000];
    sb[32..48].fill(0x50);
    sb[72..80].copy_from_slice(&7u64.to_le_bytes());
    sb[88..96].copy_from_slice(&0x31000u64.to_le_bytes());
    sb[112..120].copy_from_slice(&((2 * size) as u64).to_le_bytes());
    sb[136..144].copy_from_slice(&2u64.to_le_bytes());
    sb[160..164].copy_from_slice(&129u32.to_le_bytes());
    let chunk = 828;
    sb[chunk + 24..chunk + 32].copy_from_slice(&10u64.to_le_bytes()); // SYSTEM + RAID0
    sb[chunk + 44..chunk + 46].copy_from_slice(&2u16.to_le_bytes());
    sb[chunk + 48..chunk + 80].fill(0);
    sb[chunk + 48..chunk + 56].copy_from_slice(&1u64.to_le_bytes());
    sb[chunk + 56..chunk + 64].copy_from_slice(&(STRIPE_BASE as u64).to_le_bytes());
    sb[chunk + 64..chunk + 80].fill(0x70);
    sb[chunk + 80..chunk + 88].copy_from_slice(&2u64.to_le_bytes());
    sb[chunk + 88..chunk + 96].copy_from_slice(&(STRIPE_BASE as u64).to_le_bytes());
    sb[chunk + 96..chunk + 112].fill(0x71);
    for (index, member) in members.iter_mut().enumerate() {
        let header = &mut member[0x10000..0x11000];
        header.copy_from_slice(sb);
        header[201..209].copy_from_slice(&((index + 1) as u64).to_le_bytes());
        header[267..283].fill(0x70 + index as u8);
        header[283..299].fill(0x50);
        let crc = superblock_crc32c(&header[32..]);
        header[..4].copy_from_slice(&crc.to_le_bytes());
    }
    members
}
