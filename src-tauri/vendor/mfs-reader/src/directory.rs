use std::collections::BTreeMap;

use classic_hfs::{compare_mac_roman, decode_mac_roman, encode_mac_roman};
use lamfold::{BlockSource, FoldError, LocalFileTimes, NodeId, Result};

use crate::geometry::SECTOR;
use crate::{Header, be16, be32, require};

const FIXED_BYTES: usize = 51;
const MAC_EPOCH_DELTA: i64 = 2_082_844_800;

#[derive(Clone, Copy)]
pub(crate) struct ForkInfo {
    pub first: u16,
    pub length: u32,
    pub allocated: u32,
}

pub(crate) struct File {
    pub name: String,
    raw_name: Vec<u8>,
    version: u8,
    pub forks: [ForkInfo; 2],
    pub times: LocalFileTimes,
}

pub(crate) fn load(
    source: &mut impl BlockSource,
    header: &Header,
) -> Result<BTreeMap<NodeId, File>> {
    let mut files = BTreeMap::new();
    let mut sector = [0; SECTOR as usize];
    for offset in header.directory.clone().step_by(SECTOR as usize) {
        source.read_at(offset, &mut sector)?;
        let mut offset = 0;
        while offset < sector.len() && sector[offset] & 0x80 != 0 {
            let tail = &sector[offset..];
            require(
                tail.len() >= FIXED_BYTES,
                "MFS directory record crosses sector",
            )?;
            let length = usize::from(tail[50]);
            let record_size = (FIXED_BYTES + length).next_multiple_of(2);
            require(
                length != 0 && record_size <= tail.len(),
                "MFS filename crosses directory sector",
            )?;
            let raw_name = tail[51..51 + length].to_vec();
            require(!raw_name.contains(&0), "MFS filename contains NUL")?;
            let number = u64::from(be32(tail, 18));
            require(number != 0, "MFS file record has no file number")?;
            let fork = |at| ForkInfo {
                first: be16(tail, at),
                length: be32(tail, at + 2),
                allocated: be32(tail, at + 6),
            };
            let time = |at| {
                let secs = be32(tail, at);
                (secs != 0).then_some((i64::from(secs) - MAC_EPOCH_DELTA, 0))
            };
            let file = File {
                name: display(&raw_name),
                raw_name,
                version: tail[1],
                forks: [fork(22), fork(32)],
                times: LocalFileTimes {
                    created: time(42),
                    modified: time(46),
                    ..LocalFileTimes::default()
                },
            };
            require(
                files.insert(number, file).is_none(),
                "duplicate MFS file number",
            )?;
            offset += record_size;
        }
    }
    require(
        files.len() == usize::from(header.file_count),
        "MFS directory count disagrees with master directory block",
    )?;
    display_versions(&mut files)?;
    Ok(files)
}

fn display_versions(files: &mut BTreeMap<NodeId, File>) -> Result<()> {
    let mut sorted: Vec<_> = files.iter().collect();
    sorted.sort_unstable_by(|(_, a), (_, b)| {
        compare_mac_roman(&a.raw_name, &b.raw_name).then(a.version.cmp(&b.version))
    });
    let mut suffixes = Vec::new();
    for group in
        sorted.chunk_by(|(_, a), (_, b)| compare_mac_roman(&a.raw_name, &b.raw_name).is_eq())
    {
        if group.len() == 1 {
            continue;
        }
        require(
            group
                .windows(2)
                .all(|pair| pair[0].1.version != pair[1].1.version),
            "duplicate MFS filename and version",
        )?;
        suffixes.extend(
            group
                .iter()
                .filter(|(_, file)| file.version != 0)
                .map(|(number, file)| (**number, file.version)),
        );
    }
    for (number, version) in suffixes {
        files
            .get_mut(&number)
            .ok_or(FoldError::NotFound)?
            .name
            .push_str(&format!(" ⟨v{version}⟩"));
    }
    Ok(())
}

fn display(raw: &[u8]) -> String {
    match raw {
        b"." => "．".into(),
        b".." => "．．".into(),
        name => decode_mac_roman(name)
            .chars()
            .map(|character| match character {
                '/' => ':',
                ':' => '：',
                '\\' => '＼',
                character => character,
            })
            .collect(),
    }
}

fn key(name: &str) -> Option<(Vec<u8>, Option<u8>)> {
    let (name, version) = if let Some((base, suffix)) = name.rsplit_once(" ⟨v") {
        (base, Some(suffix.strip_suffix('⟩')?.parse::<u8>().ok()?))
    } else {
        (name, None)
    };
    let raw: String = match name {
        "．" => ".".into(),
        "．．" => "..".into(),
        name => name
            .chars()
            .map(|character| match character {
                ':' => '/',
                '：' => ':',
                '＼' => '\\',
                character => character,
            })
            .collect(),
    };
    Some((encode_mac_roman(&raw)?, version))
}

pub fn names_equal(a: &str, b: &str) -> bool {
    match (key(a), key(b)) {
        (Some((a, av)), Some((b, bv))) => av == bv && compare_mac_roman(&a, &b).is_eq(),
        _ => false,
    }
}
