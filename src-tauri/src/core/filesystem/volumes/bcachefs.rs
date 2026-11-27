//! Names retain source partition identity and their full path through native
//! subvolumes and snapshots. The reader keeps inode plus snapshot identities.
use super::*;

pub type BcachefsVolume = RomVolume<bcachefs_reader::Bcachefs<RomImageSource>>;
pub fn parse_bcachefs_id(id: &str) -> Option<(u8, String)> {
    let (partition, path) = id.strip_prefix("bcachefs:part:")?.split_once(':')?;
    let path = String::from_utf8(hex::decode(path).ok()?).ok()?;
    valid_lookup_path(&path).then_some((partition.parse().ok()?, path))
}
pub fn open_bcachefs_source(image: &Arc<DiskImage>, partition: u8) -> Result<BcachefsVolume> {
    use crate::core::evidence::partitions::bcachefs::read_member;

    let primary = find_partition(image, &partition.to_string())?;
    let header = read_member(image, primary)?;
    if header.superblock.nr_devices == 1 {
        return BcachefsVolume::open(Arc::clone(image), primary);
    }
    let mut sources = vec![RomImageSource::new(Arc::clone(image), primary)?];
    for part in image
        .partitions
        .iter()
        .filter(|part| part.id != partition && part.kind == primary.kind)
    {
        let Ok(other) = read_member(image, part) else {
            continue;
        };
        if other.superblock.uuid == header.superblock.uuid {
            sources.push(RomImageSource::new(Arc::clone(image), part)?);
        }
    }
    let reader =
        bcachefs_reader::Bcachefs::open_members(sources).context("bcachefs members open")?;
    Ok(RomVolume {
        partition_id: partition,
        reader: RefCell::new(reader),
    })
}
