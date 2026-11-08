use std::mem::replace;

use super::*;
use crate::{err, InodeNum};

const STAT_BLKSIZE: u64 = 512;

impl<R: Backend> Ufs<R> {
    fn inode_setup(&mut self, inr: InodeNum, ino: &mut Inode) -> IoResult<()> {
        log::trace!("inode_setup({inr});");
        let inp = self.superblock.ino_to_fso(inr);
        let old_nlink: u16 = self.file.decode_at(inp + 2)?;
        let old_gen: u32 = self.file.decode_at(inp + 80)?;

        if old_nlink != 0 {
            log::error!("inode_setup({inr}): use after free");
            if let Ok(ino) = self.read_inode(inr) {
                log::error!("inode_setup({inr}): ino={ino:#?}");
            }
            return Err(err!(EFAULT));
        }
        assert_eq!(ino.nlink, 0);

        ino.r#gen = old_gen + 1;
        ino.nlink = 1;
        self.write_inode(inr, ino)?;
        self.file.seek(0)?;
        let _ = self.read_inode(inr)?;
        Ok(())
    }

    pub(super) fn inode_alloc(&mut self, ino: &mut Inode) -> IoResult<InodeNum> {
        self.assert_rw()?;
        let sb = &self.superblock;
        let ipg = sb.ipg as u64;
        assert_eq!(ipg % 8, 0);

        for cgi in 0..sb.ncg {
            let cga = self.cg_addr(cgi.into());
            let mut cg: CylGroup = self.file.decode_at(cga)?;

            if cg.cs.nifree <= 0 {
                continue;
            }

            let off = cga + cg.iusedoff as u64;

            for i in 0..(ipg / 8) {
                let addr = i + off;
                let mut b: u8 = self.file.decode_at(addr)?;
                log::debug!("addr={addr:#x} b = {b:#x}");
                if b == 0xff {
                    continue;
                }

                let j = (0..8)
                    .enumerate()
                    .find(|(_, idx)| (b & (1 << idx)) == 0)
                    .unwrap()
                    .1;

                b |= 1 << j;
                self.file.encode_at(addr, &b)?;

                let inr = cgi as u64 * ipg + i * 8 + j as u64;
                let inr = unsafe { InodeNum::new(inr as u32) };

                log::trace!("inode_alloc(): {inr}");
                self.inode_setup(inr, ino)?;

                // update free count in CG
                cg.cs.nifree -= 1;
                self.file.encode_at(cga, &cg)?;

                self.update_sb(|sb| sb.cstotal.nifree -= 1)?;

                return Ok(inr);
            }
        }

        Err(err!(ENOSPC))
    }

    pub(super) fn read_pblock(&mut self, bno: u64, block: &mut [u64]) -> IoResult<()> {
        let fs = self.superblock.fsize as u64;
        let bs = self.superblock.bsize as usize;
        let pbp = bs / size_of::<u64>();

        assert_eq!(block.len(), pbp);

        self.file.seek(bno * fs)?;
        for i in block.iter_mut() {
            *i = self.file.decode()?;
        }
        Ok(())
    }

    pub(super) fn write_pblock(&mut self, bno: u64, block: &[u64]) -> IoResult<()> {
        let fs = self.superblock.fsize as u64;
        let bs = self.superblock.bsize as usize;
        let pbp = bs / size_of::<u64>();

        assert_eq!(block.len(), pbp);

        self.file.seek(bno * fs)?;
        for i in block.iter() {
            self.file.encode(i)?;
        }
        Ok(())
    }

    fn inode_free_l1(&mut self, ino: &mut Inode, bno: u64, block: &mut [u64]) -> IoResult<()> {
        if bno == 0 {
            return Ok(());
        }

        self.read_pblock(bno, block)?;

        for (idx, bno) in block.iter().enumerate() {
            if *bno == 0 {
                continue;
            }
            let size = self.inode_get_block_size(ino, idx as u64);
            self.inode_free_block(ino, *bno, size as u64)?;
        }

        self.blk_free(bno, self.superblock.bsize as u64)?;

        Ok(())
    }

    fn inode_free_l2(&mut self, ino: &mut Inode, bno: u64, block: &mut [u64]) -> IoResult<()> {
        if bno == 0 {
            return Ok(());
        }

        self.read_pblock(bno, block)?;
        let indir = block.to_owned();

        for bno in indir {
            self.inode_free_l1(ino, bno, block)?;
        }

        self.blk_free(bno, self.superblock.bsize as u64)?;

        Ok(())
    }

    fn inode_free_l3(&mut self, ino: &mut Inode, bno: u64, block: &mut [u64]) -> IoResult<()> {
        if bno == 0 {
            return Ok(());
        }

        self.read_pblock(bno, block)?;
        let indir = block.to_owned();

        for bno in indir {
            self.inode_free_l2(ino, bno, block)?;
        }

        self.blk_free(bno, self.superblock.bsize as u64)?;

        Ok(())
    }

    /// Increment the reference count of the Inode `inr`.
    pub(super) fn inode_bump(&mut self, inr: InodeNum) -> IoResult<()> {
        self.assert_rw()?;
        let mut ino = self.read_inode(inr)?;
        ino.nlink += 1;
        self.write_inode(inr, &ino)?;
        Ok(())
    }

    /// Decrement the reference count of the Inode `inr`
    /// and delete the inode, if it hits 0.
    pub(super) fn inode_free(&mut self, inr: InodeNum) -> IoResult<()> {
        self.assert_rw()?;
        let mut ino = self.read_inode(inr)?;
        ino.nlink -= 1;
        self.write_inode(inr, &ino)?;

        if ino.nlink > 0 {
            return Ok(());
        }

        let sb = &self.superblock;

        // clear the inode
        let off = self.superblock.ino_to_fso(inr);
        self.file.fill_at(off, 0u8, UFS_INOSZ)?;

        // calculate the cylinder group number and offset for the inode.
        let (cgi, cgo) = self.superblock.ino_in_cg(inr);
        let cga = self.cg_addr(cgi);
        let mut cg: CylGroup = self.file.decode_at(cga)?;

        if cg.magic != CG_MAGIC {
            panic!("inode_free({inr}): invalid cylinder group: cgi={cgi}, cga={cga:08x}");
        }

        // free the inode in the inode bitmap
        let off = cga + cg.iusedoff as u64 + cgo / 8;
        let mut b: u8 = self.file.decode_at(off)?;
        let mask = 1 << (cgo % 8);
        if (b & mask) != mask {
            panic!("inode_free({inr}): double-free: cgi={cgi}, cgo={cgo}, cga={cga:08x}, off={off:08x}, b={b:02x}, iusedoff={:02x}, ipg={}", cg.iusedoff, sb.ipg);
        }
        b &= !mask;
        self.file.encode_at(off, &b)?;

        // update the CG's free inode count
        cg.cs.nifree += 1;
        // TODO: update cg.time
        self.file.encode_at(cga, &cg)?;

        self.update_sb(|sb| sb.cstotal.nifree += 1)?;

        if let InodeData::Blocks(blocks) = &ino.data {
            let bs = self.superblock.bsize as u64;
            let mut block = vec![0u64; bs as usize / size_of::<u64>()];

            // free direct blocks
            for i in 0..UFS_NDADDR {
                let bno = blocks.direct[i] as u64;
                if bno == 0 {
                    continue;
                }
                let size = self.inode_get_block_size(&ino, i as u64);
                self.blk_free(bno, size as u64)?;
            }

            let blocks = blocks.clone();
            self.inode_free_l1(&mut ino, blocks.indirect[0] as u64, &mut block)?;
            self.inode_free_l2(&mut ino, blocks.indirect[1] as u64, &mut block)?;
            self.inode_free_l3(&mut ino, blocks.indirect[2] as u64, &mut block)?;
        }

        Ok(())
    }

    fn inode_shrink(&mut self, ino: &mut Inode, new_size: u64) -> IoResult<()> {
        let (begin_indir1, begin_indir2, begin_indir3, _) = self.inode_data_zones();
        let sb = &self.superblock;
        let bs = sb.bsize as u64;
        let fs = sb.fsize as u64;
        let (blocks, frags) = Inode::inode_size(bs, fs, new_size);
        log::trace!("inode_shrink(): blocks={blocks}, frags={frags}");
        let blocks = blocks + (frags > 0) as u64;
        let pbp = bs / size_of::<u64>() as u64;

        let InodeData::Blocks(mut iblocks) = ino.data.clone() else {
            return Err(err!(EINVAL));
        };

        let mut block = vec![0u64; bs as usize / size_of::<u64>()];

        if blocks >= begin_indir3 {
            let used = blocks - begin_indir3;
            self.read_pblock(iblocks.indirect[2] as u64, &mut block)?;
            let mut fst = block.clone();

            let off1 = used / pbp / pbp;
            let off2 = used / pbp % pbp;
            let off3 = used % pbp;

            // handle the first table separately, as it only needs to be partially freed
            self.read_pblock(fst[off1 as usize], &mut block)?;
            let mut snd = block.clone();
            self.read_pblock(snd[off2 as usize], &mut block)?;
            for i in off3..pbp {
                let bno = replace(&mut block[i as usize], 0);
                if bno == 0 {
                    continue;
                }
                let size = self
                    .inode_get_block_size(ino, begin_indir3 + off3 * pbp * pbp + off2 * pbp + i);
                self.inode_free_block(ino, bno, size as u64)?;
            }
            self.write_pblock(snd[off2 as usize], &block)?;
            for i in (off2 + 1)..pbp {
                let bno = replace(&mut snd[i as usize], 0);
                self.inode_free_l1(ino, bno, &mut block)?;
            }
            self.write_pblock(fst[off1 as usize], &snd)?;

            // the remaining tables can be freed completely
            for i in (off1 + 1)..pbp {
                let bno = replace(&mut fst[i as usize], 0);
                self.inode_free_l2(ino, bno, &mut block)?;
            }

            self.write_pblock(iblocks.indirect[2] as u64, &block)?;
            ino.data = InodeData::Blocks(iblocks);
            return Ok(());
        }

        self.inode_free_l3(ino, replace(&mut iblocks.indirect[2], 0) as u64, &mut block)?;

        if blocks >= begin_indir2 {
            let used = blocks - begin_indir2;
            self.read_pblock(iblocks.indirect[1] as u64, &mut block)?;
            let mut fst = block.clone();

            let off1 = used / pbp;
            let off2 = used % pbp;

            // handle the first table specially, as it only needs to be partially freed
            self.read_pblock(fst[off1 as usize], &mut block)?;
            for i in off2..pbp {
                let bno = replace(&mut block[i as usize], 0);
                if bno == 0 {
                    continue;
                }
                let size = self.inode_get_block_size(ino, begin_indir2 + off2 * pbp + i);
                self.inode_free_block(ino, bno, size as u64)?;
            }
            self.write_pblock(fst[off1 as usize], &block)?;

            // the remaining tables can be freed completely
            for i in (off1 + 1)..pbp {
                let bno = replace(&mut fst[i as usize], 0);
                self.inode_free_l1(ino, bno, &mut block)?;
            }

            self.write_pblock(iblocks.indirect[1] as u64, &fst)?;
            ino.data = InodeData::Blocks(iblocks);
            return Ok(());
        }

        self.inode_free_l2(ino, replace(&mut iblocks.indirect[1], 0) as u64, &mut block)?;

        if blocks >= begin_indir1 {
            let used = blocks - begin_indir1;
            self.read_pblock(iblocks.indirect[0] as u64, &mut block)?;

            for i in used..pbp {
                let bno = replace(&mut block[i as usize], 0);
                if bno == 0 {
                    continue;
                }
                let size = self.inode_get_block_size(ino, begin_indir1 + i);
                self.inode_free_block(ino, bno, size as u64)?;
            }

            self.write_pblock(iblocks.indirect[0] as u64, &block)?;

            ino.data = InodeData::Blocks(iblocks);
            return Ok(());
        }

        self.inode_free_l1(ino, replace(&mut iblocks.indirect[0], 0) as u64, &mut block)?;

        for i in (blocks as usize)..UFS_NDADDR {
            let bno = replace(&mut iblocks.direct[i], 0) as u64;
            if bno == 0 {
                continue;
            }
            let size = self.inode_get_block_size(ino, i as u64);
            self.inode_free_block(ino, bno, size as u64)?;
        }

        ino.data = InodeData::Blocks(iblocks);
        Ok(())
    }

    pub fn inode_truncate(&mut self, inr: InodeNum, new_size: u64) -> IoResult<()> {
        log::trace!("inode_truncate({inr}, {new_size});");
        self.assert_rw()?;

        let mut ino = self.read_inode(inr)?;
        let old_size = ino.size;

        if new_size < old_size {
            self.inode_shrink(&mut ino, new_size)?;
        }

        ino.size = new_size;

        self.write_inode(inr, &ino)?;

        Ok(())
    }

    fn inode_set_block(
        &mut self,
        inr: InodeNum,
        ino: &mut Inode,
        blkidx: u64,
        block: NonZeroU64,
    ) -> IoResult<()> {
        let sb = &self.superblock;
        let bs = sb.bsize as u64;
        let su64 = size_of::<UfsDaddr>() as u64;
        let pbp = bs / su64;
        let mut data = vec![0u64; pbp as usize];

        let InodeData::Blocks(InodeBlocks { direct, indirect }) = &mut ino.data else {
            log::warn!(
                "inode_set_block({inr}, {blkidx}, {block}): inode doesn't haave data blocks"
            );
            return Err(err!(EIO));
        };

        let mut wb = false;

        match self.decode_blkidx(blkidx)? {
            InodeBlock::Direct(off) => {
                direct[off] = block.get() as i64;
                wb = true;
            }
            InodeBlock::Indirect1(off) => {
                if indirect[0] == 0 {
                    indirect[0] = self.blk_alloc_full_zeroed()?.get() as i64;
                    wb = true;
                }

                let x1 = indirect[0] as u64;
                self.read_pblock(x1, &mut data)?;
                data[off] = block.get();
                self.write_pblock(x1, &data)?;
            }
            InodeBlock::Indirect2(high, low) => {
                if indirect[1] == 0 {
                    indirect[1] = self.blk_alloc_full_zeroed()?.get() as i64;
                    wb = true;
                }

                let x1 = indirect[1] as u64;
                self.read_pblock(x1, &mut data)?;

                if data[high] == 0 {
                    data[high] = self.blk_alloc_full_zeroed()?.get();
                    self.write_pblock(x1, &data)?;
                }

                let x2 = data[high];
                self.read_pblock(x2, &mut data)?;
                data[low] = block.get();
                self.write_pblock(x2, &data)?;
            }
            InodeBlock::Indirect3(high, mid, low) => {
                if indirect[2] == 0 {
                    indirect[2] = self.blk_alloc_full_zeroed()?.get() as i64;
                    wb = true;
                }

                let x1 = indirect[2] as u64;
                self.read_pblock(x1, &mut data)?;

                if data[high] == 0 {
                    data[high] = self.blk_alloc_full_zeroed()?.get();
                    self.write_pblock(x1, &data)?;
                }

                let x2 = data[high];
                self.read_pblock(x2, &mut data)?;
                if data[mid] == 0 {
                    data[mid] = self.blk_alloc_full_zeroed()?.get();
                    self.write_pblock(x2, &data)?;
                }

                let x3 = data[mid];
                self.read_pblock(x3, &mut data)?;
                data[low] = block.get();
                self.write_pblock(x3, &data)?;
            }
        }

        if wb {
            self.write_inode(inr, ino)?;
        }

        Ok(())
    }

    pub(super) fn inode_alloc_block(
        &mut self,
        inr: InodeNum,
        ino: &mut Inode,
        blkidx: u64,
        size: u64,
    ) -> IoResult<(NonZeroU64, u64)> {
        let (block, block_size) = self.blk_alloc(size)?;
        log::trace!(
            "inode_alloc_block({inr}): old_blocks: {}, block_size={block_size}",
            ino.blocks
        );
        ino.blocks += block_size / STAT_BLKSIZE;
        log::trace!("inode_alloc_block({inr}): new_blocks: {}", ino.blocks);
        self.inode_set_block(inr, ino, blkidx, block)?;
        log::trace!(
            "inode_alloc_block({inr}, {blkidx}, {size}): block={block}, block_size={block_size}"
        );
        Ok((block, block_size))
    }

    fn inode_free_block(&mut self, ino: &mut Inode, bno: u64, size: u64) -> IoResult<()> {
        self.blk_free(bno, size)?;
        log::trace!("inode_free_block({bno}, {size}): old_blocks={}", ino.blocks);
        ino.blocks -= size / STAT_BLKSIZE;
        Ok(())
    }
}
