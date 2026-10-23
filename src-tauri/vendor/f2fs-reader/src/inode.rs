//! Inodes, inline areas, all five node roots and seekable logical block reads.
use super::*;

pub(super) struct Inode {
    pub kind: FileKind,
    pub mode: u16,
    pub size: u64,
    pub times: FileTimes,
    pub bytes: [u8; 4096],
    pub address_start: usize,
    pub address_count: usize,
    pub inline_start: usize,
    pub inline_end: usize,
    pub inline_flags: u8,
    pub unreadable: Option<&'static str>,
}

impl<S: BlockSource> F2fs<S> {
    pub(super) fn inode(&mut self, nid: u32) -> Result<Inode> {
        let bytes = self.node_block(nid, Some(nid))?;
        let mode = u16_at(&bytes, 0)?;
        let inline_flags = bytes[3];
        let unreadable = if bytes[2] & 4 != 0 {
            Some("F2FS encrypted inode")
        } else if u32_at(&bytes, 80)? & (0x4 | 0x40000000 | 0x80000000) != 0 {
            Some("F2FS compressed/casefold/device-alias inode")
        } else {
            None
        };
        let extra = if inline_flags & 0x20 != 0 {
            u16_at(&bytes, 360)? as usize
        } else {
            0
        };
        require(
            extra % 4 == 0
                && extra <= 36
                && (inline_flags & 0x20 == 0 || (extra >= 4 && self.feature & 8 != 0)),
            "F2FS extra inode attributes",
        )?;
        let xattr = if self.feature & 0x40 != 0 && extra >= 4 {
            u16_at(&bytes, 362)? as usize
        } else if inline_flags & 5 != 0 {
            50
        } else {
            0
        };
        require(extra / 4 + xattr + 1 < 923, "F2FS inline xattr bounds")?;
        if self.feature & 0x20 != 0 && extra >= 12 {
            let mut crc = crc32_raw(u32::MAX, &self.uuid);
            crc = crc32_raw(crc, &nid.to_le_bytes());
            crc = crc32_raw(crc, &bytes[68..72]);
            crc = crc32_raw(crc, &bytes[..368]);
            crc = crc32_raw(crc, &[0; 4]);
            crc = crc32_raw(crc, &bytes[372..]);
            require(crc == u32_at(&bytes, 368)?, "F2FS inode checksum")?;
        }
        let time = |seconds: usize, nanos: usize| -> Result<Option<(i64, u32)>> {
            let seconds = u64_at(&bytes, seconds)? as i64;
            let nanos = u32_at(&bytes, nanos)?;
            require(nanos < 1_000_000_000, "F2FS timestamp nanoseconds")?;
            Ok(Some((seconds, nanos)))
        };
        let times = FileTimes {
            modified: time(48, 64)?,
            accessed: time(32, 56)?,
            changed: time(40, 60)?,
            created: if self.feature & 0x100 != 0 && extra >= 24 {
                time(372, 380)?
            } else {
                None
            },
        };
        let kind = match mode & 0xf000 {
            0x8000 => FileKind::Regular,
            0x4000 => FileKind::Directory,
            0xa000 => FileKind::Symlink,
            _ => FileKind::Other,
        };
        let size = u64_at(&bytes, 16)?;
        let inline_start = 364 + extra;
        let inline_end = 360 + (923 - xattr) * 4;
        require(
            inline_flags & 2 == 0
                || (matches!(kind, FileKind::Regular | FileKind::Symlink)
                    && size <= (inline_end - inline_start) as u64),
            "F2FS inline file length/kind",
        )?;
        require(
            inline_flags & 4 == 0 || kind == FileKind::Directory,
            "F2FS inline directory kind",
        )?;
        Ok(Inode {
            kind,
            mode,
            size,
            times,
            bytes,
            address_start: 360 + extra,
            address_count: 923 - extra / 4 - xattr,
            inline_start,
            inline_end,
            inline_flags,
            unreadable,
        })
    }

    fn indirect(&mut self, nid: u32, owner: u32, levels: u32, index: u64) -> Result<u32> {
        if nid == 0 {
            return Ok(0);
        }
        let bytes = self.node_block(nid, Some(owner))?;
        let stride = 1018u64.pow(levels);
        let slot = usize::try_from(index / stride)
            .map_err(|_| FoldError::Corrupt("F2FS indirect index"))?;
        require(slot < 1018, "F2FS indirect slot")?;
        let value = u32_at(&bytes, slot * 4)?;
        if levels == 0 {
            Ok(value)
        } else {
            self.indirect(value, owner, levels - 1, index % stride)
        }
    }

    pub(super) fn physical(&mut self, owner: u32, inode: &Inode, mut index: u64) -> Result<u32> {
        if index < inode.address_count as u64 {
            return u32_at(&inode.bytes, inode.address_start + index as usize * 4);
        }
        index -= inode.address_count as u64;
        for (slot, levels) in [0, 0, 1, 1, 2].into_iter().enumerate() {
            let span = 1018u64.pow(levels + 1);
            if index < span {
                return self.indirect(u32_at(&inode.bytes, 4052 + slot * 4)?, owner, levels, index);
            }
            index -= span;
        }
        Err(FoldError::Corrupt("F2FS file exceeds node tree"))
    }

    pub(super) fn content(
        &mut self,
        nid: u32,
        inode: &Inode,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize> {
        if let Some(reason) = inode.unreadable {
            return Err(FoldError::Unsupported(reason));
        }
        if offset >= inode.size {
            return Ok(0);
        }
        let count = (inode.size - offset).min(buffer.len() as u64) as usize;
        if inode.inline_flags & 2 != 0 {
            let start = inode.inline_start + offset as usize;
            buffer[..count].copy_from_slice(&inode.bytes[start..start + count]);
            return Ok(count);
        }
        let mut done = 0;
        while done < count {
            let logical = offset + done as u64;
            let within = (logical % 4096) as usize;
            let take = (count - done).min(4096 - within);
            match self.physical(nid, inode, logical / 4096)? {
                0 | u32::MAX => buffer[done..done + take].fill(0),
                address => {
                    let bytes = self.data_block(address)?;
                    buffer[done..done + take].copy_from_slice(&bytes[within..within + take]);
                }
            }
            done += take;
        }
        Ok(done)
    }
}
