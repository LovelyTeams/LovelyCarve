//! One logical reader for ordinary, LZNT1 and WOF streams in every read path.
use super::*;
use super::{
    data_stream::DataStream,
    reparse::{read_reparse, WOF_TAG},
    wof::WofStream,
};
use ntfs::NtfsAttribute;

pub(super) enum LogicalStream<'n, 'f> {
    Ordinary(Box<DataStream<'n, 'f>>),
    Wof(Box<WofStream<'n, 'f>>),
}

impl<'n, 'f> LogicalStream<'n, 'f> {
    pub(super) fn new<T: Read + Seek>(
        attribute: &NtfsAttribute<'n, 'f>,
        file: &'f ntfs::NtfsFile<'n>,
        fs: &mut T,
        cluster_size: u32,
        record: u64,
    ) -> Result<Self> {
        reject_unreadable_attribute(attribute.flags(), record)?;
        if attribute.name()?.is_empty() {
            if let Some(point) = read_reparse(file, fs)? {
                if point.tag == WOF_TAG {
                    return WofStream::new(
                        file,
                        fs,
                        point,
                        attribute.value_length(),
                        cluster_size,
                        record,
                    )
                    .map(Box::new)
                    .map(Self::Wof);
                }
            }
        }
        DataStream::new(attribute, fs, cluster_size, record)
            .map(Box::new)
            .map(Self::Ordinary)
    }

    pub(super) fn len(&self) -> u64 {
        match self {
            Self::Ordinary(value) => value.len(),
            Self::Wof(value) => value.len(),
        }
    }

    pub(super) fn seek<T: Read + Seek>(&mut self, fs: &mut T, position: SeekFrom) -> Result<u64> {
        match self {
            Self::Ordinary(value) => value.seek(fs, position),
            Self::Wof(value) => value.seek(position),
        }
    }

    pub(super) fn read<T: Read + Seek>(&mut self, fs: &mut T, buffer: &mut [u8]) -> Result<usize> {
        match self {
            Self::Ordinary(value) => value.read(fs, buffer),
            Self::Wof(value) => value.read(fs, buffer),
        }
    }
}
