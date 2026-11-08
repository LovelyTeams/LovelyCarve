use std::ops::{Bound, RangeBounds};

use super::*;
use crate::{err, InodeNum};

impl<R: Backend> Ufs<R> {
    /// Get metadata about an inode.
    #[doc(alias("stat", "getattr"))]
    pub fn inode_attr(&mut self, inr: InodeNum) -> IoResult<InodeAttr> {
        log::trace!("inode_attr({inr});");
        let ino = self.read_inode(inr)?;
        Ok(ino.as_attr(inr))
    }

    /// Read data from an inode.
    pub fn inode_read(
        &mut self,
        inr: InodeNum,
        mut offset: u64,
        buffer: &mut [u8],
    ) -> IoResult<usize> {
        log::trace!("inode_read({inr}, {offset}, {})", buffer.len());
        let mut blockbuf = vec![0u8; self.superblock.bsize as usize];
        let ino = self.read_inode(inr)?;

        let mut boff = 0;
        let len = (buffer.len() as u64).min(ino.size - offset);
        let end = offset + len;

        while offset < end {
            let block = self.inode_find_block(inr, &ino, offset);
            let num = (block.size - block.off).min(end - offset);

            self.inode_read_block(
                inr,
                &ino,
                block.blkidx,
                &mut blockbuf[0..(block.size as usize)],
            )?;
            let off = block.off as usize;
            buffer[boff..(boff + num as usize)]
                .copy_from_slice(&blockbuf[off..(off + num as usize)]);

            offset += num;
            boff += num as usize;
        }

        Ok(boff)
    }

    pub fn inode_write(
        &mut self,
        inr: InodeNum,
        mut offset: u64,
        buffer: &[u8],
    ) -> IoResult<usize> {
        log::trace!("inode_write({inr}, {offset}, {})", buffer.len());
        self.assert_rw()?;

        let mut blockbuf = vec![0u8; self.superblock.bsize as usize];
        let mut ino = self.read_inode(inr)?;
        ino.size = ino.size.max(offset + buffer.len() as u64);
        self.write_inode(inr, &ino)?;

        let mut boff = 0;
        let len = (buffer.len() as u64).min(ino.size - offset);
        let end = offset + len;

        while offset < end {
            let block = self.inode_find_block(inr, &ino, offset);
            let num = (block.size - block.off).min(end - offset);

            // TODO: remove this read, if writing a full block
            self.inode_read_block(
                inr,
                &ino,
                block.blkidx,
                &mut blockbuf[0..(block.size as usize)],
            )?;

            let off = block.off as usize;
            blockbuf[off..(off + num as usize)]
                .copy_from_slice(&buffer[boff..(boff + num as usize)]);

            self.inode_write_block(
                inr,
                &mut ino,
                block.blkidx,
                &blockbuf[0..(block.size as usize)],
            )?;

            offset += num;
            boff += num as usize;
        }

        Ok(boff)
    }

    /// Copy data within a file.
    pub(super) fn inode_copy_range(
        &mut self,
        inr: InodeNum,
        ino: &Inode,
        from: impl RangeBounds<u64>,
        to: impl RangeBounds<u64>,
    ) -> IoResult<u64> {
        fn decode(ino: &Inode, b: impl RangeBounds<u64>) -> (u64, u64, u64) {
            let beg = match b.start_bound() {
                Bound::Unbounded => 0,
                Bound::Included(x) => *x,
                Bound::Excluded(_) => todo!(),
            };
            let end = match b.end_bound() {
                Bound::Unbounded => ino.size,
                Bound::Included(x) => *x - 1,
                Bound::Excluded(x) => *x,
            };
            assert!(beg <= end);
            (beg, end, end - beg)
        }

        self.assert_rw()?;

        let (fbeg, fend, flen) = decode(ino, from);
        let (tbeg, mut tend, mut tlen) = decode(ino, to);

        assert!(tlen >= flen);
        if tlen > flen {
            tend = tbeg + flen;
            tlen = flen;
        }
        assert_eq!(flen, tlen);

        let mut fpos = fbeg;
        let mut tpos = tbeg;
        let mut buf = [0u8; 512];

        while fpos < fend {
            assert!(tpos < tend);
            let n = (fend - fpos).min(buf.len() as u64);
            let nr = self.inode_read(inr, fpos, &mut buf)?;
            assert_eq!(n, nr as u64);

            let nw = self.inode_write(inr, tpos, &buf[..nr])?;
            assert_eq!(n, nw as u64);

            fpos += n;
            tpos += n;
        }

        Ok(flen)
    }

    pub(super) fn read_inode(&mut self, inr: InodeNum) -> IoResult<Inode> {
        log::trace!("read_inode({inr});");
        let off = self.superblock.ino_to_fso(inr);
        let ino: Inode = self.file.decode_at(off)?;
        let mode = ino.mode;

        if (mode & S_IFMT) == 0 {
            log::warn!("invalid inode {inr}: Mode {mode:#o}, S_IFMT is 0");
            return Err(err!(EINVAL));
        }

        Ok(ino)
    }

    pub(super) fn write_inode(&mut self, inr: InodeNum, ino: &Inode) -> IoResult<()> {
        log::trace!("write_inode({inr});");
        self.assert_rw()?;
        let off = self.superblock.ino_to_fso(inr);
        self.file.encode_at(off, &ino)?;
        Ok(())
    }

    pub fn inode_modify(
        &mut self,
        inr: InodeNum,
        f: impl FnOnce(InodeAttr) -> InodeAttr,
    ) -> IoResult<InodeAttr> {
        self.assert_rw()?;
        let mut ino = self.read_inode(inr)?;
        let attr = f(ino.as_attr(inr));

        ino.mode = (ino.mode & S_IFMT) | (attr.perm & !S_IFMT);
        ino.uid = attr.uid;
        ino.gid = attr.gid;
        ino.set_atime(attr.atime);
        ino.set_mtime(attr.mtime);
        ino.set_ctime(attr.ctime);
        ino.set_btime(attr.btime);
        ino.flags = attr.flags;

        self.write_inode(inr, &ino)?;
        Ok(ino.as_attr(inr))
    }

    pub(super) fn inode_read_block(
        &mut self,
        inr: InodeNum,
        ino: &Inode,
        blkidx: u64,
        buf: &mut [u8],
    ) -> IoResult<usize> {
        log::trace!("inode_read_block({inr}, {blkidx}, [u8; {}]);", buf.len());
        let fs = self.superblock.fsize as u64;
        let size = self.inode_get_block_size(ino, blkidx);
        match self.inode_resolve_block(inr, ino, blkidx)? {
            Some(blkno) => {
                self.file.read_at(blkno.get() * fs, &mut buf[0..size])?;
            }
            None => buf.fill(0u8),
        }

        Ok(size)
    }

    pub(super) fn inode_write_block(
        &mut self,
        inr: InodeNum,
        ino: &mut Inode,
        blkidx: u64,
        buf: &[u8],
    ) -> IoResult<()> {
        log::trace!("inode_write_block({inr}, {blkidx})");
        let fs = self.superblock.fsize as u64;
        let size = self.inode_get_block_size(ino, blkidx);

        let blkno = match self.inode_resolve_block(inr, ino, blkidx)? {
            Some(blkno) => blkno,
            None => self.inode_alloc_block(inr, ino, blkidx, size as u64)?.0,
        };

        self.file.write_at(blkno.get() * fs, &buf[0..size])?;
        Ok(())
    }

    pub(super) fn inode_find_block(
        &mut self,
        inr: InodeNum,
        ino: &Inode,
        offset: u64,
    ) -> BlockInfo {
        let bs = self.superblock.bsize as u64;
        let fs = self.superblock.fsize as u64;
        let (blocks, frags) = ino.size(bs, fs);
        log::trace!(
			"inode_find_block({inr}, {offset}): size={}, bs={bs}, blocks={blocks}, fs={fs}, frags={frags}",
			ino.size
		);

        let x = if offset < (bs * blocks) {
            BlockInfo {
                blkidx: offset / bs,
                off: offset % bs,
                size: bs,
            }
        } else if offset < (bs * blocks + fs * frags) {
            BlockInfo {
                blkidx: blocks,
                off: offset % bs,
                size: frags * fs,
            }
        } else {
            panic!("inode_find_block({inr}, {offset}): out of bounds");
        };
        log::trace!("inode_find_block({inr}, {offset}) = {x:?}");
        x
    }

    pub(super) fn inode_data_zones(&self) -> (u64, u64, u64, u64) {
        let nd = UFS_NDADDR as u64;
        let pbp = self.superblock.bsize as u64 / size_of::<u64>() as u64;

        (
            nd,
            nd + pbp,
            nd + pbp + (pbp * pbp),
            nd + pbp + (pbp * pbp) + (pbp * pbp * pbp),
        )
    }

    pub(super) fn decode_blkidx(&self, blkidx: u64) -> IoResult<InodeBlock> {
        let bs = self.superblock.bsize as u64;
        let pbp = bs / size_of::<u64>() as u64;
        let (begin_indir1, begin_indir2, begin_indir3, begin_indir4) = self.inode_data_zones();

        if blkidx < begin_indir1 {
            Ok(InodeBlock::Direct(blkidx as usize))
        } else if blkidx < begin_indir2 {
            let x = blkidx - begin_indir1;
            Ok(InodeBlock::Indirect1(x as usize))
        } else if blkidx < begin_indir3 {
            let x = blkidx - begin_indir2;
            let high = x / pbp;
            let low = x % pbp;
            Ok(InodeBlock::Indirect2(high as usize, low as usize))
        } else if blkidx < begin_indir4 {
            let x = blkidx - begin_indir3;
            let high = x / pbp / pbp;
            let mid = x / pbp % pbp;
            let low = x % pbp;
            Ok(InodeBlock::Indirect3(
                high as usize,
                mid as usize,
                low as usize,
            ))
        } else {
            Err(err!(EINVAL))
        }
    }

    fn inode_resolve_block(
        &mut self,
        inr: InodeNum,
        ino: &Inode,
        blkno: u64,
    ) -> IoResult<Option<NonZeroU64>> {
        let sb = &self.superblock;
        let bs = sb.bsize as u64;
        let su64 = size_of::<UfsDaddr>() as u64;
        let pbp = bs / su64;

        let InodeData::Blocks(InodeBlocks { direct, indirect }) = &ino.data else {
            log::warn!("inode_resolve_block({inr}, {blkno}): inode doesn't have blocks");
            return Err(err!(EIO));
        };

        let mut data = vec![0u64; pbp as usize];
        let bno = match self.decode_blkidx(blkno)? {
            InodeBlock::Direct(off) => NonZeroU64::new(direct[off] as u64),
            InodeBlock::Indirect1(off) => {
                let x1 = indirect[0] as u64;
                if x1 == 0 {
                    return Ok(None);
                }

                self.read_pblock(x1, &mut data)?;
                NonZeroU64::new(data[off])
            }
            InodeBlock::Indirect2(high, low) => {
                let x1 = indirect[1] as u64;
                if x1 == 0 {
                    return Ok(None);
                }

                self.read_pblock(x1, &mut data)?;
                let x2 = data[high];
                if x2 == 0 {
                    return Ok(None);
                }

                self.read_pblock(x2, &mut data)?;
                NonZeroU64::new(data[low])
            }
            InodeBlock::Indirect3(high, mid, low) => {
                let x1 = indirect[2] as u64;
                if x1 == 0 {
                    return Ok(None);
                }

                self.read_pblock(x1, &mut data)?;
                let x2 = data[high];
                if x2 == 0 {
                    return Ok(None);
                }

                self.read_pblock(x2, &mut data)?;
                let x3 = data[mid];
                if x3 == 0 {
                    return Ok(None);
                }

                self.read_pblock(x3, &mut data)?;
                NonZeroU64::new(data[low])
            }
        };

        log::trace!("inode_resolve_block({inr}, {blkno}): bno={bno:?}");

        Ok(bno)
    }

    pub(super) fn inode_get_block_size(&mut self, ino: &Inode, blkidx: u64) -> usize {
        let bs = self.superblock.bsize as u64;
        let fs = self.superblock.fsize as u64;
        let (blocks, frags) = ino.size(bs, fs);

        let res = if blkidx < blocks {
            bs as usize
        } else if blkidx < blocks + frags {
            (fs * frags) as usize
        } else {
            panic!("out of bounds: blkidx={blkidx}, blocks={blocks}, frags={frags}");
        };

        log::trace!(
            "inode_get_block_size(blkidx={blkidx}) = {res}; frags={frags}, blocks={blocks}"
        );
        res
    }
}
