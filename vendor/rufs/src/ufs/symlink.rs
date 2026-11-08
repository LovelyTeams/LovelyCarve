use super::*;
use crate::{err, InodeNum};

impl<R: Backend> Ufs<R> {
    /// Read the contents of a symbolic link.
    #[doc(alias = "readlink")]
    pub fn symlink_read(&mut self, inr: InodeNum) -> IoResult<Vec<u8>> {
        let ino = self.read_inode(inr)?;

        if ino.kind() != InodeType::Symlink {
            return Err(err!(EINVAL));
        }

        match &ino.data {
            InodeData::Shortlink(link) => {
                assert_eq!(ino.blocks, 0);
                let len = ino.size as usize;
                Ok(link[0..len].to_vec())
            }
            InodeData::Blocks { .. } => {
                // TODO: this has to be tested for other configurations, such as 4K/4K
                assert!(ino.blocks <= 8);

                let len = ino.size as usize;
                let mut buf = vec![0u8; self.superblock.bsize as usize];
                self.inode_read_block(inr, &ino, 0, &mut buf)?;
                buf.resize(len, 0u8);
                Ok(buf)
            }
        }
    }

    fn symlink_set(&mut self, inr: InodeNum, link: &OsStr) -> IoResult<()> {
        let mut ino = self.read_inode(inr)?;
        if ino.kind() != InodeType::Symlink {
            return Err(err!(EINVAL));
        }

        assert_eq!(ino.blocks, 0);

        let len = link.len();
        if len < UFS_SLLEN {
            let mut data = [0u8; UFS_SLLEN];
            data[0..len].copy_from_slice(link.as_bytes());
            ino.data = InodeData::Shortlink(data);
        } else {
            todo!("creating long symlinks");
        }
        ino.size = len as u64;
        self.write_inode(inr, &ino)?;
        Ok(())
    }

    pub fn symlink(
        &mut self,
        dinr: InodeNum,
        name: &OsStr,
        link: &OsStr,
        uid: u32,
        gid: u32,
    ) -> IoResult<InodeAttr> {
        self.assert_rw()?;
        let attr = self.mknod(dinr, name, InodeType::Symlink, 0o777, uid, gid)?;
        self.symlink_set(attr.inr, link)?;
        Ok(attr)
    }
}
