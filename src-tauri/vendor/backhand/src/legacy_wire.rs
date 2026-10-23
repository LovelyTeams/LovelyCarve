//! SquashFS 1.0 / 2.0 / 2.1 wire facts, bounded source and metadata access.
use super::{Result, invalid};
use crate::BufReadSeek;
use std::collections::BTreeMap;
use std::io::SeekFrom;

pub(super) fn require(condition: bool, message: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(invalid(message))
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Order {
    Little,
    Big,
}
impl Order {
    pub fn bits(self, bytes: &[u8], offset: usize, count: usize) -> Result<u64> {
        require(
            count <= 64
                && offset
                    .checked_add(count)
                    .is_some_and(|end| end <= bytes.len().saturating_mul(8)),
            "truncated bit field",
        )?;
        let mut value = 0;
        for index in 0..count {
            let bit = offset + index;
            match self {
                Self::Little => value |= u64::from((bytes[bit / 8] >> (bit % 8)) & 1) << index,
                Self::Big => {
                    value = (value << 1) | u64::from((bytes[bit / 8] >> (7 - bit % 8)) & 1)
                }
            }
        }
        Ok(value)
    }
    pub fn u16(self, bytes: &[u8], offset: usize) -> Result<u16> {
        Ok(self.bits(bytes, offset * 8, 16)? as u16)
    }
    pub fn u32(self, bytes: &[u8], offset: usize) -> Result<u32> {
        Ok(self.bits(bytes, offset * 8, 32)? as u32)
    }
}

pub(super) fn read_at(
    reader: &mut dyn BufReadSeek,
    offset: u64,
    length: usize,
    limit: u64,
) -> Result<Vec<u8>> {
    require(
        offset
            .checked_add(length as u64)
            .is_some_and(|end| end <= limit),
        "read exceeds declared source",
    )?;
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn inflate(bytes: &[u8], maximum: usize) -> Result<Vec<u8>> {
    let mut decoded = vec![0; maximum + 1];
    let mut decoder = flate2::Decompress::new(true);
    let status = decoder
        .decompress(bytes, &mut decoded, flate2::FlushDecompress::Finish)
        .map_err(|error| invalid(&format!("zlib: {error}")))?;
    require(
        status == flate2::Status::StreamEnd
            && decoder.total_in() == bytes.len() as u64
            && decoder.total_out() <= maximum as u64,
        "zlib length, end marker or trailing bytes",
    )?;
    decoded.truncate(decoder.total_out() as usize);
    Ok(decoded)
}

pub(super) fn decode(bytes: &[u8], maximum: usize) -> Result<Vec<u8>> {
    // A valid zlib header commits to zlib: CRC/EOF failures never try another codec.
    if bytes.len() >= 2
        && bytes[0] & 0x0f == 8
        && (u16::from(bytes[0]) * 256 + u16::from(bytes[1])) % 31 == 0
    {
        return inflate(bytes, maximum);
    }
    require(
        bytes.len() >= 5 && bytes[0] < 225,
        "unsupported compressed block header",
    )?;
    let dictionary = u32::from_le_bytes(
        bytes[1..5]
            .try_into()
            .map_err(|_| invalid("LZMA properties"))?,
    );
    require(
        dictionary <= 8 * 1024 * 1024,
        "LZMA dictionary exceeds limit",
    )?;
    let mut output = Vec::new();
    crate::lzma::decompress_standard(bytes, &mut output, maximum)?;
    Ok(output)
}

/// Some v2 property-header streams omit EOS; the inode supplies the exact size.
/// Both Rust decoder interfaces must agree and consume every real source byte.
pub(super) fn decode_data(bytes: &[u8], maximum: usize, known: usize) -> Result<Vec<u8>> {
    match decode(bytes, maximum) {
        Ok(output) => return Ok(output),
        Err(error)
            if bytes.len() < 5
                || bytes[0] >= 225
                || (bytes[0] & 0x0f == 8
                    && (u16::from(bytes[0]) * 256 + u16::from(bytes[1])) % 31 == 0) =>
        {
            return Err(error);
        }
        Err(_) => {}
    }
    use std::io::Read;
    struct Checked<'a> {
        remaining: &'a [u8],
        past_end: bool,
    }
    impl Read for Checked<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = output.len().min(self.remaining.len());
            if count == 0 && !output.is_empty() {
                self.past_end = true;
            }
            output[..count].copy_from_slice(&self.remaining[..count]);
            self.remaining = &self.remaining[count..];
            Ok(count)
        }
    }
    require(known > 0 && known <= maximum, "invalid known LZMA length")?;
    let props = u32::from(bytes[0]);
    let lc = props % 9;
    let lp = (props / 9) % 5;
    let pb = props / 45;
    let dictionary = u32::from_le_bytes(
        bytes[1..5]
            .try_into()
            .map_err(|_| invalid("LZMA dictionary"))?,
    );
    require(
        dictionary <= 8 * 1024 * 1024 && lc + lp <= 4,
        "unsupported LZMA properties",
    )?;
    let dictionary = dictionary.max(4096).min(maximum.max(4096) as u32);
    let mut decoder = lzma_rust2::LzmaStream::new(known as u64, lc, lp, pb, dictionary, None)?;
    let mut output = vec![0; known + 1];
    let result = decoder.process(&bytes[5..], &mut output, lzma_rust2::Action::Finish)?;
    require(
        result.status == lzma_rust2::Status::StreamEnd
            && result.bytes_produced == known
            && result.bytes_consumed == bytes.len() - 5
            && decoder.unused_input().is_empty(),
        "known-size LZMA input/output extent",
    )?;
    output.truncate(known);
    let mut pull = lzma_rust2::LzmaReader::new(
        Checked {
            remaining: &bytes[5..],
            past_end: false,
        },
        known as u64,
        lc,
        lp,
        pb,
        dictionary,
        None,
    )?;
    let mut check = vec![0; known + 1];
    let mut written = 0;
    loop {
        let count = pull.read(&mut check[written..])?;
        if count == 0 {
            break;
        }
        written += count;
        require(written <= known, "known-size LZMA output overflow")?;
    }
    let source = pull.into_inner();
    require(
        written == known
            && source.remaining.is_empty()
            && !source.past_end
            && check[..known] == output,
        "known-size LZMA source EOF or decoder disagreement",
    )?;
    // Known-size lzma-rust2 interfaces do not expose/check the final range code.
    // The checked lzma-rs interface requires code==0 plus genuine EOF even at
    // the declared size. A truncated EOS stream cannot masquerade as sized.
    use lzma_rs::decompress::raw::{LzmaDecoder, LzmaParams, LzmaProperties};
    struct Bounded {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.bytes.len() + bytes.len() > self.limit {
                return Err(std::io::Error::other("LZMA verifier output overflow"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let params = LzmaParams::new(
        LzmaProperties { lc, lp, pb },
        dictionary,
        Some(known as u64),
    );
    let mut validator = LzmaDecoder::new(params, Some(dictionary as usize))
        .map_err(|error| invalid(&format!("LZMA verifier: {error}")))?;
    let mut input = std::io::Cursor::new(&bytes[5..]);
    let mut verified = Bounded {
        bytes: Vec::with_capacity(known),
        limit: known,
    };
    validator
        .decompress_checked(&mut input, &mut verified)
        .map_err(|error| invalid(&format!("LZMA final state: {error}")))?;
    require(
        input.position() == (bytes.len() - 5) as u64 && verified.bytes == output,
        "LZMA verifier final extent/bytes",
    )?;
    Ok(output)
}

pub(super) struct Header {
    pub order: Order,
    pub version: u16,
    pub minor: u16,
    pub count: u32,
    pub length: u64,
    pub size: usize,
    pub check_data: bool,
    pub root: u64,
    pub inode_start: u64,
    pub directory_start: u64,
    pub uid_start: u64,
    pub fragment_start: u64,
    pub fragments: u32,
    pub uids: Vec<u32>,
    pub gids: Vec<u32>,
    pub header_size: u64,
}
impl Header {
    pub fn read(reader: &mut dyn BufReadSeek) -> Result<Self> {
        let source_length = reader.seek(SeekFrom::End(0))?;
        let bytes = read_at(reader, 0, 51, source_length)?;
        let order = match bytes.get(..4) {
            Some(b"hsqs") => Order::Little,
            Some(b"sqsh") => Order::Big,
            _ => return Err(invalid("magic")),
        };
        let version = order.u16(&bytes, 28)?;
        let minor = order.u16(&bytes, 30)?;
        require(
            matches!((version, minor), (1, 0) | (2, 0..=1)),
            "unsupported legacy version",
        )?;
        let bytes = if version == 2 {
            read_at(reader, 0, 63, source_length)?
        } else {
            bytes
        };
        let header_size = bytes.len() as u64;
        let length = u64::from(order.u32(&bytes, 8)?);
        let count = order.u32(&bytes, 4)?;
        require(
            length <= source_length && length >= header_size && (1..=1_000_000).contains(&count),
            "source length or inode count",
        )?;
        let size = if version == 1 {
            u32::from(order.u16(&bytes, 32)?)
        } else {
            order.u32(&bytes, 51)?
        };
        require(
            size.is_power_of_two()
                && (4096..=if version == 1 { 32768 } else { 65536 }).contains(&size)
                && u32::from(order.u16(&bytes, 34)?) == size.trailing_zeros(),
            "block size/log",
        )?;
        let inode_start = u64::from(order.u32(&bytes, 20)?);
        let directory_start = u64::from(order.u32(&bytes, 24)?);
        let uid_start = u64::from(order.u32(&bytes, 12)?);
        let gid_start = u64::from(order.u32(&bytes, 16)?);
        let uid_count = bytes[37] as usize;
        let gid_count = bytes[38] as usize;
        require(
            uid_count > 0 && (version != 1 || (uid_count <= 48 && gid_count <= 15)),
            "ID table count",
        )?;
        require(
            header_size <= inode_start
                && inode_start < directory_start
                && directory_start <= uid_start
                && uid_start.checked_add((uid_count * 4) as u64)
                    == Some(if gid_count == 0 { length } else { gid_start }),
            "table ordering/UID extent",
        )?;
        require(
            if gid_count == 0 {
                gid_start == 0
            } else {
                gid_start.checked_add((gid_count * 4) as u64) == Some(length)
            },
            "GID extent",
        )?;
        let ids = read_at(reader, uid_start, uid_count * 4, length)?;
        let uids = ids
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| order.u32(bytes, 0))
            .collect::<Result<Vec<_>>>()?;
        let ids = if gid_count == 0 {
            Vec::new()
        } else {
            read_at(reader, gid_start, gid_count * 4, length)?
        };
        let gids = ids
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| order.u32(bytes, 0))
            .collect::<Result<Vec<_>>>()?;
        let fragments = if version == 2 {
            order.u32(&bytes, 55)?
        } else {
            0
        };
        let fragment_start = if version == 2 {
            u64::from(order.u32(&bytes, 59)?)
        } else {
            uid_start
        };
        require(
            fragments <= count
                && fragment_start <= uid_start
                && (fragments != 0 || fragment_start == uid_start),
            "fragment index extent/count",
        )?;
        Ok(Self {
            order,
            version,
            minor,
            count,
            length,
            size: size as usize,
            check_data: bytes[36] & 4 != 0,
            root: order.bits(&bytes, 43 * 8, 64)?,
            inode_start,
            directory_start,
            uid_start,
            fragment_start,
            fragments,
            uids,
            gids,
            header_size,
        })
    }
}

pub(super) struct Metadata {
    pub bytes: Vec<u8>,
    pub blocks: BTreeMap<u64, (usize, usize)>,
}
impl Metadata {
    pub fn read(
        reader: &mut dyn BufReadSeek,
        start: u64,
        end: u64,
        header: &Header,
        budget: &mut usize,
    ) -> Result<Self> {
        require(start <= end && end <= header.length, "metadata table range")?;
        let mut bytes = Vec::new();
        let mut blocks = BTreeMap::new();
        let mut position = start;
        while position < end {
            require(blocks.len() < 1_000_000, "metadata block count exceeded")?;
            let (decoded, next) = metadata_block(reader, position, end, header)?;
            *budget = budget
                .checked_add(decoded.len())
                .ok_or_else(|| invalid("metadata budget overflow"))?;
            require(*budget <= 512 * 1024 * 1024, "metadata budget exceeded")?;
            blocks.insert(position - start, (bytes.len(), decoded.len()));
            bytes.extend(decoded);
            position = next;
        }
        Ok(Self { bytes, blocks })
    }
    pub fn offset(&self, reference: u64) -> Result<usize> {
        let (start, length) = self
            .blocks
            .get(&(reference >> 16))
            .ok_or_else(|| invalid("unknown metadata block reference"))?;
        let within = (reference & 0xffff) as usize;
        require(within < *length, "metadata offset outside decoded block")?;
        Ok(start + within)
    }
}
pub(super) fn metadata_block(
    reader: &mut dyn BufReadSeek,
    offset: u64,
    limit: u64,
    header: &Header,
) -> Result<(Vec<u8>, u64)> {
    let prefix = if header.check_data { 3 } else { 2 };
    let bytes = read_at(reader, offset, prefix, limit)?;
    if header.check_data {
        require(bytes[2] == 0xff, "metadata check marker")?;
    }
    let size = header.order.u16(&bytes, 0)?;
    let count = (size & 0x7fff) as usize;
    require((1..=8192).contains(&count), "metadata block length")?;
    let bytes = read_at(reader, offset + prefix as u64, count, limit)?;
    let decoded = if size & 0x8000 != 0 {
        bytes
    } else {
        decode(&bytes, 8192)?
    };
    require(!decoded.is_empty(), "empty metadata block")?;
    Ok((decoded, offset + prefix as u64 + count as u64))
}
