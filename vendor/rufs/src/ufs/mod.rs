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

mod balloc;
mod dir;
mod ialloc;
mod inode;
mod symlink;
mod xattr;

use crate::{
    blockreader::{Backend, BlockReader},
    data::*,
    decoder::{Config, Decoder},
};

/// (INTERNAL) Constructs an [`std::io::Error`] from an `errno`.
#[macro_export]
macro_rules! err {
    ($name:ident) => {
        IoError::from_raw_os_error(libc::$name)
    };
}

macro_rules! iobail {
	($kind:expr, $($tk:tt)+) => {
		return Err(IoError::new($kind, format!($($tk)+)))
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

/// Berkley Unix (Fast) Filesystem v2
pub struct Ufs<R: Backend> {
    file: Decoder<BlockReader<R>>,
    superblock: Superblock,
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
        let pos = SBLOCK_UFS2 as u64 + MAGIC_OFFSET;
        file.seek(SeekFrom::Start(pos))?;
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)?;

        // magic: 0x19 54 01 19
        let config = match magic {
            [0x19, 0x01, 0x54, 0x19] => Config::little(),
            [0x19, 0x54, 0x01, 0x19] => Config::big(),
            _ => {
                iobail!(
                    ErrorKind::InvalidInput,
                    "invalid superblock magic number: {magic:?}"
                )
            }
        };
        // FIXME: Choose based on hash of input or so, to excercise BE as well with introducing non-determinism

        let mut file = Decoder::new(file, config);

        let superblock: Superblock = file.decode_at(SBLOCK_UFS2 as u64)?;
        if superblock.magic != FS_UFS2_MAGIC {
            iobail!(
                ErrorKind::InvalidInput,
                "invalid superblock magic number: {}",
                superblock.magic
            );
        }
        let mut s = Self { file, superblock };
        s.check()?;
        Ok(s)
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
            files: (sb.ipg * sb.ncg) as u64,
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
        log::info!("CG Size: {}MiB", sb.cgsize() / 1024 / 1024);

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
        sbassert!(sb.fsize == (sb.bsize / sb.frag));
        // TODO: this looks ugly:
        sbassert!(Some(sb.bsize) == 1i32.checked_shl(sb.bshift as u32));
        sbassert!(Some(sb.fsize) == 1i32.checked_shl(sb.fshift as u32));
        sbassert!(Some(sb.frag) == 1i32.checked_shl(sb.fragshift as u32));
        sbassert!(sb.bsize == (!sb.bmask + 1));
        sbassert!(sb.fsize == (!sb.fmask + 1));
        sbassert!(sb.sbsize == sb.fsize);
        sbassert!(sb.cgsize_struct() < sb.bsize as usize);

        let fpg = sb.fpg as u64;
        let sblkno = sb.sblkno as u64;
        let fs = sb.fsize as u64;

        // check that all superblocks are ok.
        for i in 0..sb.ncg {
            let addr = (i as u64 * fpg + sblkno) * fs;
            let csb: Superblock = self.file.decode_at(addr).unwrap();
            if csb.magic != FS_UFS2_MAGIC {
                log::error!("CG{i} has invalid superblock magic: {:x}", csb.magic);
                return Err(err!(EIO));
            }
        }

        // check that all cylgroups are ok.
        for i in 0..self.superblock.ncg {
            let addr = self.cg_addr(i as u64);
            let cg: CylGroup = self.file.decode_at(addr).unwrap();
            if cg.magic != CG_MAGIC {
                log::error!("CG{i} has invalid cg magic: {:x}", cg.magic);
                return Err(err!(EIO));
            }
        }
        log::info!("OK");
        Ok(())
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
