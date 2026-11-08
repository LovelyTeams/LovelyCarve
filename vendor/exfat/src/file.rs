use crate::cluster::ClustersReader;
use crate::disk::DiskPartition;
use crate::entries::StreamEntry;
use crate::fat::Fat;
use crate::param::Params;
use crate::timestamp::Timestamps;
use alloc::sync::Arc;
use core::cmp::min;
use thiserror::Error;

/// Represents a file in an exFAT filesystem.
pub struct File<D> {
    name: String,
    len: u64,
    valid_len: u64,
    attributes: crate::FileAttributes,
    entry_offset: u64,
    reader: Option<ClustersReader<Arc<D>, Arc<Params>>>,
    timestamps: Timestamps,
}

impl<D> File<D> {
    pub(crate) fn new(
        disk: &Arc<D>,
        params: &Arc<Params>,
        fat: &Fat,
        name: String,
        stream: StreamEntry,
        timestamps: Timestamps,
        attributes: crate::FileAttributes,
        entry_offset: u64,
    ) -> Result<Self, NewError> {
        // Create a cluster reader.
        let alloc = stream.allocation();
        let first_cluster = alloc.first_cluster();
        let len = alloc.data_length();
        let valid_len = stream.valid_data_length();
        let reader = if len == 0 {
            None
        } else {
            match ClustersReader::new(
                disk.clone(),
                params.clone(),
                fat,
                first_cluster,
                Some(len),
                Some(stream.no_fat_chain()),
            ) {
                Ok(v) => Some(v),
                Err(e) => return Err(NewError::CreateClustersReaderFailed(first_cluster, len, e)),
            }
        };

        Ok(Self {
            name,
            len,
            valid_len,
            attributes,
            entry_offset,
            reader,
            timestamps,
        })
    }

    pub fn attributes(&self) -> crate::FileAttributes {
        self.attributes
    }
    pub fn entry_offset(&self) -> u64 {
        self.entry_offset
    }
    pub fn valid_data_length(&self) -> u64 {
        self.valid_len
    }
    pub fn extents(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.reader.iter().flat_map(ClustersReader::extents)
    }

    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn timestamps(&self) -> &Timestamps {
        &self.timestamps
    }
}

#[cfg(feature = "std")]
impl<D> std::io::Seek for File<D> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        use std::io::{Error, ErrorKind, SeekFrom};

        // Check if empty file.
        let r = match &mut self.reader {
            Some(v) => v,
            None => return std::io::empty().seek(pos),
        };

        // Get absolute offset.
        let o = match pos {
            SeekFrom::Start(v) => min(v, r.data_length()),
            SeekFrom::End(v) => {
                if v >= 0 {
                    r.data_length()
                } else if let Some(v) = r.data_length().checked_sub(v.unsigned_abs()) {
                    v
                } else {
                    return Err(Error::from(ErrorKind::InvalidInput));
                }
            }
            SeekFrom::Current(v) => v.try_into().map_or_else(
                |_| {
                    r.stream_position()
                        .checked_sub(v.unsigned_abs())
                        .ok_or_else(|| Error::from(ErrorKind::InvalidInput))
                },
                |v| Ok(min(r.stream_position().saturating_add(v), r.data_length())),
            )?,
        };

        assert!(r.seek(o));

        Ok(o)
    }

    fn rewind(&mut self) -> std::io::Result<()> {
        let r = match &mut self.reader {
            Some(v) => v,
            None => return Ok(()),
        };

        r.rewind();

        Ok(())
    }

    fn stream_position(&mut self) -> std::io::Result<u64> {
        let r = match &mut self.reader {
            Some(v) => v,
            None => return Ok(0),
        };

        Ok(r.stream_position())
    }
}

#[cfg(feature = "std")]
impl<D: DiskPartition> std::io::Read for File<D> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match &mut self.reader {
            Some(reader) => {
                let position = reader.stream_position();
                let want = (self.len.saturating_sub(position)).min(buf.len() as u64) as usize;
                let initialized = self.valid_len.saturating_sub(position).min(want as u64) as usize;
                if initialized > 0 {
                    return reader.read(&mut buf[..initialized]);
                }
                buf[..want].fill(0);
                if !reader.seek(position + want as u64) {
                    return Err(std::io::ErrorKind::InvalidInput.into());
                }
                Ok(want)
            }
            None => Ok(0),
        }
    }
}

/// Represents an error for [`File::new()`].
#[derive(Debug, Error)]
pub enum NewError {
    #[error("cannot create a clusters reader for allocation {0}:{1}")]
    CreateClustersReaderFailed(usize, u64, #[source] crate::cluster::NewError),
}
