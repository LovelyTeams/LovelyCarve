//! NILFS2 current committed checkpoint. Native history is available in the
//! standalone reader API; path IDs here retain source partition identity.
use super::*;

pub type Nilfs2Volume = RomVolume<nilfs2_reader::Nilfs2<RomImageSource>>;
pub fn parse_nilfs2_id(id: &str) -> Option<(u8, String)> {
    let (partition, path) = id.strip_prefix("nilfs2:part:")?.split_once(':')?;
    let path = String::from_utf8(hex::decode(path).ok()?).ok()?;
    valid_lookup_path(&path).then_some((partition.parse().ok()?, path))
}
pub fn open_nilfs2_source(image: &Arc<DiskImage>, partition: u8) -> Result<Nilfs2Volume> {
    Nilfs2Volume::open(
        Arc::clone(image),
        find_partition(image, &partition.to_string())?,
    )
}
