// TODO: remove once the driver is complete
#![allow(dead_code)]

use std::{
    ffi::{OsStr, OsString},
    fmt::{self, Display, Formatter},
    mem::size_of,
    time::SystemTime,
};

use bincode_next::{Decode, Encode};

/// UFS2 fast filesystem magic number
pub const FS_UFS2_MAGIC: i32 = 0x19540119;

/// Offset of the magic number in the superblock
pub const MAGIC_OFFSET: u64 = 1372;

/// Magic number of a CylGroup
pub const CG_MAGIC: i32 = 0x090255;

/// Location of the superblock on UFS2.
pub const SBLOCK_UFS2: usize = 65536;

/// Size of a superblock
pub const SBLOCKSIZE: usize = 8192;

/// Size of the CylGroup structure.
pub const CGSIZE: usize = 32768;

/// Max number of fragments per block.
pub const MAXFRAG: usize = 8;

/// `ufs_time_t` on FreeBSD
pub type UfsTime = i64;

/// `ufs2_daddr_t` on FreeBSD
pub type UfsDaddr = i64;

/// Size of a directory block.
pub const DIRBLKSIZE: usize = 512;

/// UFS-native inode number type
#[derive(Debug, Decode, Encode, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct InodeNum(u32);
impl InodeNum {
    /// The inode number of the root directory (`/`) of the filesystem.
    pub const ROOT: Self = Self(2);

    /// Get the numeric value.
    pub fn get(&self) -> u32 {
        self.0
    }

    /// The same as `.get()`, but returns u64.
    pub fn get64(&self) -> u64 {
        self.0.into()
    }

    /// Create a new inode number.
    /// # Safety
    /// `inr` must be a valid inode number
    pub unsafe fn new(inr: u32) -> Self {
        Self(inr)
    }
}

/// The path name on which the filesystem is mounted is maintained
/// in fs_fsmnt. MAXMNTLEN defines the amount of space allocated in
/// the super block for this name.
pub const MAXMNTLEN: usize = 468;

/// The volume name for this filesystem is maintained in fs_volname.
/// MAXVOLLEN defines the length of the buffer allocated.
pub const MAXVOLLEN: usize = 32;

/// The maximum number of snapshot nodes that can be associated
/// with each filesystem. This limit affects only the number of
/// snapshot files that can be recorded within the superblock so
/// that they can be found when the filesystem is mounted. However,
/// maintaining too many will slow the filesystem performance, so
/// having this limit is a good idea.
pub const FSMAXSNAP: usize = 20;

/// There is a 128-byte region in the superblock reserved for in-core
/// pointers to summary information. Originally this included an array
/// of pointers to blocks of struct csum; now there are just a few
/// pointers and the remaining space is padded with fs_ocsp[].
///
/// NOCSPTRS determines the size of this padding. Historically this
/// space was used to store pointers to structures that summaried
/// filesystem usage and layout information. However, these pointers
/// left various kernel pointers in the superblock which made otherwise
/// identical superblocks appear to have differences. So, all the
/// pointers in the superblock were moved to a fs_summary_info structure
/// reducing the superblock to having only a single pointer to this
/// structure. When writing the superblock to disk, this pointer is
/// temporarily NULL'ed out so that the kernel pointer will not appear
/// in the on-disk copy of the superblock.
pub const NOCSPTRS: usize = (128 / size_of::<usize>()) - 1;

/// External addresses in inode.
pub const UFS_NXADDR: usize = 2;

/// Direct addresses in inode.
pub const UFS_NDADDR: usize = 12;

/// Maximum length of a file name.
pub const UFS_MAXNAMELEN: usize = 255;

/// Indirect addresses in inode.
pub const UFS_NIADDR: usize = 3;

/// Length of a short link.
pub const UFS_SLLEN: usize = (UFS_NDADDR + UFS_NIADDR) * size_of::<UfsDaddr>();

/// Size of an on-disk inode.
pub const UFS_INOSZ: usize = 256;

/// Maximum length of an extattr name.
pub const UFS_EXTATTR_MAXNAMELEN: usize = 64; // excluding null

/// type of file mask
pub const S_IFMT: u16 = 0o170000;

/// named pipe (fifo)
pub const S_IFIFO: u16 = 0o010000;

/// character special
pub const S_IFCHR: u16 = 0o020000;

/// directory
pub const S_IFDIR: u16 = 0o040000;

/// block special
pub const S_IFBLK: u16 = 0o060000;

/// regular
pub const S_IFREG: u16 = 0o100000;

/// symbolic link
pub const S_IFLNK: u16 = 0o120000;

/// socket
pub const S_IFSOCK: u16 = 0o140000;

pub const DT_UNKNOWN: u8 = 0;
pub const DT_FIFO: u8 = 1;
pub const DT_CHR: u8 = 2;
pub const DT_DIR: u8 = 4;
pub const DT_BLK: u8 = 6;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;
pub const DT_SOCK: u8 = 12;
pub const DT_WHT: u8 = 14;

/// Per cylinder group information; summarized in blocks allocated
/// from first cylinder group data blocks.  These blocks have to be
/// read in from fs_csaddr (size fs_cssize) in addition to the
/// super block.
/// `struct csum` in FreeBSD
#[derive(Debug, Decode, Encode)]
pub struct Csum {
    pub ndir: i32,   // number of directories
    pub nbfree: i32, // number of free blocks
    pub nifree: i32, // number of free inodes
    pub nffree: i32, // number of free frags
}

/// `struct csum_total` in FreeBSD
#[derive(Debug, Decode, Encode)]
pub struct CsumTotal {
    pub ndir: i64,        // number of directories
    pub nbfree: i64,      // number of free blocks
    pub nifree: i64,      // number of free inodes
    pub nffree: i64,      // number of free frags
    pub numclusters: i64, // number of free clusters
    pub spare: [i64; 3],  // future expansion
}

/// Super block for an FFS filesystem.
/// `struct fs` in FreeBSD
#[derive(Debug, Decode, Encode)]
pub struct Superblock {
    pub firstfield: i32,   // historic filesystem linked list,
    pub unused_1: i32,     // used for incore super blocks
    pub sblkno: i32,       // offset of super-block in filesys
    pub cblkno: i32,       // offset of cyl-block in filesys
    pub iblkno: i32,       // offset of inode-blocks in filesys
    pub dblkno: i32,       // offset of first data after cg
    pub old_cgoffset: i32, // cylinder group offset in cylinder
    pub old_cgmask: i32,   // used to calc mod fs_ntrak
    pub old_time: i32,     // last time written
    pub old_size: i32,     // number of blocks in fs
    pub old_dsize: i32,    // number of data blocks in fs
    pub ncg: u32,          // number of cylinder groups
    pub bsize: i32,        // size of basic blocks in fs
    pub fsize: i32,        // size of frag blocks in fs
    pub frag: i32,         // number of frags in a block in fs
    // these are configuration parameters
    pub minfree: i32,      // minimum percentage of free blocks
    pub old_rotdelay: i32, // num of ms for optimal next block
    pub old_rps: i32,      // disk revolutions per second
    // these fields can be computed from the others
    pub bmask: i32,  // ``blkoff'' calc of blk offsets
    pub fmask: i32,  // ``fragoff'' calc of frag offsets
    pub bshift: i32, // ``lblkno'' calc of logical blkno
    pub fshift: i32, // ``numfrags'' calc number of frags
    // these are configuration parameters
    pub fs_maxcontig: i32, // max number of contiguous blks
    pub fs_maxbpg: i32,    // max number of blks per cyl group
    // these fields can be computed from the others
    pub fragshift: i32,   // block to frag shift
    pub fsbtodb: i32,     // fsbtodb and dbtofsb shift constant
    pub sbsize: i32,      // actual size of super block
    pub spare1: [i32; 2], // old fs_csmask
    // old fs_csshift
    pub nindir: i32,   // value of NINDIR
    pub inopb: u32,    // value of INOPB
    pub old_nspf: i32, // value of NSPF
    // yet another configuration parameter
    pub optim: i32,          // optimization preference, see below
    pub old_npsect: i32,     // # sectors/track including spares
    pub old_interleave: i32, // hardware sector interleave
    pub old_trackskew: i32,  // sector 0 skew, per track
    pub id: [i32; 2],        // unique filesystem id
    // sizes determined by number of cylinder groups and their sizes
    pub old_csaddr: i32, // blk addr of cyl grp summary area
    pub cssize: i32,     // size of cyl grp summary area
    pub cgsize: i32,     // cylinder group size
    pub spare2: i32,     // old fs_ntrak
    pub old_nsect: i32,  // sectors per track
    pub old_spc: i32,    // sectors per cylinder
    pub old_ncyl: i32,   // cylinders in filesystem
    pub old_cpg: i32,    // cylinders per group
    pub ipg: u32,        // inodes per group
    pub fpg: i32,        // blocks per group * fs_frag
    // this data must be re-computed after crashes
    pub old_cstotal: Csum, // cylinder summary information
    // these fields are cleared at mount time
    pub fmod: i8,                 // super block modified flag
    pub clean: i8,                // filesystem is clean flag
    pub ronly: i8,                // mounted read-only flag
    pub old_flags: i8,            // old FS_ flags
    pub fsmnt: [u8; MAXMNTLEN],   // name mounted on
    pub volname: [u8; MAXVOLLEN], // volume name
    pub swuid: u64,               // system-wide uid
    pub pad: i32,                 // due to alignment of fs_swuid
    // these fields retain the current block allocation info
    pub cgrotor: i32,               // last cg searched
    pub ocsp: [usize; NOCSPTRS],    // padding; was list of fs_cs buffers
    pub si: usize,                  // In-core pointer to summary info
    pub old_cpc: i32,               // cyl per cycle in postbl
    pub maxbsize: i32,              // maximum blocking factor permitted
    pub unrefs: i64,                // number of unreferenced inodes
    pub providersize: i64,          // size of underlying GEOM provider
    pub metaspace: i64,             // size of area reserved for metadata
    pub sparecon64: [i64; 13],      // old rotation block list head
    pub sblockactualloc: i64,       // byte offset of this superblock
    pub sblockloc: i64,             // byte offset of standard superblock
    pub cstotal: CsumTotal,         // (u) cylinder summary information
    pub time: UfsTime,              // last time written
    pub size: i64,                  // number of blocks in fs
    pub dsize: i64,                 // number of data blocks in fs
    pub csaddr: UfsDaddr,           // blk addr of cyl grp summary area
    pub pendingblocks: i64,         // (u) blocks being freed
    pub pendinginodes: u32,         // (u) inodes being freed
    pub snapinum: [u32; FSMAXSNAP], // list of snapshot inode numbers
    pub avgfilesize: u32,           // expected average file size
    pub avgfpdir: u32,              // expected # of files per directory
    pub save_cgsize: i32,           // save real cg size to use fs_bsize
    pub mtime: UfsTime,             // Last mount or fsck time.
    pub sujfree: i32,               // SUJ free list
    pub sparecon32: [i32; 21],      // reserved for future constants
    pub ckhash: u32,                // if CK_SUPERBLOCK, its check-hash
    pub metackhash: u32,            // metadata check-hash, see CK_ below
    pub flags: i32,                 // see FS_ flags below
    pub contigsumsize: i32,         // size of cluster summary array
    pub maxsymlinklen: i32,         // max length of an internal symlink
    pub old_inodefmt: i32,          // format of on-disk inodes
    pub maxfilesize: u64,           // maximum representable file size
    pub qbmask: i64,                // ~fs_bmask for use with 64-bit size
    pub qfmask: i64,                // ~fs_fmask for use with 64-bit size
    pub state: i32,                 // validate fs_clean field
    pub old_postblformat: i32,      // format of positional layout tables
    pub old_nrpos: i32,             // number of rotational positions
    pub spare5: [i32; 2],           // old fs_postbloff
    // old fs_rotbloff
    pub magic: i32, // magic number
}

#[derive(Debug, Decode, Encode)]
#[allow(dead_code)]
pub struct CylGroup {
    pub firstfield: i32,       // historic cyl groups linked list
    pub magic: i32,            // magic number
    pub old_time: i32,         // time last written
    pub cgx: u32,              // we are the cgx'th cylinder group
    pub old_ncyl: i16,         // number of cyl's this cg
    pub old_niblk: i16,        // number of inode blocks this cg
    pub ndblk: u32,            // number of data blocks this cg
    pub cs: Csum,              // cylinder summary information
    pub rotor: u32,            // position of last used block
    pub frotor: u32,           // position of last used frag
    pub irotor: u32,           // position of last used inode
    pub frsum: [u32; MAXFRAG], // counts of available frags
    pub old_btotoff: i32,      // (int32) block totals per cylinder
    pub old_boff: i32,         // (uint16) free block positions
    pub iusedoff: u32,         // (ui8) used inode map
    pub freeoff: u32,          // (ui8) free block map
    pub nextfreeoff: u32,      // (ui8) next available space
    pub clustersumoff: u32,    // (ui32) counts of avail clusters
    pub clusteroff: u32,       // (ui8) free cluster map
    pub nclusterblks: u32,     // number of clusters this cg
    pub niblk: u32,            // number of inode blocks this cg
    pub initediblk: u32,       // last initialized inode
    pub unrefs: u32,           // number of unreferenced inodes
    pub sparecon32: [i32; 1],  // reserved for future use
    pub ckhash: u32,           // check-hash of this cg
    pub time: UfsTime,         // time last written
    pub sparecon64: [i64; 3],  // reserved for future use
                               // actually longer - space used for cylinder group maps
}

#[derive(Debug, Default, Clone, Decode, Encode)]
pub struct InodeBlocks {
    pub direct: [UfsDaddr; UFS_NDADDR],
    pub indirect: [UfsDaddr; UFS_NIADDR],
}

#[derive(Debug, Clone)]
pub enum InodeData {
    Blocks(InodeBlocks),
    Shortlink([u8; UFS_SLLEN]),
}

#[allow(dead_code)]
#[derive(Debug, Encode)]
pub struct Inode {
    pub mode: u16,                    //   0: IFMT, permissions; see below.
    pub nlink: u16,                   //   2: File link count.
    pub uid: u32,                     //   4: File owner.
    pub gid: u32,                     //   8: File group.
    pub blksize: u32,                 //  12: Inode blocksize.
    pub size: u64,                    //  16: File byte count.
    pub blocks: u64,                  //  24: Blocks actually held.
    pub atime: UfsTime,               //  32: Last access time.
    pub mtime: UfsTime,               //  40: Last modified time.
    pub ctime: UfsTime,               //  48: Last inode change time.
    pub birthtime: UfsTime,           //  56: Inode creation time.
    pub mtimensec: u32,               //  64: Last modified time.
    pub atimensec: u32,               //  68: Last access time.
    pub ctimensec: u32,               //  72: Last inode change time.
    pub birthnsec: u32,               //  76: Inode creation time.
    pub r#gen: u32,                   //  80: Generation number.
    pub kernflags: u32,               //  84: Kernel flags.
    pub flags: u32,                   //  88: Status flags (chflags).
    pub extsize: u32,                 //  92: External attributes size.
    pub extb: [UfsDaddr; UFS_NXADDR], //  96: External attributes block.
    pub data: InodeData,              // XXX: Blocks
    pub modrev: u64,                  // 232: i_modrev for NFSv4
    pub ignored: u32, // 240: (SUJ: Next unlinked inode) or (IFDIR: depth from root dir)
    pub ckhash: u32,  // 244: if CK_INODE, its check-hash
    pub spare: [u32; 2], // 248: Reserved; currently unused
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeType {
    RegularFile,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    Socket,
    NamedPipe,
    //Whiteout,
}

/// Inode Metadata
#[derive(Debug)]
#[doc(alias = "Stat")]
pub struct InodeAttr {
    /// Inode number.
    pub inr: InodeNum,

    /// UNIX Permissions.
    pub perm: u16,

    /// Type of the inode.
    pub kind: InodeType,

    /// Inode data size.
    pub size: u64,

    /// Total number of blocks allocated.
    pub blocks: u64,

    /// Access time.
    pub atime: SystemTime,

    /// Modify time.
    pub mtime: SystemTime,

    /// Change time.
    pub ctime: SystemTime,

    /// Birth time.
    pub btime: SystemTime,

    /// Number of hard links.
    pub nlink: u16,

    /// User ID.
    pub uid: u32,

    /// Group ID.
    pub gid: u32,

    /// Inode Generation.
    pub r#gen: u32,

    /// Block size.
    pub blksize: u32,

    /// Additional inode flags (like immutable).
    pub flags: u32,

    /// FreeBSD kernel flags.
    pub kernflags: u32,

    /// Size of the extended attribute area.
    pub extsize: u32,
}

pub enum InodeBlock {
    Direct(usize),
    Indirect1(usize),
    Indirect2(usize, usize),
    Indirect3(usize, usize, usize),
}

#[derive(Debug, Clone, Copy, Decode, PartialEq, Eq)]
#[repr(u8)]
pub enum ExtattrNamespace {
    Empty = 0,
    User = 1,
    System = 2,
}

#[derive(Debug, Decode)]
pub struct ExtattrHeader {
    pub len: u32,
    pub namespace: u8,
    pub contentpadlen: u8,
    pub namelen: u8,
}

#[derive(Debug)]
pub struct BlockInfo {
    /// offset from the start of the block
    pub off: u64,

    /// block index in the inode
    pub blkidx: u64,

    /// size of the block
    pub size: u64,
}

impl Superblock {
    /// Calculate the size of a cylinder group.
    pub fn cgsize(&self) -> u64 {
        self.fpg as u64 * self.fsize as u64
    }

    /// Calculate the size of a cylinder group structure.
    pub fn cgsize_struct(&self) -> usize {
        // TODO: size_of() is not valid
        size_of::<CylGroup>()
            + howmany(self.fpg as usize, 8)
            + howmany(self.ipg as usize, 8)
            + size_of::<i32>()
            + (if self.contigsumsize <= 0 {
                0usize
            } else {
                self.contigsumsize as usize * size_of::<i32>()
                    + howmany(self.fpg as usize >> (self.fshift as usize), 8)
            })
    }

    /// inode number to cylinder group number.
    pub fn ino_to_cg(&self, inr: InodeNum) -> u64 {
        inr.get64() / self.ipg as u64
    }

    /// inode number to cylinder group number and offset.
    pub fn ino_in_cg(&self, inr: InodeNum) -> (u64, u64) {
        let ipg = self.ipg as u64;
        let inr = inr.get64();
        (inr / ipg, inr % ipg)
    }

    pub fn blocks_to_frags(&self, blocks: u64) -> u64 {
        blocks << self.fragshift as u32
    }

    pub fn ino_to_fso(&self, inr: InodeNum) -> u64 {
        let ipg = self.ipg as u64;
        let fpg = self.fpg as u64;
        let fs = self.fsize as u64;
        let cgi = inr.get64() / ipg;
        let off = inr.get64() % ipg;
        let cgstart = cgi * fpg * fs;
        let cgistart = cgstart + (self.iblkno as u64 * fs);
        cgistart + (off * UFS_INOSZ as u64)
    }
}

const fn howmany(x: usize, y: usize) -> usize {
    x.div_ceil(y)
}

impl ExtattrHeader {
    pub fn namespace(&self) -> Option<ExtattrNamespace> {
        match self.namespace {
            0 => Some(ExtattrNamespace::Empty),
            1 => Some(ExtattrNamespace::User),
            2 => Some(ExtattrNamespace::System),
            _ => None,
        }
    }
}

impl ExtattrNamespace {
    pub fn with_name(self, name: &OsStr) -> OsString {
        let ns = match self {
            Self::Empty => "",
            Self::User => "user.",
            Self::System => "system.",
        };
        let mut out = OsString::from(ns);
        out.push(name);
        out
    }
}

impl Display for InodeNum {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
