use super::*;
use crate::InodeNum;

impl<R: Backend> Ufs<R> {
    fn iter_xattr<T>(
        &mut self,
        ino: &Inode,
        mut f: impl FnMut(&ExtattrHeader, &OsStr, &[u8]) -> Option<T>,
    ) -> IoResult<Option<T>> {
        if ino.extsize == 0 {
            return Ok(None);
        }

        let fs = self.superblock.fsize as u64;
        let bs = self.superblock.bsize as usize;
        let sz = ino.extsize as usize;
        assert!(sz < UFS_NXADDR * bs);

        let mut blocks = vec![0u8; ino.extsize as usize];
        let mut nr = 0;
        let mut blkidx = 0;

        while nr < blocks.len() {
            let pos = ino.extb[blkidx] as u64 * fs;
            let num = bs.min(blocks.len() - nr);
            self.file.read_at(pos, &mut blocks[nr..(nr + num)])?;
            blkidx += 1;
            nr += num;
        }

        let file = Cursor::new(blocks);
        let mut file = Decoder::new(file, self.file.config());
        let mut name = [0u8; 64];
        let mut data = Vec::new();

        loop {
            let begin = file.pos()?;
            let Ok(hdr) = file.decode::<ExtattrHeader>() else {
                break;
            };
            let namelen = hdr.namelen as usize;

            if namelen == 0 {
                break;
            } else if namelen > UFS_EXTATTR_MAXNAMELEN {
                log::error!("invalid extattr name length: {namelen}");
                break;
            }

            file.read(&mut name[0..namelen])?;
            file.align_to(8)?;
            let len = hdr.len as u64 - (file.pos()? - begin);
            data.resize(len as usize, 0u8);
            file.read(&mut data)?;
            data.resize(data.len() - hdr.contentpadlen as usize, 0u8);

            let name = OsStr::from_bytes(&name[0..namelen]);
            if let Some(x) = f(&hdr, name, &data) {
                return Ok(Some(x));
            }
        }

        Ok(None)
    }

    fn read_xattr<T>(
        &mut self,
        ino: &Inode,
        xname: &OsStr,
        mut f: impl FnMut(&ExtattrHeader, &[u8]) -> T,
    ) -> IoResult<T> {
        self.iter_xattr(ino, |hdr, n, data| {
            let ns = hdr.namespace()?;
            if xname == ns.with_name(n) {
                Some(f(hdr, data))
            } else {
                None
            }
        })
        .and_then(|r| r.ok_or(IoError::from_raw_os_error(crate::ENOATTR)))
    }

    /// Get the size of the extended attribute area of inode `inr`.
    pub fn xattr_list_len(&mut self, inr: InodeNum) -> IoResult<u32> {
        let ino = self.read_inode(inr)?;
        Ok(ino.extsize)
    }

    /// Get the list of extended attribyte names.
    /// Each entry follows the following format:
    /// `"namespace.name\0"`
    pub fn xattr_list(&mut self, inr: InodeNum) -> IoResult<Vec<u8>> {
        let ino = self.read_inode(inr)?;
        let mut data = OsString::new();
        self.iter_xattr(&ino, |hdr, name, _data| {
            let ns = hdr.namespace()?;
            let name = ns.with_name(name);
            data.push(name);
            data.push("\0");
            None::<()>
        })?;
        Ok(data.into_vec())
    }

    /// Get the size of an extended attribute.
    pub fn xattr_len(&mut self, inr: InodeNum, name: &OsStr) -> IoResult<u32> {
        let ino = self.read_inode(inr)?;
        let len = self.read_xattr(&ino, name, |_hdr, data| data.len())?;
        Ok(len as u32)
    }

    /// Read the value of an extended attribute.
    pub fn xattr_read(&mut self, inr: InodeNum, name: &OsStr) -> IoResult<Vec<u8>> {
        let ino = self.read_inode(inr)?;
        let data = self.read_xattr(&ino, name, |_hdr, data| data.into())?;
        Ok(data)
    }
}
