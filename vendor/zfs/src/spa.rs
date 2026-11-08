//!
//! Datastructures used by the Storage Pool Alocator
//!
use std::convert::TryFrom;
use std::fmt;

use nom::{number::complete as number, IResult};

use enum_repr_derive::TryFrom;

use crate::fletcher::{Fletcher2, Fletcher4};

/// The disk label contains the top level metadata for the filesystem
///
/// There are four copies of the disk label stored per disk, two at the start and two at the
/// end of the disk.
///
/// The `nv_pairs` contains metadata about the disk including options used when creating the
/// filesystem. Parsing this datastructure is not currently implemented
///
/// The `uberblocks` array contains the most recent uberblocks written, each of which includes a
/// `BlockPtr` referencing the top level meta object set
// TODO: implement boot_header and nv_pairs parsing
#[derive(Debug, Clone)]
pub struct Label {
    pub boot_header: Vec<u8>,
    pub nv_pairs: Vec<u8>,
    pub uberblocks: Vec<Uberblock>,
}

impl Label {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (_pad, boot_header, nv_pairs, uberblocks)) = nom::sequence::tuple((
            nom::bytes::complete::take(8 * 1024usize),
            nom::bytes::complete::take(8 * 1024usize),
            nom::bytes::complete::take(112 * 1024usize),
            nom::bytes::complete::take(128 * 1024usize),
        ))(input)?;
        let asize = 12; // TODO: read this from nv_pairs
        let uberblock_count = 1 << (17 - asize.max(10));
        let uberblock_size = 1 << asize.max(10);
        let (_, uberblocks) = nom::multi::count(
            |i| Uberblock::parse(i, uberblock_size),
            uberblock_count,
        )(uberblocks)?;
        Ok((
            input,
            Self {
                boot_header: boot_header.to_vec(),
                nv_pairs: nv_pairs.to_vec(),
                uberblocks,
            },
        ))
    }
}

/// The uberblock points to the meta object set for the current transaction group
#[derive(Debug, Clone)]
pub struct Uberblock {
    /// Always 0x00BAB10C (oo-ba-block)
    pub magic: u64,
    /// Version of the on disk format
    pub version: u64,
    /// The transaction group this uberblock is for
    pub txg: u64,
    /// Sum of the GUIDs for all vdevs in this pool
    pub guid_sum: u64,
    /// number of seconds since 1970 when this uberblock was written
    pub timestamp: u64,
    /// BlockPtr pointing to the meta object set
    pub rootbp: BlockPtr,
    pub software_version: u64,
    pub mmp_magic: u64,
    pub mmp_delay: u64,
    pub mmp_config: u64,
    pub checkpoint_txg: u64,
}

impl Uberblock {
    pub fn parse(input: &[u8], size: usize) -> IResult<&[u8], Self> {
        let (input, block) = nom::bytes::complete::take(size)(input)?;
        let (
            _block,
            (
                magic,
                version,
                txg,
                guid_sum,
                timestamp,
                rootbp,
                software_version,
                mmp_magic,
                mmp_delay,
                mmp_config,
                checkpoint_txg,
            ),
        ) = nom::sequence::tuple((
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            BlockPtr::parse,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
        ))(block)?;
        Ok((
            input,
            Self {
                magic,
                version,
                txg,
                guid_sum,
                timestamp,
                rootbp,
                software_version,
                mmp_magic,
                mmp_delay,
                mmp_config,
                checkpoint_txg,
            },
        ))
    }
}

/// Data Virtual Address, a raw pointer of sorts to a location on disk
#[derive(Copy, Clone)]
pub struct DVA {
    /// The id of the vdev the pointed at blocks reside on
    pub vdev: u32,
    /// Something to do with raidz
    pub grid: u8,
    /// Allocated size (of what, I'm not certain)
    pub asize: u32,
    /// Offset into the vdev the blocks reside at
    pub offset: u64,
    /// Whether or not this is a gang pointer, used to fragment a block when a large enough
    /// chunk of contiguous free space can't be found
    pub gang: bool,
}

impl fmt::Debug for DVA {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DVA")
            .field("vdev", &self.vdev)
            .field("grid", &self.grid)
            .field("asize", &self.asize)
            .field("offset", &format!("{:x}", self.offset << 9))
            .field("gang", &self.gang)
            .finish()
    }
}

impl DVA {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (asize, grid, vdev)) =
            nom::sequence::tuple((number::le_u24, number::le_u8, number::le_u32))(input)?;
        let (input, offset_gang) = number::le_u64(input)?;
        let gang = offset_gang & (1 << 63);
        let offset = offset_gang & ((1 << 63) - 1);
        //let (input, (offset, gang)): (_, (u64, u8)) =
        //    bits!(input, tuple!(take_bits!(63usize), take_bits!(1usize)))?;
        let gang = if gang == 0 { false } else { true };
        Ok((
            input,
            Self {
                vdev,
                grid,
                asize,
                offset,
                gang,
            },
        ))
    }
}

/// A checksum of bytes pointed to by a blockptr
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Checksum {
    pub checksum: [u64; 4],
}

impl From<Fletcher4> for Checksum {
    fn from(f: Fletcher4) -> Self {
        Self {
            checksum: [f.a, f.b, f.c, f.d],
        }
    }
}

impl From<Fletcher2> for Checksum {
    fn from(f: Fletcher2) -> Self {
        Self {
            checksum: [f.a0, f.a1, f.b0, f.b1],
        }
    }
}

impl Checksum {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (c1, c2, c3, c4)) = nom::sequence::tuple((
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
        ))(input)?;
        Ok((
            input,
            Self {
                checksum: [c1, c2, c3, c4],
            },
        ))
    }
}

/// Compression algorithm to use
#[repr(u8)]
#[derive(Debug, Clone, TryFrom, Eq, PartialEq)]
pub enum CompressionType {
    Inherit = 0,
    On = 1,
    Off = 2,
    LZJB = 3,
    Empty = 4,
    GZIP1 = 5,
    GZIP2 = 6,
    GZIP3 = 7,
    GZIP4 = 8,
    GZIP5 = 9,
    GZIP6 = 10,
    GZIP7 = 11,
    GZIP8 = 12,
    GZIP9 = 13,
    ZLE = 14,
    LZ4 = 15,
}

impl CompressionType {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        nom::combinator::map_res(number::le_u8, |x| Self::try_from(x))(input)
    }
}

/// Checksum algorithm to use
#[repr(u8)]
#[derive(Debug, Clone, TryFrom, Eq, PartialEq)]
pub enum ChecksumType {
    Inherit = 0,
    On = 1,
    Off = 2,
    Label = 3,
    GangHeader = 4,
    ZILog = 5,
    Fletcher2 = 6,
    Fletcher4 = 7,
    SHA256 = 8,
    ZILog2 = 9,
    NoParity = 10,
    SHA512 = 11,
    Skein = 12,
}

impl ChecksumType {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        nom::combinator::map_res(number::le_u8, |x| Self::try_from(x))(input)
    }
}

/// A pointer to other blocks on disk
///
/// The integrity of the data pointed at can be verified using the checksum stored in the
/// `BlockPtr`
///
/// The data may be compressed using the algorithm specified in `compression_type`
///
/// The data may also be stored inline in the block pointer itself
#[derive(Debug, Clone)]
pub struct BlockPtr {
    /// Either the inline data or the pointer data
    pub embedded: BlockPtrKind,
    /// The endianness of the data pointed at
    pub byteorder: bool,
    pub dedup: bool,
    /// Whether the data is encrypted or not (the block pointer layout is different in this case
    /// but encyrption is not currently supported)
    pub encryption: bool,
    /// What type of data is pointed at
    pub kind: u8,
    /// What compression algorithm is used to decompress the datga
    pub compression_type: CompressionType,
    /// Whether data is stored inline in the `BlockPtr` or not
    pub embedded_data: bool,
    /// How many layers of indirect blocks before we reach data blocks
    pub indirection_level: u8,
    /// The size on disk of the data pointed at. Stored as number of 512 byte sectors, **minus 1**, so
    /// a `physical_size` of 0 = 512 bytes
    pub physical_size: u16,
    /// The size of the data pointed at, after decompression/decryption. Stored as number of 512
    /// byte sectors, **minus 1**, so a `logical_size` of 0 = 512 bytes
    pub logical_size: u16,
    /// The transcation group this block was created in
    pub logical_transaction: u64,
}

/// Fields of the block pointer present when data is not stored inline
#[derive(Debug, Clone)]
pub struct BlockPtrPtr {
    /// Pointers to the data on disk. Each `DVA` points to the same data, stored in a different
    /// location, for redundancy. If only one or two `DVA`s are used the others will be zeroed
    pub addresses: [DVA; 3],
    /// The transcation group the space for this block was allocated in
    pub physical_transaction: u64,
    /// What checksum type was used
    pub checksum_type: ChecksumType,
    pub fill_count: u64,
    /// The checksum itself
    pub checksum: Checksum,
}

/// Whether this `BlockPtr` has data stored inline or not
#[derive(Debug, Clone)]
pub enum BlockPtrKind {
    Ptr(BlockPtrPtr),
    Data { embedded_type: u8, data: Vec<u8> },
}

impl BlockPtr {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (_pad, flags)) = nom::combinator::peek(nom::sequence::tuple((
            nom::bytes::complete::take(6 * 8usize),
            number::le_u64,
        )))(input)?;
        let embedded_data = (flags & (1 << 39)) != 0;
        if embedded_data {
            let (input, data0) = nom::bytes::complete::take(6 * 8usize)(input)?;
            let (input, (logical_size, physical_size)) =
                nom::sequence::tuple((number::le_u24, number::le_u8))(input)?;
            let (input, compression_type_embedded) = number::le_u8(input)?;
            let embedded_data = compression_type_embedded & (1 << 7);
            let compression_type =
                CompressionType::try_from(compression_type_embedded & ((1 << 7) - 1)).unwrap();
            let embedded_data = embedded_data != 0;
            let (input, (embedded_type, kind)) =
                nom::sequence::tuple((number::le_u8, number::le_u8))(input)?;
            let (input, level_flags) = number::le_u8(input)?;
            let indirection_level = level_flags & ((1 << 5) - 1);
            let encryption = level_flags & (1 << 5) != 0;
            let dedup = level_flags & (1 << 6) != 0;
            let byteorder = level_flags & (1 << 7) != 0;
            //let (input, (indirection_level, encryption, dedup, byteorder)): (_, (_, u8, u8, u8)) =
            //    bits!(
            //        input,
            //        tuple!(
            //            take_bits!(5usize),
            //            take_bits!(1usize),
            //            take_bits!(1usize),
            //            take_bits!(1usize)
            //        )
            //    )?;
            let (input, (data1, logical_transaction, data2)) = nom::sequence::tuple((
                nom::bytes::complete::take(3 * 8usize),
                number::le_u64,
                nom::bytes::complete::take(5 * 8usize),
            ))(input)?;

            let mut data = Vec::with_capacity(data0.len() + data1.len() + data2.len());
            data.extend(data0);
            data.extend(data1);
            data.extend(data2);

            let embedded = BlockPtrKind::Data {
                embedded_type,
                data,
            };
            Ok((
                input,
                Self {
                    embedded,
                    byteorder,
                    dedup,
                    encryption,
                    kind,
                    compression_type,
                    embedded_data,
                    indirection_level,
                    physical_size: physical_size as u16 / 512,
                    logical_size: logical_size as u16 / 512,
                    logical_transaction,
                },
            ))
        } else {
            let (input, (a1, a2, a3)) =
                nom::sequence::tuple((DVA::parse, DVA::parse, DVA::parse))(input)?;
            let addresses = [a1, a2, a3];
            let (input, (logical_size, physical_size)) =
                nom::sequence::tuple((number::le_u16, number::le_u16))(input)?;
            let (input, compression_type_embedded) = number::le_u8(input)?;
            let embedded_data = compression_type_embedded & (1 << 7);
            let compression_type =
                CompressionType::try_from(compression_type_embedded & ((1 << 7) - 1)).unwrap();
            let embedded_data = embedded_data != 0;
            let (input, (checksum_type, kind)) =
                nom::sequence::tuple((ChecksumType::parse, number::le_u8))(input)?;
            let (input, level_flags) = number::le_u8(input)?;

            let indirection_level = level_flags & ((1 << 5) - 1);
            let encryption = level_flags & (1 << 5) != 0;
            let dedup = level_flags & (1 << 6) != 0;
            let byteorder = level_flags & (1 << 7) != 0;

            //let (input, (indirection_level, encryption, dedup, byteorder)): (_, (_, u8, u8, u8)) =
            //    bits!(
            //        input,
            //        tuple!(
            //            take_bits!(5usize),
            //            take_bits!(1usize),
            //            take_bits!(1usize),
            //            take_bits!(1usize)
            //        )
            //    )?;

            //let encryption = encryption != 0;
            //let dedup = dedup != 0;
            //let byteorder = byteorder != 0;
            let (input, (_pad, physical_transaction, logical_transaction, fill_count)) =
                nom::sequence::tuple((
                    nom::bytes::complete::take(16usize),
                    number::le_u64,
                    number::le_u64,
                    number::le_u64,
                ))(input)?;
            let (input, checksum) = Checksum::parse(input)?;
            let embedded = BlockPtrKind::Ptr(BlockPtrPtr {
                addresses,
                physical_transaction,
                checksum_type,
                fill_count,
                checksum,
            });
            Ok((
                input,
                Self {
                    embedded,
                    byteorder,
                    dedup,
                    encryption,
                    kind,
                    compression_type,
                    embedded_data,
                    indirection_level,
                    physical_size,
                    logical_size,
                    logical_transaction,
                },
            ))
        }
    }
}

/// Gang block header
#[derive(Debug, Clone)]
pub struct ZioGbh {
    pub blkptr: [BlockPtr; 3],
    pub tail: ZioBlockTail,
}

impl ZioGbh {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (b0, b1, b2, _pad, tail)) = nom::sequence::tuple((
            BlockPtr::parse,
            BlockPtr::parse,
            BlockPtr::parse,
            nom::bytes::complete::take(512usize - (128 * 3) - (5 * 8)),
            ZioBlockTail::parse,
        ))(input)?;
        Ok((
            input,
            Self {
                blkptr: [b0, b1, b2],
                tail,
            },
        ))
    }
}

/// Gang block tail containing checksum of the gang block
#[derive(Debug, Clone)]
pub struct ZioBlockTail {
    pub magic: u64,
    pub checksum: Checksum,
}

impl ZioBlockTail {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (magic, checksum)) =
            nom::sequence::tuple((number::le_u64, Checksum::parse))(input)?;
        Ok((input, Self { magic, checksum }))
    }
}
