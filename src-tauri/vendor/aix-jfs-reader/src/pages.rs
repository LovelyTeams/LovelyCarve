use lamfold::{BlockSource, FileKind, FoldError, Result};

use crate::{AixJfs, Inode, PAGE, be32, require};

const DIRECT_PAGES: u64 = 8;
const SINGLE_ENTRIES: u64 = PAGE / 4;
const DOUBLE_ENTRIES: u64 = PAGE / 8;
const FRAGMENT_ADDRESS_MASK: u32 = 0x0fff_ffff;

impl<S: BlockSource> AixJfs<S> {
    pub(crate) fn read_inode(
        &mut self,
        inode: &Inode,
        mut offset: u64,
        mut bytes: &mut [u8],
    ) -> Result<()> {
        require(
            offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= inode.size),
            "AIX JFS read outside logical inode",
        )?;
        if inode.kind() == FileKind::Symlink && inode.size < inode.addresses.len() as u64 {
            let start = offset as usize;
            bytes.copy_from_slice(&inode.addresses[start..start + bytes.len()]);
            return Ok(());
        }
        let pages = inode.size.div_ceil(PAGE);
        while !bytes.is_empty() {
            let logical = offset / PAGE;
            let within = offset % PAGE;
            let count = (PAGE - within).min(bytes.len() as u64) as usize;
            let (head, tail) = bytes.split_at_mut(count);
            let address = self.page_address(inode, pages, logical)?;
            if let Some((physical, allocated)) = self.fragment_extent(address)? {
                let logical_bytes = (inode.size - logical * PAGE).min(PAGE);
                require(
                    logical_bytes <= allocated && (pages <= DIRECT_PAGES || allocated == PAGE),
                    "AIX JFS logical page exceeds allocated fragments",
                )?;
                self.read_volume(physical + within, head)?;
            } else {
                require(
                    inode.kind() == FileKind::Regular,
                    "AIX JFS unallocated directory or link block",
                )?;
                head.fill(0);
            }
            offset += count as u64;
            bytes = tail;
        }
        Ok(())
    }

    fn page_address(&mut self, inode: &Inode, pages: u64, logical: u64) -> Result<u32> {
        if pages <= DIRECT_PAGES {
            return Ok(be32(&inode.addresses, logical as usize * 4));
        }
        let root = be32(&inode.addresses, 36);
        if pages <= SINGLE_ENTRIES {
            return self.address_entry(root, logical * 4);
        }
        let slot = logical / SINGLE_ENTRIES;
        require(slot < DOUBLE_ENTRIES, "AIX JFS double-indirect map index")?;
        let leaf = self.address_entry(root, slot * 8 + 4)?;
        require(
            leaf == 0 || (leaf & FRAGMENT_ADDRESS_MASK) != (root & FRAGMENT_ADDRESS_MASK),
            "AIX JFS cyclic double-indirect map",
        )?;
        self.address_entry(leaf, (logical % SINGLE_ENTRIES) * 4)
    }

    fn address_entry(&mut self, address: u32, in_page: u64) -> Result<u32> {
        let Some((physical, bytes)) = self.fragment_extent(address)? else {
            return Ok(0);
        };
        require(
            bytes == PAGE && in_page + 4 <= PAGE,
            "AIX JFS invalid indirect block size or slot",
        )?;
        let mut word = [0; 4];
        self.read_volume(physical + in_page, &mut word)?;
        Ok(u32::from_be_bytes(word))
    }

    fn fragment_extent(&self, address: u32) -> Result<Option<(u64, u64)>> {
        if address == 0 {
            return Ok(None);
        }
        let fragment = u64::from(address & FRAGMENT_ADDRESS_MASK);
        let unused = u64::from((address >> 28) & 7);
        let per_page = PAGE / self.header.fragment_size;
        require(
            address >> 31 == 0 && fragment != 0 && unused < per_page,
            "AIX JFS invalid packed fragment address",
        )?;
        let allocated = (per_page - unused) * self.header.fragment_size;
        let offset = fragment * self.header.fragment_size;
        if offset + allocated > self.header.volume_size {
            return Err(FoldError::Corrupt("AIX JFS fragment extent exceeds volume"));
        }
        Ok(Some((offset, allocated)))
    }
}
