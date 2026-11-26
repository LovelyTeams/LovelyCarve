use crate::{
    object::{Content, FilePlugins, Segment},
    require,
    tree::{Item, COMPRESSED_TAIL},
};
use flate2::{Decompress, FlushDecompress, Status};
use lamfold::{decode, Codec, FoldError, Result};
use ruzstd::{decoding::StreamingDecoder, io::Read};

pub(crate) const CCREG: u16 = 4;
const MAX_CLUSTER: u64 = 64 * 1024;
const BAD_STREAM: FoldError = FoldError::Decompress("reiser4: invalid compressed cluster");

#[derive(Clone, Copy)]
pub(crate) enum Compression {
    Lzo,
    Deflate,
    Zstd,
}

impl Compression {
    pub(crate) fn decode(self, payload: &[u8], len: usize) -> Result<Vec<u8>> {
        let (input, checksum) = payload
            .split_last_chunk::<4>()
            .ok_or(FoldError::Corrupt("reiser4: missing cluster checksum"))?;
        require(
            adler2::adler32_slice(input) == u32::from_le_bytes(*checksum),
            "reiser4: cluster checksum",
        )?;

        match self {
            Self::Lzo => {
                let output = decode(Codec::Lzo, input, len)?;
                if output.len() != len {
                    return Err(BAD_STREAM);
                }
                Ok(output)
            }
            Self::Deflate => {
                let mut output = vec![0; len];
                let mut decoder = Decompress::new(false);
                let status = decoder
                    .decompress(input, &mut output, FlushDecompress::Finish)
                    .map_err(|_| BAD_STREAM)?;
                require(
                    status == Status::StreamEnd
                        && decoder.total_in() == input.len() as u64
                        && decoder.total_out() == len as u64,
                    "reiser4: deflate cluster length",
                )?;
                Ok(output)
            }
            Self::Zstd => {
                let mut output = vec![0; len];
                let mut decoder = StreamingDecoder::new_with_max_window_size(input, MAX_CLUSTER)
                    .map_err(|_| BAD_STREAM)?;
                decoder.read_exact(&mut output).map_err(|_| BAD_STREAM)?;
                let extra = decoder.read(&mut [0]).map_err(|_| BAD_STREAM)?;
                require(
                    extra == 0 && decoder.into_inner().is_empty(),
                    "reiser4: zstd cluster length",
                )?;
                Ok(output)
            }
        }
    }
}

pub(crate) fn file_segments(
    items: Vec<Item>,
    size: u64,
    file: FilePlugins,
    root: FilePlugins,
) -> Result<Vec<Segment>> {
    let codec = match file.compression.or(root.compression).unwrap_or(0) {
        0 => Compression::Lzo,
        1 => Compression::Deflate,
        2 => Compression::Zstd,
        _ => {
            return Err(FoldError::Unsupported(
                "reiser4: cluster compression plugin",
            ))
        }
    };
    let cluster = file.cluster.or(root.cluster).unwrap_or(0);
    if cluster > 4 {
        return Err(FoldError::Unsupported("reiser4: cluster size plugin"));
    }
    let shift = 16 - cluster as u8;
    let cluster_size = 1u64 << shift;
    let mut items = items.into_iter().peekable();
    let mut covered = 0;
    let mut segments = Vec::new();

    while let Some(first) = items.peek() {
        let start = first.key.0[3];
        require(
            start >= covered && start < size && start % cluster_size == 0,
            "reiser4: invalid first cluster item offset",
        )?;
        if covered < start {
            segments.push(Segment {
                start: covered,
                end: start,
                content: Content::Hole,
            });
        }
        let len = (size - start).min(cluster_size);
        let end = start + len;
        let mut payload = Vec::new();
        // Item keys within a cluster advance by the stored byte length.
        while let Some(item) = items.next_if(|item| item.key.0[3] < end) {
            require(
                item.plugin == COMPRESSED_TAIL
                    && item.key.0[3] == start + payload.len() as u64
                    && item.bytes().first() == Some(&shift)
                    && item.bytes().len() > 1
                    && (payload.len() + item.bytes().len() - 1) as u64 <= len,
                "reiser4: invalid cluster fragment",
            )?;
            payload.extend_from_slice(&item.bytes()[1..]);
        }
        let content = if payload.len() as u64 == len {
            Content::Inline(payload)
        } else {
            require(payload.len() > 4, "reiser4: truncated compressed cluster")?;
            Content::Compressed(codec, payload)
        };
        segments.push(Segment {
            start,
            end,
            content,
        });
        covered = end;
    }
    if covered < size {
        segments.push(Segment {
            start: covered,
            end: size,
            content: Content::Hole,
        });
    }
    Ok(segments)
}
