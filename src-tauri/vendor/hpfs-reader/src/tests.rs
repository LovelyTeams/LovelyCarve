use std::io::Read;

use flate2::read::ZlibDecoder;
use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};

use crate::{Hpfs, SECTOR_SIZE, le32};

fn fixture() -> Vec<u8> {
    let compressed = include_bytes!("../tests/fixtures/hpfsutils.zlib");
    let mut bytes = Vec::new();
    ZlibDecoder::new(compressed.as_slice())
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[test]
fn hpfsutils_lists_split_dnodes_and_preserves_file_bytes() {
    let bytes = fixture();
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut fs = Hpfs::open(SliceSource::new(&bytes), &mut cx).unwrap();
    let root = fs.root();
    let entries = fs.read_dir(root, &mut cx).unwrap();
    assert_eq!(entries.len(), 4);
    let tree = fs.lookup(root, "TREE", &mut cx).unwrap().unwrap();
    let entries = fs.read_dir(tree, &mut cx).unwrap();
    assert_eq!(entries.len(), 513);
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry.name, format!("Item {index:04}.txt"));
        let mut bytes = [0; 16];
        let len = fs.read_at(entry.node, 0, &mut bytes, &mut cx).unwrap();
        assert_eq!(&bytes[..len], format!("{index:04} HPFS entry\n").as_bytes());
    }
    let readme = fs.lookup(root, "read me.txt", &mut cx).unwrap().unwrap();
    let mut bytes = [0; 512];
    let count = fs.read_at(readme, 0, &mut bytes, &mut cx).unwrap();
    assert_eq!(&bytes[..count], b"independent HPFS fixture\n");
    assert!(
        fs.local_timestamps(readme, &mut cx)
            .unwrap()
            .created
            .is_some()
    );
    assert!(fs.timestamps(readme, &mut cx).unwrap().created.is_none());
    let payload = fs.lookup(root, "payload.bin", &mut cx).unwrap().unwrap();
    let size = 4129 * 256;
    assert_eq!(fs.metadata(payload, &mut cx).unwrap().size, size + 4);
    let count = fs.read_at(payload, size - 3, &mut bytes, &mut cx).unwrap();
    assert_eq!(&bytes[..count], b"\xfd\xfe\xfftail");
    assert_eq!(
        fs.read_at(payload, size + 2048, &mut bytes, &mut cx)
            .unwrap(),
        0
    );
}

#[test]
fn allocation_anodes_and_hotfixes_preserve_unaligned_ranges() {
    let mut image = fixture();
    let mut cache = BlockCache::new(0);
    let mut cx = SubstrateCtx {
        cache: &mut cache,
        verifier: &NoVerifier,
    };
    let mut fs = Hpfs::open(SliceSource::new(&image), &mut cx).unwrap();
    let payload = fs
        .lookup(fs.root(), "payload.bin", &mut cx)
        .unwrap()
        .unwrap();
    let fnode = fs.record(payload).unwrap().fnode;
    let fnode_offset = fnode as usize * SECTOR_SIZE;
    let physical = le32(&image, fnode_offset + 72);
    let length = le32(&image, fnode_offset + 68);
    let split = length / 2;
    let first_anode = 49150;
    let second_anode = 49151;
    let spare_offset = 17 * SECTOR_SIZE;
    let hotfix_offset = le32(&image, spare_offset + 12) as usize * SECTOR_SIZE;
    let hotfix_capacity = le32(&image, spare_offset + 20) as usize;
    let replacement = le32(&image, hotfix_offset + hotfix_capacity * 4);
    let original = physical + split;
    let range = |sector: u32| sector as usize * SECTOR_SIZE..(sector as usize + 1) * SECTOR_SIZE;
    image.copy_within(range(original), range(replacement).start);
    image[range(original)].fill(0xba);
    image[spare_offset + 8] |= 4;
    write32(&mut image, spare_offset + 16, 1);
    write32(&mut image, hotfix_offset, original);

    let tree = &mut image[fnode_offset + 56..fnode_offset + 160];
    tree.fill(0);
    tree[..8].copy_from_slice(&[0x80, 0, 0, 0, 10, 2, 24, 0]);
    write32(tree, 8, split);
    write32(tree, 12, first_anode);
    write32(tree, 16, u32::MAX);
    write32(tree, 20, second_anode);
    for (sector, logical, physical, length) in [
        (first_anode, 0, physical, split),
        (second_anode, split, original, length - split),
    ] {
        let anode = &mut image[range(sector)];
        write32(anode, 0, 0x37e40aae);
        write32(anode, 4, sector);
        write32(anode, 8, fnode);
        anode[12..20].copy_from_slice(&[0x20, 0, 0, 0, 39, 1, 20, 0]);
        write32(anode, 20, logical);
        write32(anode, 24, length);
        write32(anode, 28, physical);
    }
    let offset = u64::from(split) * SECTOR_SIZE as u64 - 3;
    let mut bytes = [0; 10];
    let mut fs = Hpfs::open(SliceSource::new(&image), &mut cx).unwrap();
    assert_eq!(
        fs.read_at(payload, offset, &mut bytes, &mut cx).unwrap(),
        bytes.len()
    );
    assert_eq!(bytes, [253, 254, 255, 0, 1, 2, 3, 4, 5, 6]);
    write32(&mut image, range(second_anode).start + 8, 0);
    let mut fs = Hpfs::open(SliceSource::new(&image), &mut cx).unwrap();
    assert!(fs.read_at(payload, offset, &mut bytes, &mut cx).is_err());
}

fn write32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
