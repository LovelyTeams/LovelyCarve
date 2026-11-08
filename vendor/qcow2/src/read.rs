//! Checked L1/L2 translation, logical holes, DEFLATE and sector reads.
use crate::{header::check_range, Error, Qcow2, Result};
use byteorder::{BigEndian, ByteOrder};
use flate2::{Decompress, FlushDecompress, Status};
use positioned_io::{ReadAt, Size};
use std::{
    cmp::min,
    io::{self, Read, Seek, SeekFrom},
    sync::Arc,
};

const OFFSET: u64 = 0x00ff_ffff_ffff_fe00;
enum Entry {
    Empty,
    Zero,
    Standard(u64),
    Compressed { position: u64, size: usize },
}

impl<I: ReadAt> Qcow2<I> {
    fn entry(&self, guest: u64) -> Result<Entry> {
        let cluster = guest / self.cluster_size();
        let l1_index = cluster / self.header.l2_entries();
        let l1_offset = usize::try_from(l1_index * 8)
            .map_err(|_| Error::FileFormat("L1 index overflow".into()))?;
        let bytes = self
            .l1
            .get(l1_offset..l1_offset + 8)
            .ok_or_else(|| Error::FileFormat("L1 index out of bounds".into()))?;
        let l1_entry = BigEndian::read_u64(bytes);
        let table = if self.header.version == 1 {
            if l1_entry & (1 << 63) != 0 {
                return Err(Error::FileFormat("compressed L1 table".into()));
            }
            l1_entry
        } else {
            if l1_entry & !(OFFSET | (1 << 63)) != 0 {
                return Err(Error::FileFormat("reserved L1 bits".into()));
            }
            l1_entry & OFFSET
        };
        if table == 0 {
            return Ok(Entry::Empty);
        }
        if table % self.cluster_size() != 0 || table < self.cluster_size() {
            return Err(Error::FileFormat(
                "unaligned or header-overlapping L2 table".into(),
            ));
        }
        check_range(
            table,
            self.header.l2_entries() * 8,
            self.source_size,
            "L2 table",
        )?;
        let position = table
            .checked_add((cluster % self.header.l2_entries()) * 8)
            .ok_or_else(|| Error::FileFormat("L2 offset overflow".into()))?;
        let cached = self.l2_cache.lock()?.get(&position).copied();
        let entry = match cached {
            Some(entry) => entry,
            None => {
                let mut bytes = [0; 8];
                self.io.read_exact_at(position, &mut bytes)?;
                let entry = BigEndian::read_u64(&bytes);
                self.l2_cache.lock()?.put(position, entry);
                entry
            }
        };
        let compressed_bit = if self.header.version == 1 {
            1 << 63
        } else {
            1 << 62
        };
        if entry & compressed_bit != 0 {
            if self.header.encryption != 0 {
                return Err(Error::UnsupportedFeature(
                    "simultaneous encryption and compression".into(),
                ));
            }
            if self.header.version != 1 && entry & (1 << 63) != 0 {
                return Err(Error::FileFormat("copied bit on compressed cluster".into()));
            }
            let shift = if self.header.version == 1 {
                63 - self.header.cluster_bits
            } else {
                70 - self.header.cluster_bits
            };
            let descriptor = entry & !compressed_bit;
            let position = descriptor & ((1u64 << shift) - 1);
            let size = if self.header.version == 1 {
                descriptor >> shift
            } else {
                ((descriptor >> shift) + 1) * 512 - (position & 511)
            };
            if position < self.cluster_size() || size == 0 || size > self.cluster_size() * 2 {
                return Err(Error::FileFormat("invalid compressed descriptor".into()));
            }
            // QEMU may omit trailing sector padding at physical EOF. Decode the
            // available bytes and still require a complete stream and cluster.
            let size = if self.header.version != 1 {
                self.source_size
                    .map_or(size, |length| size.min(length.saturating_sub(position)))
            } else {
                size
            };
            check_range(position, size, self.source_size, "compressed cluster")?;
            if size == 0 {
                return Err(Error::FileFormat("compressed cluster beyond EOF".into()));
            }
            return Ok(Entry::Compressed {
                position,
                size: size as usize,
            });
        }
        let position = if self.header.version == 1 {
            entry
        } else {
            let zero_bit = if self.header.version == 3 { 1 } else { 0 };
            if entry & !(OFFSET | (1 << 63) | zero_bit) != 0 {
                return Err(Error::FileFormat("reserved L2 bits".into()));
            }
            let position = entry & OFFSET;
            if entry & zero_bit != 0 {
                if position != 0
                    && (!position.is_multiple_of(self.cluster_size())
                        || position < self.cluster_size())
                {
                    return Err(Error::FileFormat("invalid allocated zero cluster".into()));
                }
                if position != 0 {
                    check_range(position, 1, self.source_size, "allocated zero cluster")?;
                }
                return Ok(Entry::Zero);
            }
            position
        };
        if position == 0 {
            return Ok(Entry::Empty);
        }
        if position % self.cluster_size() != 0 || position < self.cluster_size() {
            return Err(Error::FileFormat(
                "unaligned or header-overlapping data cluster".into(),
            ));
        }
        Ok(Entry::Standard(position))
    }

    fn compressed(&self, guest_cluster: u64, position: u64, size: usize) -> Result<Arc<Vec<u8>>> {
        // Guest identity keeps the cache correct even for shared host clusters.
        if let Some(bytes) = self.decoded_cache.lock()?.get(&guest_cluster) {
            return Ok(Arc::clone(bytes));
        }
        let mut input = vec![0; size];
        self.io.read_exact_at(position, &mut input)?;
        let mut output = vec![0; self.cluster_size() as usize + 1];
        // QCOW uses raw DEFLATE with QEMU's 12-bit window, no zlib wrapper.
        let mut decoder = Decompress::new(false);
        let status = decoder
            .decompress(&input, &mut output, FlushDecompress::Finish)
            .map_err(|error| Error::FileFormat(format!("invalid DEFLATE cluster: {error}")))?;
        if status != Status::StreamEnd || decoder.total_out() != self.cluster_size() {
            return Err(Error::FileFormat(
                "incomplete or oversized DEFLATE cluster".into(),
            ));
        }
        output.truncate(self.cluster_size() as usize);
        let output = Arc::new(output);
        self.decoded_cache
            .lock()?
            .put(guest_cluster, Arc::clone(&output));
        Ok(output)
    }

    pub(crate) fn guest_read(
        &self,
        position: u64,
        buffer: &mut [u8],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<usize> {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        if self.header.encryption != 0 && self.key.is_none() {
            return Err(Error::KeyRequired);
        }
        if position >= self.guest_size() || buffer.is_empty() {
            return Ok(0);
        }
        let length = min(buffer.len() as u64, self.guest_size() - position) as usize;
        let mut done = 0;
        while done < length {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let guest = position + done as u64;
            let offset = guest % self.cluster_size();
            let cluster = guest - offset;
            let count = min((length - done) as u64, self.cluster_size() - offset) as usize;
            let output = &mut buffer[done..done + count];
            match self.entry(guest)? {
                Entry::Zero => output.fill(0),
                Entry::Empty => {
                    if self.header.backing_name.is_some() {
                        let parent = self.backing.as_ref().ok_or(Error::MissingBacking)?;
                        let parent_size = self.backing_size.ok_or(Error::MissingBacking)?;
                        let available =
                            min(count as u64, parent_size.saturating_sub(guest)) as usize;
                        parent.read_exact_at(guest, &mut output[..available])?;
                        output[available..].fill(0);
                    } else {
                        output.fill(0);
                    }
                }
                Entry::Standard(host) => {
                    if let Some(key) = self.key.as_ref().filter(|_| self.header.encryption == 1) {
                        // Read only complete sectors covering the request, so
                        // random reads don't need a whole-cluster allocation.
                        let sector_start = offset / 512 * 512;
                        let sector_end = crate::int::div_ceil(offset + count as u64, 512) * 512;
                        let mut bytes =
                            zeroize::Zeroizing::new(vec![0; (sector_end - sector_start) as usize]);
                        check_range(host, sector_end, self.source_size, "encrypted data")?;
                        self.io.read_exact_at(host + sector_start, &mut bytes)?;
                        key.decrypt(cluster + sector_start, &mut bytes)?;
                        let start = (offset - sector_start) as usize;
                        output.copy_from_slice(&bytes[start..start + count]);
                    } else {
                        check_range(
                            host,
                            offset + count as u64,
                            self.source_size,
                            "allocated data",
                        )?;
                        self.io.read_exact_at(host + offset, output)?;
                    }
                }
                Entry::Compressed { position, size } => {
                    let bytes = self.compressed(cluster, position, size)?;
                    output.copy_from_slice(&bytes[offset as usize..offset as usize + count]);
                }
            }
            if cancelled() {
                return Err(Error::Cancelled);
            }
            done += count;
        }
        Ok(length)
    }
}

/// Logical disk cursor. Positioned reads leave its sequential offset unchanged.
pub struct Reader<'a, I: ReadAt> {
    image: &'a Qcow2<I>,
    position: u64,
}
impl<'a, I: ReadAt> Reader<'a, I> {
    pub(crate) fn new(image: &'a Qcow2<I>) -> Self {
        Self { image, position: 0 }
    }
    /// Current sequential offset.
    pub fn position(&self) -> u64 {
        self.position
    }
}
impl<I: ReadAt> ReadAt for Reader<'_, I> {
    fn read_at(&self, position: u64, buffer: &mut [u8]) -> io::Result<usize> {
        self.image.read_at(position, buffer).map_err(Into::into)
    }
}
impl<I: ReadAt> Size for Reader<'_, I> {
    fn size(&self) -> io::Result<Option<u64>> {
        Ok(Some(self.image.guest_size()))
    }
}
impl<I: ReadAt> Read for Reader<'_, I> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.image.read_at(self.position, buffer)?;
        self.position += count as u64;
        Ok(count)
    }
}
impl<I: ReadAt> Seek for Reader<'_, I> {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        let position = match offset {
            SeekFrom::Start(position) => position as i128,
            SeekFrom::Current(delta) => self.position as i128 + delta as i128,
            SeekFrom::End(delta) => self.image.guest_size() as i128 + delta as i128,
        };
        if !(0..=i64::MAX as i128).contains(&position) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "QCOW seek outside supported range",
            ));
        }
        self.position = position as u64;
        Ok(self.position)
    }
}
