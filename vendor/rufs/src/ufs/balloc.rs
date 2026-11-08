use super::*;
use crate::err;

impl<R: Backend> Ufs<R> {
    /// See /sys/ufs/ffs/ffs_subr.c: ffs_isblock()
    fn cg_isfreeblock(&mut self, cgo: u64, cg: &CylGroup, bno: u64) -> IoResult<bool> {
        let sb = &self.superblock;
        let frag = sb.frag as u64;
        let freeoff = cg.freeoff as u64;
        let h = bno / frag;

        assert!(bno < cg.ndblk as u64);

        let mut cp = |h| self.file.decode_at::<u8>(cgo + freeoff + h);

        let st = match frag {
            8 => cp(h)? == 0xff,
            4 => {
                let mask = 0x0f << ((h & 0x01) << 2);
                cp(h >> 1)? & mask == mask
            }
            2 => {
                let mask = 0x03 << ((h & 0x03) << 1);
                cp(h >> 2)? & mask == mask
            }
            1 => {
                let mask = 0x01 << (h & 0x07);
                cp(h >> 3)? & mask == mask
            }
            _ => unreachable!("invalid fragment size: {frag}"),
        };

        Ok(st)
    }

    fn cg_isfreefrag(&mut self, cgo: u64, cg: &CylGroup, bno: u64) -> IoResult<bool> {
        let freeoff = cg.freeoff as u64;
        let off = cgo + freeoff + bno / 8;
        let mask = 1 << (bno % 8);
        let b: u8 = self.file.decode_at(off)?;

        Ok(b & mask == mask)
    }

    /// See /sys/ufs/ffs/ffs_subr.c: ffs_setblock() and ffs_clrblock()
    fn cg_setblock(&mut self, cgo: u64, cg: &CylGroup, bno: u64, free: bool) -> IoResult<()> {
        let sb = &self.superblock;
        let frag = sb.frag as u64;
        let freeoff = cg.freeoff as u64;
        let h = bno / frag;

        assert!(bno < cg.ndblk as u64);

        let mut cp = |h, x: u8| {
            let old = self.file.decode_at::<u8>(cgo + freeoff + h)?;
            let new = if free { old | x } else { old & !x };
            self.file.encode_at(cgo + freeoff + h, &new)
        };

        match frag {
            8 => cp(h, 0xff),
            4 => cp(h >> 1, 0x0f << ((h & 0x01) << 2)),
            2 => cp(h >> 2, 0x03 << ((h & 0x03) << 1)),
            1 => cp(h >> 3, 0x01 << (h & 0x07)),
            _ => unreachable!("invalid fragment size: {frag}"),
        }
    }

    fn cg_setfrag(&mut self, cgo: u64, cg: &CylGroup, bno: u64, free: bool) -> IoResult<()> {
        let freeoff = cg.freeoff as u64;
        let off = cgo + freeoff + bno / 8;
        let mut b = self.file.decode_at::<u8>(off)?;
        let mask = 1 << (bno % 8);

        if free {
            b |= mask;
        } else {
            b &= !mask;
        }

        self.file.encode_at(off, &b)?;

        Ok(())
    }

    /// See /sys/ufs/ffs/ffs_subr.c: ffs_isfreeblock()
    fn cg_isfullblock(&mut self, cgo: u64, cg: &CylGroup, bno: u64) -> IoResult<bool> {
        let sb = &self.superblock;
        let frag = sb.frag as u64;
        let freeoff = cg.freeoff as u64;
        let h = bno / frag;

        let mut cp = |h| self.file.decode_at::<u8>(cgo + freeoff + h);

        let st = match frag {
            8 => cp(h)? == 0,
            4 => (cp(h >> 1)? & (0x0f << ((h & 0x01) << 2))) == 0,
            2 => (cp(h >> 2)? & (0x03 << ((h & 0x03) << 1))) == 0,
            1 => (cp(h >> 3)? & (0x01 << (h & 0x07))) == 0,
            _ => unreachable!("invalid fragment size: {frag}"),
        };

        Ok(st)
    }

    /// See: /sys/ufs/ffs/ffs_alloc.c: ffs_blkfree()
    pub(super) fn blk_free(&mut self, bno: u64, size: u64) -> IoResult<()> {
        log::trace!("blk_free(bno={bno}, size={size});");
        self.assert_rw()?;

        if bno == 0 {
            return Ok(());
        }

        let sb = &self.superblock;
        let fsize = sb.fsize as u64;
        let bsize = sb.bsize as u64;
        let nfrag = size / fsize;
        let fpg = sb.fpg as u64;
        let frag = sb.frag;

        assert_ne!(size, 0);
        assert!(size <= bsize);
        assert!(size.is_multiple_of(fsize));
        assert!(bno % bsize / fsize + nfrag <= sb.frag as u64);

        let cgi = bno / fpg;
        let cgo = self.cg_addr(cgi);
        let mut cg: CylGroup = self.file.decode_at(cgo)?;

        let bno = bno % fpg;

        // TODO: fragacct and cg.cg_frsum

        if size == bsize {
            if !self.cg_isfullblock(cgo, &cg, bno)? {
                panic!("freeing free block: cgi={cgi}, bno={bno}");
            }

            self.cg_setblock(cgo, &cg, bno, true)?;

            cg.cs.nbfree += 1;
            self.update_sb(|sb| sb.cstotal.nbfree += 1)?;
        } else {
            // deallocate fragments
            for i in 0..nfrag {
                let bno = bno + i;
                if self.cg_isfreefrag(cgo, &cg, bno)? {
                    panic!("freeing free frag: cgi={cgi}, bno={bno}");
                }
                self.cg_setfrag(cgo, &cg, bno, true)?;
            }

            cg.cs.nffree += nfrag as i32;
            self.update_sb(|sb| sb.cstotal.nffree += nfrag as i64)?;

            // if a complete block has been reassembled, account for it
            if self.cg_isfreeblock(cgo, &cg, bno)? {
                cg.cs.nffree -= frag;
                cg.cs.nbfree += 1;
                self.update_sb(|sb| {
                    sb.cstotal.nffree -= frag as i64;
                    sb.cstotal.nbfree += 1;
                })?;
            }
        }

        self.file.encode_at(cgo, &cg)?;

        Ok(())
    }

    pub(super) fn blk_alloc_full(&mut self) -> IoResult<NonZeroU64> {
        let sb = &self.superblock;
        let frag = sb.frag as u64;
        let fpg = sb.fpg as u64;

        for i in 0..(sb.ncg as u64) {
            let cgo = self.cg_addr(i);
            let mut cg: CylGroup = self.file.decode_at(cgo)?;
            if cg.cs.nbfree <= 0 {
                continue;
            }

            let ndblk = cg.ndblk as u64;

            for bno in (0..ndblk).filter(|bno| bno % frag == 0) {
                if !self.cg_isfreeblock(cgo, &cg, bno)? {
                    continue;
                }

                self.cg_setblock(cgo, &cg, bno, false)?;
                cg.cs.nbfree -= 1;
                self.file.encode_at(cgo, &cg)?;
                self.update_sb(|sb| sb.cstotal.nbfree -= 1)?;
                let blkno = NonZeroU64::new(i * fpg + bno).unwrap();
                log::trace!("blk_alloc_full(): {blkno}");
                return Ok(blkno);
            }
        }

        Err(err!(ENOSPC))
    }

    pub(super) fn blk_alloc(&mut self, size: u64) -> IoResult<(NonZeroU64, u64)> {
        self.assert_rw()?;

        let sb = &self.superblock;
        let fsize = sb.fsize as u64;
        let bsize = sb.bsize as u64;

        assert!(size > 0);
        assert!(size <= bsize);
        assert!(size.is_multiple_of(fsize));

        // for now only allocate full blocks
        let blk = self.blk_alloc_full()?;
        Ok((blk, bsize))
    }

    pub(super) fn blk_alloc_full_zeroed(&mut self) -> IoResult<NonZeroU64> {
        let bs = self.superblock.bsize as usize;
        let fs = self.superblock.fsize as u64;
        let blkno = self.blk_alloc_full()?;
        let data = vec![0u8; bs];
        self.file.encode_at(blkno.get() * fs, &data)?;
        Ok(blkno)
    }
}
