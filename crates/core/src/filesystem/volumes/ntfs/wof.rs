//! Windows Overlay Filter file-provider streams, with bounded random-access chunks.
use super::*;
use super::{data_stream::DataStream, reparse::ReparsePoint};
use anyhow::{ensure, Context};
use ntfs::NtfsAttributeItem;

pub(super) struct WofStream<'n, 'f> {
    item: NtfsAttributeItem<'n, 'f>,
    length: u64,
    stored_length: u64,
    position: u64,
    algorithm: u32,
    chunk_size: usize,
    chunks: u64,
    offset_width: usize,
    payload_start: u64,
    cluster_size: u32,
    record: u64,
    cached: Option<u64>,
    decoded: Vec<u8>,
}

impl<'n, 'f> WofStream<'n, 'f> {
    pub(super) fn new<T: Read + Seek>(
        file: &'f ntfs::NtfsFile<'n>,
        fs: &mut T,
        point: ReparsePoint,
        length: u64,
        cluster_size: u32,
        record: u64,
    ) -> Result<Self> {
        ensure!(
            point.data.len() == 16,
            "invalid WOF file-provider metadata length"
        );
        let words: Vec<_> = point
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect();
        ensure!(
            words[0] == 1 && words[2] == 1,
            "unsupported WOF metadata version"
        );
        ensure!(
            words[1] == 2,
            "WOF provider {} requires external WIM resources; logical data unavailable",
            words[1]
        );
        let algorithm = words[3];
        let chunk_size = match algorithm {
            0 => 4096,
            1 => 32768,
            2 => 8192,
            3 => 16384,
            _ => anyhow::bail!("unsupported WOF compression algorithm {algorithm}"),
        };
        let item = file
            .data(fs, "WofCompressedData")
            .context("WOF file is missing WofCompressedData")??;
        let attribute = item.to_attribute()?;
        reject_unreadable_attribute(attribute.flags(), record)?;
        ensure!(
            !attribute.flags().contains(NtfsAttributeFlags::COMPRESSED),
            "nested NTFS/WOF compression is invalid"
        );
        let stored_length = attribute.value_length();
        let offset_width = if stored_length > u64::from(u32::MAX) {
            8
        } else {
            4
        };
        let chunks = length.div_ceil(chunk_size as u64);
        let payload_start = chunks
            .saturating_sub(1)
            .checked_mul(offset_width as u64)
            .context("WOF offset table overflow")?;
        ensure!(payload_start <= stored_length, "truncated WOF offset table");
        ensure!(
            length != 0 || stored_length == 0,
            "nonempty WOF payload for empty file"
        );
        Ok(Self {
            item,
            length,
            stored_length,
            position: 0,
            algorithm,
            chunk_size,
            chunks,
            offset_width,
            payload_start,
            cluster_size,
            record,
            cached: None,
            decoded: Vec::new(),
        })
    }

    pub(super) fn len(&self) -> u64 {
        self.length
    }

    pub(super) fn seek(&mut self, position: SeekFrom) -> Result<u64> {
        let SeekFrom::Start(offset) = position else {
            anyhow::bail!("WOF requires an absolute seek");
        };
        self.position = offset;
        Ok(offset)
    }

    pub(super) fn read<T: Read + Seek>(&mut self, fs: &mut T, buffer: &mut [u8]) -> Result<usize> {
        let wanted = self
            .length
            .saturating_sub(self.position)
            .min(buffer.len() as u64) as usize;
        let mut filled = 0;
        while filled < wanted {
            let chunk = self.position / self.chunk_size as u64;
            if self.cached != Some(chunk) {
                self.load_chunk(fs, chunk)?;
            }
            let inside = (self.position % self.chunk_size as u64) as usize;
            let count = (wanted - filled).min(self.decoded.len() - inside);
            buffer[filled..filled + count].copy_from_slice(&self.decoded[inside..inside + count]);
            self.position += count as u64;
            filled += count;
        }
        Ok(filled)
    }

    fn load_chunk<T: Read + Seek>(&mut self, fs: &mut T, chunk: u64) -> Result<()> {
        self.cached = None;
        let attribute = self.item.to_attribute()?;
        let mut value = DataStream::new(&attribute, fs, self.cluster_size, self.record)?;
        let mut offsets = [0u64; 2];
        for (index, boundary) in [chunk, chunk + 1].into_iter().enumerate() {
            offsets[index] = if boundary == 0 {
                0
            } else if boundary == self.chunks {
                self.stored_length - self.payload_start
            } else {
                value.seek(
                    fs,
                    SeekFrom::Start((boundary - 1) * self.offset_width as u64),
                )?;
                let mut bytes = [0u8; 8];
                let mut filled = 0;
                while filled < self.offset_width {
                    let read = value.read(fs, &mut bytes[filled..self.offset_width])?;
                    ensure!(read > 0, "truncated WOF chunk offset");
                    filled += read;
                }
                u64::from_le_bytes(bytes)
            };
        }
        ensure!(
            offsets[0] < offsets[1] && offsets[1] <= self.stored_length - self.payload_start,
            "invalid WOF chunk offsets"
        );
        let expected =
            (self.length - chunk * self.chunk_size as u64).min(self.chunk_size as u64) as usize;
        let stored = usize::try_from(offsets[1] - offsets[0])?;
        ensure!(
            stored <= expected,
            "WOF compressed chunk exceeds logical chunk size"
        );
        let mut bytes = vec![0; stored];
        value.seek(fs, SeekFrom::Start(self.payload_start + offsets[0]))?;
        let mut filled = 0;
        while filled < bytes.len() {
            let read = value.read(fs, &mut bytes[filled..])?;
            ensure!(read > 0, "truncated WOF compressed chunk");
            filled += read;
        }
        self.decoded.resize(expected, 0);
        if stored == expected {
            self.decoded.copy_from_slice(&bytes);
        } else if self.algorithm == 1 {
            ms_compress::lzx::decompress_lzx(&bytes, &mut self.decoded, self.chunk_size)
                .context("WOF LZX decode")?;
        } else {
            let produced = ms_compress::decompress_xpress_huffman_buffer(&bytes, &mut self.decoded)
                .context("WOF XPRESS-Huffman decode")?;
            ensure!(produced == expected, "short WOF XPRESS-Huffman output");
        }
        self.cached = Some(chunk);
        Ok(())
    }
}
