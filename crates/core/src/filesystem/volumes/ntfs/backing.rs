//! Bounded read and seek adapters for image and BitLocker partitions.
use super::*;

/// Backing source for an NTFS volume. Either a raw partition slice of a
/// `DiskImage` (the common case) or the plaintext stream produced by an
/// unlocked BitLocker volume.
#[derive(Clone)]
pub(super) enum PartitionBacking<'a> {
    Image {
        image: &'a DiskImage,
        base_offset: u64,
        length_bytes: u64,
        layout: Option<crate::evidence::partitions::lvm2::layout::LinearLayout>,
    },
    Bitlocker {
        unlocked: Arc<UnlockedVolume>,
        length_bytes: u64,
    },
}

impl<'a> PartitionBacking<'a> {
    pub(super) fn length(&self) -> u64 {
        match self {
            Self::Image { length_bytes, .. } => *length_bytes,
            Self::Bitlocker { length_bytes, .. } => *length_bytes,
        }
    }

    /// Coordinate base for run capture: original image offset for a slice,
    /// zero for a mapped LV. `physical_ranges` resolves original image offsets.
    /// BitLocker has no direct source runs and returns `None`.
    pub(super) fn base_offset(&self) -> Option<u64> {
        match self {
            Self::Image {
                base_offset,
                layout: None,
                ..
            } => Some(*base_offset),
            Self::Image {
                layout: Some(_), ..
            } => Some(0),
            Self::Bitlocker { .. } => None,
        }
    }

    pub(super) fn physical_ranges(&self, position: u64, length: u64) -> Result<Vec<(u64, u64)>> {
        match self {
            Self::Image {
                layout: Some(layout),
                ..
            } => Ok(layout
                .ranges(position, length)?
                .into_iter()
                .map(|range| (range.physical_offset, range.size))
                .collect()),
            Self::Image {
                base_offset,
                layout: None,
                ..
            } => Ok(vec![(
                base_offset
                    .checked_add(position)
                    .ok_or_else(|| anyhow!("NTFS source offset overflow"))?,
                length,
            )]),
            Self::Bitlocker { .. } => Ok(Vec::new()),
        }
    }

    pub(super) fn read_into(&self, position: u64, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Image {
                image,
                base_offset,
                layout,
                ..
            } => {
                let bytes = crate::filesystem::volumes::lvm::read_backing_range(
                    image,
                    layout.as_ref(),
                    base_offset + position,
                    buf.len(),
                )
                .map_err(|err| io::Error::other(err.to_string()))?;
                let copied = bytes.len();
                buf[..copied].copy_from_slice(&bytes);
                Ok(copied)
            }
            Self::Bitlocker { unlocked, .. } => unlocked
                .read_at(position, buf)
                .map_err(|err| io::Error::other(err.to_string())),
        }
    }

    pub(super) fn reader(&self) -> PartitionReader<'a> {
        PartitionReader {
            backing: self.clone(),
            position: 0,
        }
    }
}

pub(super) struct PartitionReader<'a> {
    pub(super) backing: PartitionBacking<'a>,
    pub(super) position: u64,
}

impl Read for PartitionReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let length = self.backing.length();
        if self.position >= length || buf.is_empty() {
            return Ok(0);
        }
        let remaining = length - self.position;
        let want = (buf.len() as u64).min(remaining) as usize;
        let copied = self.backing.read_into(self.position, &mut buf[..want])?;
        self.position += copied as u64;
        Ok(copied)
    }
}

impl Seek for PartitionReader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let length = self.backing.length();
        let target = match pos {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(delta) => self.position as i128 + delta as i128,
            SeekFrom::End(delta) => length as i128 + delta as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NTFS partition seek before start",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}
