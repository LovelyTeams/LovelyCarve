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
    BcachefsVolume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}
