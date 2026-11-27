use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{Cursor, Error as IoError, ErrorKind, Read, Result as IoResult, Seek, SeekFrom},
    mem::size_of,
    num::NonZeroU64,
};
// Lovelycarve patch: `Path` is only referenced from the Unix-only `Ufs::open`
// constructor; gate the import to match.
#[cfg(unix)]
use std::path::Path;

// Lovelycarve patch: route OsStr<->bytes through the cross-platform shim in
// `crate::os_compat` instead of the Unix-only `std::os::unix::ffi` import.
use crate::os_compat::{OsStrExt, OsStringExt};

use crate::{
    blockreader::{Backend, BlockReader},
    data::*,
    decoder::{Config, Decoder},
};

macro_rules! iobail {
	($kind:expr, $($tk:tt)+) => {
		return Err(IoError::new($kind, format!($($tk)+)))
	};
}

mod balloc;
mod dir;
mod ialloc;
mod inode;
mod symlink;
mod ufs1;
mod xattr;

/// (INTERNAL) Constructs an [`std::io::Error`] from an `errno`.
#[macro_export]
macro_rules! err {
    ($name:ident) => {
        IoError::from_raw_os_error(libc::$name)
    };
}

/// Summary of filesystem statistics.
#[derive(Debug, Clone)]
#[doc(alias = "Statfs")]
pub struct Info {
    /// Number of blocks.
    pub blocks: u64,

    /// Number of free blocks.
    pub bfree: u64,

    /// Number of inodes (files).
    pub files: u64,

    /// Number of free inodes (files).
    pub ffree: u64,

    /// Block size.
    pub bsize: u32,

    /// Fragment size.
    pub fsize: u32,
}

/// Berkeley Fast File System (UFS1 or UFS2).
pub struct Ufs<R: Backend> {
    file: Decoder<BlockReader<R>>,
    superblock: Superblock,
    source_size: u64,
}

impl Ufs<File> {
    // Lovelycarve patch: gate the `Path`-based convenience constructor behind
    // `cfg(unix)` since `BlockReader::open` is Unix-only. Cross-platform
    // callers go through `Ufs::new(BlockReader::new(cursor, bs, false))`.
    #[cfg(unix)]
    pub fn open(path: &Path, rw: bool) -> IoResult<Self> {
        let file = BlockReader::open(path, rw)?;
        Self::new(file)
    }
}

impl<R: Backend> Ufs<R> {
    pub fn new(mut file: BlockReader<R>) -> IoResult<Self> {
        let source_size = file.source_len()?;
        for (offset, expected_magic) in [
            (SBLOCK_UFS2, FS_UFS2_MAGIC),
            (SBLOCK_UFS1, FS_UFS1_MAGIC),
            (SBLOCK_UFS1, FS_UFS2_MAGIC),
        ] {
            let pos = offset as u64 + MAGIC_OFFSET;
            if pos + 4 > source_size {
                continue;
            }
            file.seek(SeekFrom::Start(pos))?;
            let mut magic = [0u8; 4];
            file.read_exact(&mut magic)?;
            let config = if i32::from_le_bytes(magic) == expected_magic {
                Config::little()
            } else if i32::from_be_bytes(magic) == expected_magic {
                Config::big()
            } else {
                continue;
            };
            let mut file = Decoder::new(file, config);
            let mut superblock: Superblock = file.decode_at(offset as u64)?;
            if superblock.magic != expected_magic {
                iobail!(ErrorKind::InvalidData, "inconsistent superblock magic");
            }
            if offset != SBLOCK_UFS2 && file.inner().write_enabled() {
                iobail!(
                    ErrorKind::Unsupported,
                    "UFS alternate primary layout requires read-only access"
                );
            }
            if expected_magic == FS_UFS1_MAGIC {
                if superblock.old_inodefmt != 2 {
                    iobail!(
                        ErrorKind::Unsupported,
                        "UFS1 requires read-only 4.4BSD dinodes"
                    );
                }
                superblock.size = i64::from(superblock.old_size);
                superblock.dsize = i64::from(superblock.old_dsize);
                superblock.cstotal.nbfree = i64::from(superblock.old_cstotal.nbfree);
                superblock.cstotal.nffree = i64::from(superblock.old_cstotal.nffree);
                superblock.cstotal.nifree = i64::from(superblock.old_cstotal.nifree);
            }
            let mut ufs = Self {
                file,
                superblock,
                source_size,
            };
            ufs.check()?;
            return Ok(ufs);
        }
        iobail!(ErrorKind::InvalidInput, "no UFS1/UFS2 primary superblock");
    }

    pub fn is_ufs1(&self) -> bool {
        self.superblock.magic == FS_UFS1_MAGIC
    }

    fn pointer_size(&self) -> usize {
        if self.is_ufs1() {
            UFS1_POINTER_SIZE
        } else {
            size_of::<UfsDaddr>()
        }
    }

    pub fn write_enabled(&self) -> bool {
        self.file.inner().write_enabled()
    }

    fn assert_rw(&self) -> IoResult<()> {
        if self.write_enabled() {
            Ok(())
        } else {
            Err(err!(EROFS))
        }
    }

    /// Get filesystem metadata.
    #[doc(alias("statfs", "statvfs"))]
    pub fn info(&self) -> Info {
        let sb = &self.superblock;
        let cst = &sb.cstotal;
        Info {
            blocks: sb.dsize as u64,
            bfree: (cst.nbfree * sb.frag as i64 + cst.nffree) as u64,
            files: u64::from(sb.ipg) * u64::from(sb.ncg),
            ffree: cst.nifree as u64,
            bsize: sb.bsize as u32,
            fsize: sb.fsize as u32,
        }
    }

    fn check(&mut self) -> IoResult<()> {
        let sb = &self.superblock;
        log::debug!("Superblock: {sb:#?}");

        log::info!("Summary:");
        log::info!("Block Size: {}", sb.bsize);
        log::info!("# Blocks: {}", sb.size);
        log::info!("# Data Blocks: {}", sb.dsize);
        log::info!("Fragment Size: {}", sb.fsize);
        log::info!("Fragments per Block: {}", sb.frag);
        log::info!("# Cylinder Groups: {}", sb.ncg);

        macro_rules! sbassert {
            ($e:expr) => {
                if !($e) {
                    log::error!("superblock corrupted: {}", stringify!($e));
                    return Err(IoError::from_raw_os_error(libc::EIO));
                }
            };
        }

        sbassert!(sb.ncg > 0);
        sbassert!(sb.ipg > 0);
        sbassert!(sb.fpg > 0);
        sbassert!(sb.frag > 0 && sb.frag <= 8);
        sbassert!((512..=65536).contains(&sb.fsize) && (512..=65536).contains(&sb.bsize));
        sbassert!(sb.fsize == (sb.bsize / sb.frag));
        // TODO: this looks ugly:
        sbassert!(Some(sb.bsize) == 1i32.checked_shl(sb.bshift as u32));
        sbassert!(Some(sb.fsize) == 1i32.checked_shl(sb.fshift as u32));
        sbassert!(Some(sb.frag) == 1i32.checked_shl(sb.fragshift as u32));
        sbassert!((!sb.bmask).checked_add(1) == Some(sb.bsize));
        sbassert!((!sb.fmask).checked_add(1) == Some(sb.fsize));
        sbassert!(sb.size > 0 && sb.dsize >= 0 && sb.dsize <= sb.size);
        sbassert!(sb.size as u64 <= self.source_size / sb.fsize as u64);
        sbassert!(sb.ncg as u64 == (sb.size as u64).div_ceil(sb.fpg as u64));
        sbassert!(sb.sblkno >= 0 && sb.cblkno >= 0 && sb.iblkno >= 0);
        if self.is_ufs1() {
            sbassert!(sb.old_cgoffset >= 0);
            sbassert!((1376..=8192).contains(&sb.sbsize));
            sbassert!(sb.nindir == sb.bsize / UFS1_POINTER_SIZE as i32);
            sbassert!(sb.inopb == sb.bsize as u32 / UFS1_INODE_SIZE as u32);
        } else {
            sbassert!(sb.sbsize == sb.fsize);
            sbassert!(sb.cgsize_struct() < sb.bsize as usize);
        }

        // Validate each group's original on-disk placement before any inode access.
        for i in 0..sb.ncg {
            let fragment = self.group_start(u64::from(i))? + sb.sblkno as u64;
            let addr = self.fragment_offset(fragment, 1376)?;
            let csb: Superblock = self.file.decode_at(addr)?;
            if csb.magic != sb.magic {
                log::error!("CG{i} has invalid superblock magic: {:x}", csb.magic);
                return Err(err!(EIO));
            }
        }

        // check that all cylgroups are ok.
        for i in 0..self.superblock.ncg {
            let fragment = self.group_start(u64::from(i))? + sb.cblkno as u64;
            let addr = self.fragment_offset(fragment, 8)?;
            let magic: i32 = self.file.decode_at(addr + 4)?;
            if magic != CG_MAGIC {
                log::error!("CG{i} has invalid cg magic: {:x}", magic);
                return Err(err!(EIO));
            }
        }
        log::info!("OK");
        Ok(())
    }

    fn group_start(&self, group: u64) -> IoResult<u64> {
        let sb = &self.superblock;
        if group >= u64::from(sb.ncg) {
            iobail!(ErrorKind::InvalidData, "UFS cylinder group outside volume");
        }
        let base = group * sb.fpg as u64;
        let skew = if self.is_ufs1() {
            (group & u64::from(!sb.old_cgmask as u32)) * sb.old_cgoffset as u64
        } else {
            0
        };
        base.checked_add(skew).ok_or_else(|| err!(EIO))
    }

    fn fragment_offset(&self, fragment: u64, length: u64) -> IoResult<u64> {
        let size = self.superblock.fsize as u64;
        let limit = self.superblock.size as u64 * size;
        fragment
            .checked_mul(size)
            .filter(|offset| offset.checked_add(length).is_some_and(|end| end <= limit))
            .ok_or_else(|| {
                IoError::new(ErrorKind::InvalidData, "UFS allocation outside filesystem")
            })
    }

    fn cg_addr(&self, idx: u64) -> u64 {
        let sb = &self.superblock;
        let fpg = sb.fpg as u64;
        let cblkno = sb.cblkno as u64;
        let fs = sb.fsize as u64;

        (idx * fpg + cblkno) * fs
    }

    fn update_sb(&mut self, f: impl FnOnce(&mut Superblock)) -> IoResult<()> {
        // Only update the first superblock, because we're lazy.
        f(&mut self.superblock);
        self.file.encode_at(SBLOCK_UFS2 as u64, &self.superblock)?;
        Ok(())
    }
}

fn check_name_is_legal(name: &OsStr, allow_special: bool) -> IoResult<()> {
    let b = name.as_encoded_bytes();

    let x = b.contains(&b'/')
        || (name == "." && !allow_special)
        || (name == ".." && !allow_special)
        || b.contains(&b'\0');

    if x {
        Err(err!(EINVAL))
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn ufs1_native_makefs_oracle_when_available() {
    let Some(directory) = std::env::var_os("LOVELYCARVE_UFS1_ORACLE_DIR") else {
        return;
    };
    for endian in ["little", "big"] {
        let path = std::path::Path::new(&directory).join(format!("ufs1-{endian}.img"));
        let file = File::open(path).unwrap();
        let mut ufs = Ufs::new(BlockReader::new(file, 4096, false))
            .unwrap_or_else(|error| panic!("{endian} endian: {error}"));
        assert!(ufs.is_ufs1());
        let inode = ufs
            .dir_lookup(InodeNum::ROOT, OsStr::new("big.bin"))
            .unwrap();
        let alias = ufs
            .dir_lookup(InodeNum::ROOT, OsStr::new("alias.bin"))
            .unwrap();
        assert_eq!(inode, alias);
        let link = ufs.dir_lookup(InodeNum::ROOT, OsStr::new("link")).unwrap();
        assert_eq!(ufs.symlink_read(link).unwrap(), b"sub/readme.txt");
        let expected: Vec<u8> = (0..164000)
            .map(|i| ((i * 7 + i / 257) % 256) as u8)
            .collect();
        let mut actual = vec![0; expected.len()];
        assert_eq!(ufs.inode_read(inode, 0, &mut actual).unwrap(), actual.len());
        assert_eq!(actual, expected);
        let mut window = [0; 73];
        let offset = 12 * 4096 - 11;
        assert_eq!(
            ufs.inode_read(inode, offset as u64, &mut window).unwrap(),
            window.len()
        );
        assert_eq!(window, expected[offset..offset + window.len()]);
    }
}

#[cfg(test)]
#[test]
fn ufs2_native_makefs_oracle_when_available() {
    let Some(directory) = std::env::var_os("LOVELYCARVE_UFS1_ORACLE_DIR") else {
        return;
    };
    for endian in ["little", "big"] {
        let path = std::path::Path::new(&directory).join(format!("ufs2-{endian}.img"));
        if !path.exists() {
            continue;
        }
        let mut ufs = Ufs::new(BlockReader::new(File::open(path).unwrap(), 4096, false))
            .unwrap_or_else(|error| panic!("{endian} endian UFS2: {error}"));
        let inode = ufs
            .dir_lookup(InodeNum::ROOT, OsStr::new("readme.txt"))
            .unwrap();
        let mut bytes = [0u8; 25];
        assert_eq!(ufs.inode_read(inode, 0, &mut bytes).unwrap(), bytes.len());
        assert_eq!(&bytes, b"UFS2 compatibility check\n");
    }
}
