//! Bounded QCOW 1/2/3 header parsing.
use crate::{
    extension::{Extension, FeatureNameTable},
    feature::{Feature, FeatureKind},
    int::div_ceil,
    Error, Result,
};
use byteorder::{BigEndian, ByteOrder};
use positioned_io::ReadAt;
use std::{collections::HashSet, path::PathBuf};

pub(crate) const MAGIC: [u8; 4] = *b"QFI\xfb";

#[derive(Debug, Default)]
pub(crate) struct Header {
    pub version: u32,
    pub cluster_bits: u32,
    pub l2_bits: u32,
    pub size: u64,
    pub encryption: u32,
    pub l1_size: u64,
    pub l1_offset: u64,
    pub snapshots: u32,
    pub incompatible: u64,
    pub compatible: u64,
    pub autoclear: u64,
    pub backing_name: Option<PathBuf>,
    pub backing_format: Option<String>,
}

impl Header {
    pub fn read(io: &impl ReadAt, source_size: Option<u64>) -> Result<Self> {
        let mut bytes = [0; 104];
        io.read_exact_at(0, &mut bytes[..8])?;
        if bytes[..4] != MAGIC {
            return Err(Error::FileType);
        }
        let version = BigEndian::read_u32(&bytes[4..8]);
        let minimum = match version {
            1 => 48,
            2 => 72,
            3 => 104,
            _ => return Err(Error::Version(version)),
        };
        io.read_exact_at(8, &mut bytes[8..minimum])?;
        let backing_offset = BigEndian::read_u64(&bytes[8..16]);
        let backing_size = BigEndian::read_u32(&bytes[16..20]) as u64;
        let cluster_bits = if version == 1 {
            bytes[32] as u32
        } else {
            BigEndian::read_u32(&bytes[20..24])
        };
        if !(9..=21).contains(&cluster_bits) {
            return Err(Error::FileFormat(format!(
                "invalid cluster bits: {cluster_bits}"
            )));
        }
        let l2_bits = if version == 1 {
            bytes[33] as u32
        } else {
            cluster_bits - 3
        };
        if l2_bits == 0 || l2_bits > 21 {
            return Err(Error::FileFormat(format!("invalid L2 bits: {l2_bits}")));
        }
        let mut header = Self {
            version,
            cluster_bits,
            l2_bits,
            size: BigEndian::read_u64(&bytes[24..32]),
            encryption: BigEndian::read_u32(if version == 1 {
                &bytes[36..40]
            } else {
                &bytes[32..36]
            }),
            l1_offset: BigEndian::read_u64(&bytes[40..48]),
            ..Self::default()
        };
        if header.size > i64::MAX as u64 {
            return Err(Error::FileFormat(
                "media size exceeds seekable range".into(),
            ));
        }
        match header.encryption {
            0 | 1 => {}
            2 => return Err(Error::UnsupportedFeature("QCOW LUKS encryption".into())),
            value => {
                return Err(Error::UnsupportedFeature(format!(
                    "encryption method {value}"
                )))
            }
        }
        let required_entries = div_ceil(header.size, header.cluster_size() * header.l2_entries());
        header.l1_size = if version == 1 {
            required_entries
        } else {
            BigEndian::read_u32(&bytes[36..40]) as u64
        };
        if header.l1_size < required_entries || header.l1_size * 8 > 32 * 1024 * 1024 {
            return Err(Error::FileFormat(
                "L1 table is too small or exceeds 32 MiB limit".into(),
            ));
        }
        // QCOW1 only requires 8-byte L1 alignment.
        let alignment = if version == 1 {
            8
        } else {
            header.cluster_size()
        };
        if !header.l1_offset.is_multiple_of(alignment)
            || (header.l1_size != 0 && header.l1_offset < minimum as u64)
        {
            return Err(Error::FileFormat("invalid L1 table offset".into()));
        }
        check_range(
            header.l1_offset,
            header.l1_size * 8,
            source_size,
            "L1 table",
        )?;
        if version != 1 {
            let refcount_offset = BigEndian::read_u64(&bytes[48..56]);
            let refcount_clusters = BigEndian::read_u32(&bytes[56..60]) as u64;
            let snapshots_offset = BigEndian::read_u64(&bytes[64..72]);
            header.snapshots = BigEndian::read_u32(&bytes[60..64]);
            if refcount_offset % header.cluster_size() != 0
                || snapshots_offset % header.cluster_size() != 0
            {
                return Err(Error::FileFormat(
                    "unaligned refcount or snapshot table".into(),
                ));
            }
            if refcount_clusters != 0 {
                if refcount_offset < header.cluster_size() {
                    return Err(Error::FileFormat("refcount table overlaps header".into()));
                }
                check_range(
                    refcount_offset,
                    refcount_clusters * header.cluster_size(),
                    source_size,
                    "refcount table",
                )?;
            }
            if header.snapshots != 0 {
                if snapshots_offset < header.cluster_size() {
                    return Err(Error::FileFormat("snapshot table overlaps header".into()));
                }
                check_range(
                    snapshots_offset,
                    header.snapshots as u64 * 40,
                    source_size,
                    "snapshot headers",
                )?;
            }
        }
        let mut header_length = minimum as u64;
        if version == 3 {
            header.incompatible = BigEndian::read_u64(&bytes[72..80]);
            header.compatible = BigEndian::read_u64(&bytes[80..88]);
            header.autoclear = BigEndian::read_u64(&bytes[88..96]);
            let refcount_order = BigEndian::read_u32(&bytes[96..100]);
            header_length = BigEndian::read_u32(&bytes[100..104]) as u64;
            if header_length < 104
                || !header_length.is_multiple_of(8)
                || header_length > header.cluster_size()
                || refcount_order > 6
            {
                return Err(Error::FileFormat(
                    "invalid v3 header length or refcount order".into(),
                ));
            }
            check_range(0, header_length, source_size, "header")?;
            if header_length > 104 {
                let mut compression = [0];
                io.read_exact_at(104, &mut compression)?;
                if compression[0] != 0 {
                    return Err(Error::UnsupportedFeature(format!(
                        "compression type {}",
                        compression[0]
                    )));
                }
            }
            if header.incompatible & 8 != 0 {
                return Err(Error::FileFormat(
                    "compression feature requires a nonzero compression type".into(),
                ));
            }
        }
        if backing_offset != 0 {
            let limit = if version == 1 {
                source_size.unwrap_or(u64::MAX)
            } else {
                header.cluster_size()
            };
            if backing_offset < header_length
                || backing_size == 0
                || backing_size > 1023
                || backing_offset
                    .checked_add(backing_size)
                    .is_none_or(|end| end > limit)
            {
                return Err(Error::FileFormat("invalid backing filename range".into()));
            }
            check_range(
                backing_offset,
                backing_size,
                source_size,
                "backing filename",
            )?;
            let mut name = vec![0; backing_size as usize];
            io.read_exact_at(backing_offset, &mut name)?;
            if name.contains(&0) {
                return Err(Error::FileFormat("NUL in backing filename".into()));
            }
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                header.backing_name = Some(std::ffi::OsString::from_vec(name).into());
            }
            #[cfg(not(unix))]
            {
                header.backing_name = Some(
                    String::from_utf8(name)
                        .map_err(|_| Error::FileFormat("backing filename is not UTF-8".into()))?
                        .into(),
                );
            }
        }
        let mut feature_names = FeatureNameTable::default();
        if version != 1 {
            let limit = if backing_offset == 0 {
                header.cluster_size()
            } else {
                backing_offset
            };
            let mut position = header_length;
            let mut seen = HashSet::new();
            while position < limit {
                if limit - position < 8 {
                    return Err(Error::FileFormat("truncated extension header".into()));
                }
                let mut record = [0; 8];
                io.read_exact_at(position, &mut record)?;
                let code = BigEndian::read_u32(&record[..4]);
                let length = BigEndian::read_u32(&record[4..]) as u64;
                position += 8;
                if code == 0 {
                    if length != 0 {
                        return Err(Error::FileFormat("nonempty extension terminator".into()));
                    }
                    break;
                }
                let padded = div_ceil(length, 8) * 8;
                if !seen.insert(code) || padded > limit - position {
                    return Err(Error::FileFormat(
                        "duplicate or out-of-bounds header extension".into(),
                    ));
                }
                check_range(position, padded, source_size, "header extension")?;
                let mut data = vec![0; length as usize];
                io.read_exact_at(position, &mut data)?;
                match code {
                    0x6803_f857 => feature_names.read(&mut data.as_slice())?,
                    0xe279_2aca => {
                        if data.is_empty() || data.contains(&0) {
                            return Err(Error::FileFormat("invalid backing format name".into()));
                        }
                        header.backing_format = Some(String::from_utf8(data).map_err(|_| {
                            Error::FileFormat("invalid backing format encoding".into())
                        })?);
                    }
                    _ => {}
                }
                position += padded;
            }
            if header.incompatible & 2 != 0 {
                return Err(Error::UnsupportedFeature("corrupt image flag".into()));
            }
            let mut features = Feature::new(
                FeatureKind::Incompatible,
                &[
                    "dirty",
                    "corrupt",
                    "external data file",
                    "compression type",
                    "extended L2 entries",
                ],
            );
            features.set(header.incompatible & !1);
            if features.bits() != 0 {
                return Err(Error::UnsupportedFeature(
                    features.to_string(&feature_names),
                ));
            }
        }
        Ok(header)
    }
    pub fn cluster_size(&self) -> u64 {
        1 << self.cluster_bits
    }
    pub fn l2_entries(&self) -> u64 {
        1 << self.l2_bits
    }
}

pub(crate) fn check_range(
    position: u64,
    length: u64,
    source_size: Option<u64>,
    context: &str,
) -> Result<()> {
    let end = position
        .checked_add(length)
        .ok_or_else(|| Error::FileFormat(format!("{context} offset overflow")))?;
    if source_size.is_some_and(|size| end > size) || end > i64::MAX as u64 {
        return Err(Error::FileFormat(format!(
            "{context} extends beyond source"
        )));
    }
    Ok(())
}
