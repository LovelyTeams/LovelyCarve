use std::{
    io::Error,
    time::{Duration, SystemTime},
};

use bincode_next::{
    de::Decoder,
    enc::Encoder,
    error::{DecodeError, EncodeError},
    Decode, Encode,
};

use crate::data::*;

fn timetosys(mut s: UfsTime, ns: u32) -> SystemTime {
    let neg = s < 0;
    if neg {
        s = -s;
    }
    let dur = Duration::new(s as u64, ns);
    let mut time = SystemTime::UNIX_EPOCH;
    if neg {
        time -= dur;
    } else {
        time += dur;
    }
    time
}

fn systotime(t: SystemTime) -> (UfsTime, u32) {
    let (diff, neg) = if t >= SystemTime::UNIX_EPOCH {
        (t.duration_since(SystemTime::UNIX_EPOCH).unwrap(), 1)
    } else {
        (SystemTime::UNIX_EPOCH.duration_since(t).unwrap(), -1)
    };

    (neg * diff.as_secs() as UfsTime, diff.subsec_nanos())
}

impl Inode {
    pub fn new(kind: InodeType, perm: u16, uid: u32, gid: u32, blksize: u32) -> Self {
        let (now, nowsnsec) = systotime(SystemTime::now());
        let data = match kind {
            InodeType::Symlink => InodeData::Shortlink([0u8; UFS_SLLEN]),
            _ => InodeData::Blocks(InodeBlocks::default()),
        };
        let kind = match kind {
            InodeType::RegularFile => S_IFREG,
            InodeType::Directory => S_IFDIR,
            InodeType::Symlink => S_IFLNK,
            InodeType::CharDevice => S_IFCHR,
            InodeType::BlockDevice => S_IFBLK,
            InodeType::Socket => S_IFSOCK,
            InodeType::NamedPipe => S_IFIFO,
        };
        let mode = kind | (perm & !S_IFMT);
        Self {
            mode,
            nlink: 0,
            uid,
            gid,
            blksize,
            size: 0,
            blocks: 0,
            atime: now,
            mtime: now,
            ctime: now,
            birthtime: now,
            atimensec: nowsnsec,
            mtimensec: nowsnsec,
            ctimensec: nowsnsec,
            birthnsec: nowsnsec,
            r#gen: 0,
            kernflags: 0,
            flags: 0,
            extsize: 0,
            extb: [0; UFS_NXADDR],
            data,
            modrev: 0,
            ignored: 0,
            ckhash: 0,
            spare: [0; 2],
        }
    }

    pub fn atime(&self) -> SystemTime {
        timetosys(self.atime, self.atimensec)
    }

    pub fn mtime(&self) -> SystemTime {
        timetosys(self.mtime, self.mtimensec)
    }

    pub fn ctime(&self) -> SystemTime {
        timetosys(self.ctime, self.ctimensec)
    }

    pub fn btime(&self) -> SystemTime {
        timetosys(self.birthtime, self.birthnsec)
    }

    pub fn set_atime(&mut self, t: SystemTime) {
        (self.atime, self.atimensec) = systotime(t);
    }

    pub fn set_mtime(&mut self, t: SystemTime) {
        (self.mtime, self.mtimensec) = systotime(t);
    }

    pub fn set_ctime(&mut self, t: SystemTime) {
        (self.ctime, self.ctimensec) = systotime(t);
    }

    pub fn set_btime(&mut self, t: SystemTime) {
        (self.birthtime, self.birthnsec) = systotime(t);
    }

    pub fn assert_dir(&self) -> Result<(), Error> {
        if self.kind() == InodeType::Directory {
            Ok(())
        } else {
            Err(Error::from_raw_os_error(libc::ENOTDIR))
        }
    }

    pub fn kind(&self) -> InodeType {
        let mode = self.mode & S_IFMT;
        match mode {
            S_IFIFO => InodeType::NamedPipe,
            S_IFCHR => InodeType::CharDevice,
            S_IFDIR => InodeType::Directory,
            S_IFBLK => InodeType::BlockDevice,
            S_IFREG => InodeType::RegularFile,
            S_IFLNK => InodeType::Symlink,
            S_IFSOCK => InodeType::Socket,
            _ => unreachable!("invalid file mode: {mode:o}"),
        }
    }

    pub fn as_attr(&self, inr: InodeNum) -> InodeAttr {
        InodeAttr {
            inr,
            perm: self.mode & 0o7777,
            kind: self.kind(),
            size: self.size,
            blocks: self.blocks,
            atime: self.atime(),
            mtime: self.mtime(),
            ctime: self.ctime(),
            btime: self.btime(),
            nlink: self.nlink,
            uid: self.uid,
            gid: self.gid,
            r#gen: self.r#gen,
            blksize: self.blksize,
            flags: self.flags,
            kernflags: self.kernflags,
            extsize: self.extsize,
        }
    }

    pub fn size(&self, bs: u64, fs: u64) -> (u64, u64) {
        let size = match self.kind() {
            InodeType::RegularFile | InodeType::Symlink | InodeType::Directory => self.size,
            kind => todo!("Inode::size() is undefined for {kind:?}"),
        };
        Self::inode_size(bs, fs, size)
    }

    /// The number of blocks and fragments this inode needs.
    pub fn inode_size(bs: u64, fs: u64, size: u64) -> (u64, u64) {
        let blocks = size / bs;
        let frags = (size % bs).div_ceil(fs);

        (blocks, frags)
    }
}

impl<Context> Decode<Context> for Inode {
    fn decode<D: Decoder<Context = Context>>(d: &mut D) -> Result<Self, DecodeError> {
        let mode = u16::decode(d)?;
        let nlink = u16::decode(d)?;
        let uid = u32::decode(d)?;
        let gid = u32::decode(d)?;
        let blksize = u32::decode(d)?;
        let size = u64::decode(d)?;
        let blocks = u64::decode(d)?;
        let atime = UfsTime::decode(d)?;
        let mtime = UfsTime::decode(d)?;
        let ctime = UfsTime::decode(d)?;
        let birthtime = UfsTime::decode(d)?;
        let mtimensec = u32::decode(d)?;
        let atimensec = u32::decode(d)?;
        let ctimensec = u32::decode(d)?;
        let birthnsec = u32::decode(d)?;
        let r#gen = u32::decode(d)?;
        let kernflags = u32::decode(d)?;
        let flags = u32::decode(d)?;
        let extsize = u32::decode(d)?;
        let extb = <[UfsDaddr; UFS_NXADDR]>::decode(d)?;
        let data = if (mode & S_IFMT) == S_IFLNK && blocks == 0 {
            InodeData::Shortlink(Decode::decode(d)?)
        } else {
            InodeData::Blocks(InodeBlocks::decode(d)?)
        };

        let ino = Self {
            mode,
            nlink,
            uid,
            gid,
            blksize,
            size,
            blocks,
            atime,
            mtime,
            ctime,
            birthtime,
            mtimensec,
            atimensec,
            ctimensec,
            birthnsec,
            r#gen,
            kernflags,
            flags,
            extsize,
            extb,
            data,
            modrev: u64::decode(d)?,
            ignored: u32::decode(d)?,
            ckhash: u32::decode(d)?,
            spare: <[u32; 2]>::decode(d)?,
        };

        Ok(ino)
    }
}

impl Encode for InodeData {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        match self {
            Self::Blocks(blocks) => InodeBlocks::encode(blocks, encoder),
            Self::Shortlink(link) => <[u8; UFS_SLLEN]>::encode(link, encoder),
        }
    }
}

mod test {
    #[test]
    fn inode_size() {
        let bs = 32768;
        let fs = 4096;

        let isz = |sz| super::Inode::inode_size(bs, fs, sz);

        assert_eq!(isz(0), (0, 0));
        assert_eq!(isz(1), (0, 1));
        assert_eq!(isz(fs), (0, 1));
        assert_eq!(isz(bs), (1, 0));
        assert_eq!(isz(bs + 2 * fs), (1, 2));
        assert_eq!(isz(100 * bs + 7 * fs), (100, 7));
    }
}

// Lovelycarve patch: drop the upstream FUSE bridge modules. These would
// require the `fuser` / `fuse2rs` dependencies, which we do not vendor — we
// consume rufs purely as a read-only block parser and never expose it through
// FUSE. Re-add by restoring the upstream `mod f` / `mod f2` blocks behind
// `feature = "fuser"` / `feature = "fuse2rs"` if/when a contributor needs the
// bridge.
