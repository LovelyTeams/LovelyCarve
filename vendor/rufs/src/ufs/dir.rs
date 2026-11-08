use std::io::{BufRead, Write};

use super::*;
use crate::{err, InodeNum};

#[derive(Debug, Clone, Copy)]
struct Header {
    inr: InodeNum,
    reclen: u16,
    kind: Option<InodeType>,
    namelen: u8,
    name: [u8; UFS_MAXNAMELEN + 1],
}

impl Header {
    fn new(inr: InodeNum, kind: InodeType, dname: &OsStr) -> Self {
        assert!(dname.len() <= UFS_MAXNAMELEN);
        let mut name = [0u8; UFS_MAXNAMELEN + 1];
        name[0..dname.len()].copy_from_slice(dname.as_bytes());
        let reclen = ((4 + 2 + 1 + 1 + name.len() + 3) & !3) as u16;
        Self {
            inr,
            reclen,
            kind: Some(kind),
            name,
            namelen: dname.len() as u8,
        }
    }

    fn parse<T: BufRead + Seek>(file: &mut Decoder<T>) -> IoResult<Option<Header>> {
        let inr: InodeNum = file.decode()?;
        let reclen: u16 = file.decode()?;
        if reclen == 0 {
            return Ok(None);
        }
        let kind: u8 = file.decode()?;
        let namelen: u8 = file.decode()?;
        let mut name = [0u8; UFS_MAXNAMELEN + 1];
        file.read(&mut name[0..namelen.into()])?;

        // skip remaining bytes of record, if any
        let off = reclen - (namelen as u16) - 8;
        file.seek_relative(off as i64)?;

        if inr.get() == 0 {
            return Ok(None);
        }

        log::trace!("Header::read(): {{ inr={inr}, reclen={reclen}, namelen={namelen}, name={:?}, kind={kind} }}",
					unsafe { OsStr::from_encoded_bytes_unchecked(&name[0..namelen.into()]) });

        let kind = match kind {
            DT_FIFO => Some(InodeType::NamedPipe),
            DT_CHR => Some(InodeType::CharDevice),
            DT_DIR => Some(InodeType::Directory),
            DT_BLK => Some(InodeType::BlockDevice),
            DT_REG => Some(InodeType::RegularFile),
            DT_LNK => Some(InodeType::Symlink),
            DT_SOCK => Some(InodeType::Socket),
            DT_WHT => None,
            DT_UNKNOWN => todo!("DT_UNKNOWN: {inr}"),
            _ => panic!("invalid filetype: {kind}"),
        };

        Ok(Some(Self {
            inr,
            reclen,
            kind,
            namelen,
            name,
        }))
    }

    fn write<T: Read + Write + Seek>(&self, file: &mut Decoder<T>) -> IoResult<()> {
        let kind: u8 = match self.kind {
            Some(InodeType::NamedPipe) => DT_FIFO,
            Some(InodeType::CharDevice) => DT_CHR,
            Some(InodeType::Directory) => DT_DIR,
            Some(InodeType::BlockDevice) => DT_BLK,
            Some(InodeType::RegularFile) => DT_REG,
            Some(InodeType::Symlink) => DT_LNK,
            Some(InodeType::Socket) => DT_SOCK,
            None => DT_WHT,
        };
        log::trace!(
            "Header::write(inr={}, reclen={}, namelen={}, name={:?}, kind={kind})",
            self.inr,
            self.reclen,
            self.namelen,
            self.name()
        );
        file.encode(&self.inr)?;
        file.encode(&self.reclen)?;
        file.encode(&kind)?;
        file.encode(&self.namelen)?;
        file.write(&self.name[0..self.namelen.into()])?;

        // fill the rest of the record with zeros
        file.fill(0u8, (self.reclen - (self.namelen as u16) - 8).into())?;

        Ok(())
    }

    fn name(&self) -> &OsStr {
        unsafe { OsStr::from_encoded_bytes_unchecked(&self.name[0..self.namelen.into()]) }
    }

    fn minlen(&self) -> u16 {
        (4 + 2 + 1 + 1 + (self.namelen as u16) + 3) & !3
    }
}

fn readdir_block<T>(
    inr: InodeNum,
    block: &[u8],
    config: Config,
    mut f: impl FnMut(&OsStr, InodeNum, InodeType) -> Option<T>,
) -> IoResult<Option<T>> {
    let mut file = Decoder::new(Cursor::new(block), config);

    loop {
        let Ok(Some(hdr)) = Header::parse(&mut file) else {
            break;
        };

        if hdr.inr.get() == 0 {
            break;
        }

        let Some(kind) = hdr.kind else {
            log::warn!(
                "readdir_block({inr}): encountered a whiteout entry: {:?}",
                hdr.name()
            );
            continue;
        };

        let res = f(hdr.name(), hdr.inr, kind);
        if res.is_some() {
            return Ok(res);
        }
    }

    Ok(None)
}

fn newlink_block(block: &mut [u8], mut entry: Header, config: Config) -> IoResult<bool> {
    let mut file = Decoder::new(Cursor::new(block), config);

    loop {
        let pos = file.pos()?;
        let Ok(Some(mut hdr)) = Header::parse(&mut file) else {
            break;
        };
        let minlen = hdr.minlen();
        let rem = hdr.reclen - minlen;
        if rem < entry.minlen() {
            continue;
        }

        hdr.reclen = minlen;
        entry.reclen = rem;

        file.seek(pos)?;
        hdr.write(&mut file)?;
        entry.write(&mut file)?;
        return Ok(true);
    }

    Ok(false)
}

fn unlink_block(
    dinr: InodeNum,
    block: &mut [u8],
    name: &OsStr,
    config: Config,
) -> IoResult<Option<(InodeNum, bool)>> {
    let mut file = Decoder::new(Cursor::new(block), config);
    let mut prevpos = 0;

    loop {
        let pos = file.pos()?;
        let Ok(Some(hdr)) = Header::parse(&mut file) else {
            break;
        };

        if hdr.name() != name {
            prevpos = pos;
            continue;
        }

        log::trace!("unlink_block({dinr}, {name:?}): pos={pos}, hdr={hdr:?}");

        let has;
        if pos == 0 {
            match Header::parse(&mut file) {
                Ok(Some(next)) => {
                    log::trace!("unlink_block({dinr}, {name:?}): next={next:?}");
                    let new = Header {
                        reclen: hdr.reclen + next.reclen,
                        ..next
                    };
                    file.seek(pos)?;
                    new.write(&mut file)?;
                    has = true;
                }
                _ => {
                    log::trace!("unlink_block({dinr}, {name:?}): no next");
                    has = false;
                }
            }
        } else {
            file.seek(prevpos)?;
            match Header::parse(&mut file)? {
                Some(mut prev) => {
                    prev.reclen += hdr.reclen;
                    file.seek(prevpos)?;
                    prev.write(&mut file)?;
                    has = true;
                }
                None => {
                    log::error!(
                        "unlink_block({dinr}): previous entry is bad: prevpos={prevpos}, pos={pos}"
                    );
                    return Err(err!(EIO));
                }
            }
        }
        return Ok(Some((hdr.inr, has)));
    }

    Ok(None)
}

fn newdir(dinr: InodeNum, inr: InodeNum, config: Config) -> IoResult<[u8; DIRBLKSIZE]> {
    let mut block = [0u8; DIRBLKSIZE];
    let mut file = Decoder::new(Cursor::new(&mut block as &mut [u8]), config);

    let h_self = Header::new(inr, InodeType::Directory, OsStr::new("."));
    let mut h_parent = Header::new(dinr, InodeType::Directory, OsStr::new(".."));
    h_parent.reclen = (DIRBLKSIZE as u16) - h_self.reclen;
    h_self.write(&mut file)?;
    h_parent.write(&mut file)?;

    Ok(block)
}

impl<R: Backend> Ufs<R> {
    /// Find a file named `name` in the directory referenced by `pinr`.
    pub fn dir_lookup(&mut self, pinr: InodeNum, name: &OsStr) -> IoResult<InodeNum> {
        log::trace!("dir_lookup({pinr}, {name:?});");
        self.dir_iter(
            pinr,
            |name2, inr, _kind| {
                if name == name2 {
                    Some(inr)
                } else {
                    None
                }
            },
        )?
        .ok_or(err!(ENOENT))
    }

    /// Iterate through a directory referenced by `inr`, and call `f` for each entry.
    pub fn dir_iter<T>(
        &mut self,
        inr: InodeNum,
        mut f: impl FnMut(&OsStr, InodeNum, InodeType) -> Option<T>,
    ) -> IoResult<Option<T>> {
        let ino = self.read_inode(inr)?;
        ino.assert_dir()?;
        let mut block = [0u8; DIRBLKSIZE];
        let mut pos = 0;
        while pos < ino.size {
            let n = self.inode_read(inr, pos, &mut block)?;
            assert_eq!(n, DIRBLKSIZE);
            if let Some(x) = readdir_block(inr, &block, self.file.config(), &mut f)? {
                return Ok(Some(x));
            }

            pos += DIRBLKSIZE as u64;
        }
        Ok(None)
    }

    pub(super) fn dir_unlink(&mut self, dinr: InodeNum, name: &OsStr) -> IoResult<InodeNum> {
        log::trace!("dir_unlink({dinr}, {name:?});");
        self.assert_rw()?;
        let dino = self.read_inode(dinr)?;
        dino.assert_dir()?;

        let mut block = vec![0u8; DIRBLKSIZE];
        let mut pos = 0;
        while pos < dino.size {
            let n = self.inode_read(dinr, pos, &mut block)?;
            assert_eq!(n, DIRBLKSIZE);

            if let Some((inr, has)) = unlink_block(dinr, &mut block, name, self.file.config())? {
                if has {
                    self.inode_write(dinr, pos, &block)?;
                } else {
                    let n =
                        self.inode_copy_range(dinr, &dino, (pos + DIRBLKSIZE as u64).., pos..)?;
                    assert_eq!(n, dino.size - pos - DIRBLKSIZE as u64);
                    self.inode_truncate(dinr, dino.size - DIRBLKSIZE as u64)?;
                }
                return Ok(inr);
            }

            pos += DIRBLKSIZE as u64;
        }

        Err(err!(ENOENT))
    }

    pub(super) fn dir_newlink(
        &mut self,
        dinr: InodeNum,
        inr: InodeNum,
        name: &OsStr,
        kind: InodeType,
    ) -> IoResult<()> {
        log::trace!("dir_newlink({dinr}, {inr}, {name:?}, {kind:?});");
        self.assert_rw()?;
        let dino = self.read_inode(dinr)?;
        dino.assert_dir()?;

        let mut entry = Header::new(inr, kind, name);

        let mut block = [0u8; DIRBLKSIZE];
        let mut pos = 0;
        while pos < dino.size {
            let n = self.inode_read(dinr, pos, &mut block)?;
            assert_eq!(n, DIRBLKSIZE);

            if newlink_block(&mut block, entry, self.file.config())? {
                self.inode_write(dinr, pos, &block)?;
                return Ok(());
            }

            pos += DIRBLKSIZE as u64;
        }

        log::trace!("dir_link({dinr}, {inr}, {name:?}, {kind:?}): extending directory for new entry: {entry:?}");
        self.inode_truncate(dinr, dino.size + DIRBLKSIZE as u64)?;
        entry.reclen = DIRBLKSIZE as u16;
        entry.write(&mut Decoder::new(
            Cursor::new(&mut block as &mut [u8]),
            self.file.config(),
        ))?;
        self.inode_write(dinr, pos, &block)?;
        Ok(())
    }

    pub fn unlink(&mut self, dinr: InodeNum, name: &OsStr) -> IoResult<()> {
        log::trace!("unlink({dinr}, {name:?});");
        self.assert_rw()?;
        let inr = self.dir_unlink(dinr, name)?;
        self.inode_free(inr)?;
        Ok(())
    }

    pub fn rename(
        &mut self,
        d_dinr: InodeNum,
        d_name: &OsStr,
        s_dinr: InodeNum,
        s_name: &OsStr,
        replace: bool,
    ) -> IoResult<InodeNum> {
        log::trace!("rename({d_dinr}, {d_name:?} {s_dinr}, {s_name:?}, {replace});");
        self.assert_rw()?;

        let inr = self.dir_lookup(s_dinr, s_name)?;

        if !replace {
            // User has requested to error if the destination already exists
            match self.dir_lookup(s_dinr, s_name) {
                Ok(_) => return Err(err!(EEXIST)),
                // TODO: Need raw OS error here?
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                // TODO: Might want to handle not a directory etc specially.
                Err(e) => return Err(e),
            }
        } else {
            // Not sure whether the kernel ensures for us that the file no longer exists,
            // if this triggers we know we guessed wrong.
            match self.unlink(d_dinr, d_name) {
				    Ok(_) => log::warn!("rename({d_dinr}, {d_name:?} {s_dinr}, {s_name:?}, {replace}): Destination already exists, unlinked"),
					// TODO: Need raw OS error here?
					Err(e) if e.kind() == ErrorKind::NotFound => { },
					// TODO: Handle not a directory etc.
					Err(e) => return Err(e),
				}
        }

        let kind = self.inode_attr(inr)?.kind;
        // unlink decrements the refcount, if it reaches 0 the file may get removed.
        // Bump the counter before to workaround.
        self.inode_bump(inr)?;
        self.dir_newlink(d_dinr, inr, d_name, kind)?;

        self.unlink(s_dinr, s_name)?;
        Ok(inr)
    }

    pub fn rmdir(&mut self, dinr: InodeNum, name: &OsStr) -> IoResult<()> {
        self.assert_rw()?;
        let inr = self.dir_lookup(dinr, name)?;
        let x = self.dir_iter(inr, |name, _inr, kind| {
            if kind != InodeType::Directory || (name != "." && name != "..") {
                Some(name.to_os_string())
            } else {
                None
            }
        })?;

        if x.is_some() {
            log::debug!("rmdir({dinr}, {name:?}): x = {x:?}");
            return Err(err!(ENOTEMPTY));
        }

        assert_eq!(inr, self.dir_unlink(dinr, name)?);

        self.unlink(inr, OsStr::new(".."))?;
        self.unlink(inr, OsStr::new("."))?;
        self.inode_free(inr)?;
        Ok(())
    }

    pub fn mknod(
        &mut self,
        dinr: InodeNum,
        name: &OsStr,
        kind: InodeType,
        perm: u16,
        uid: u32,
        gid: u32,
    ) -> IoResult<InodeAttr> {
        self.assert_rw()?;
        check_name_is_legal(name, false)?;
        let mut ino = Inode::new(kind, perm, uid, gid, self.superblock.bsize as u32);
        let inr = self.inode_alloc(&mut ino)?;
        self.dir_newlink(dinr, inr, name, kind)?;
        Ok(ino.as_attr(inr))
    }

    pub fn mkdir(
        &mut self,
        dinr: InodeNum,
        name: &OsStr,
        perm: u16,
        uid: u32,
        gid: u32,
    ) -> IoResult<InodeAttr> {
        let inr = self
            .mknod(dinr, name, InodeType::Directory, perm, uid, gid)?
            .inr;

        let mut dino = self.read_inode(dinr)?;
        dino.nlink += 1;
        self.write_inode(dinr, &dino)?;

        // update nlink
        let mut ino = self.read_inode(inr)?;
        ino.nlink = 2;
        self.write_inode(inr, &ino)?;

        let block = newdir(dinr, inr, self.file.config())?;
        self.inode_truncate(inr, block.len() as u64)?;
        self.inode_write(inr, 0, &block)?;

        let ino = self.read_inode(inr)?;
        Ok(ino.as_attr(inr))
    }
}
