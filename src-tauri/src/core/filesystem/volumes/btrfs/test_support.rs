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
    let mut directory = vec![0; 160];
    directory[52..56].copy_from_slice(&0o40755u32.to_le_bytes());
    let mut file = vec![0; 160];
    file[16..24].copy_from_slice(&(CONTENT.len() as u64).to_le_bytes());
    file[52..56].copy_from_slice(&0o100644u32.to_le_bytes());
    let mut entry = vec![0; 30];
    entry[..8].copy_from_slice(&257u64.to_le_bytes());
    entry[8] = 1;
    entry[27..29].copy_from_slice(&9u16.to_le_bytes());
    entry[29] = 1;
    entry.extend_from_slice(b"hello.txt");
    let mut extent = vec![0; 21];
    extent[8..16].copy_from_slice(&(CONTENT.len() as u64).to_le_bytes());
    extent.extend_from_slice(CONTENT);
    bytes[0x22000..0x23000].copy_from_slice(&leaf(&[
        (256, 1, 0, directory),
        (256, 96, 2, entry),
        (257, 1, 0, file),
        (257, 108, 0, extent),
    ]));
    bytes
}
