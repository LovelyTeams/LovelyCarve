use crate::{check_signature, input::Input, invalid, Member};
use crc32fast::Hasher;
use std::io::{self, Read};

// Each header byte contributes once to both the bound and FHCRC. The bound
// includes fixed fields, XLEN, string terminators and the checksum itself.
struct HeaderReader<'a, R> {
    input: &'a mut Input<R>,
    bytes: usize,
    checksum: Hasher,
    limit: usize,
}

impl<R: Read> HeaderReader<'_, R> {
    fn exact(&mut self, output: &mut [u8]) -> io::Result<()> {
        let next = self
            .bytes
            .checked_add(output.len())
            .ok_or_else(|| invalid("GZIP header length overflow"))?;
        if next > self.limit {
            return Err(invalid("GZIP header or metadata byte limit exceeded"));
        }
        self.input.exact(output)?;
        self.checksum.update(output);
        self.bytes = next;
        Ok(())
    }

    fn string(&mut self) -> io::Result<Vec<u8>> {
        let mut value = Vec::new();
        loop {
            let mut byte = [0];
            self.exact(&mut byte)?;
            if byte[0] == 0 {
                return Ok(value);
            }
            value.try_reserve(1).map_err(io::Error::other)?;
            value.push(byte[0]);
        }
    }
}

pub(crate) fn read<R: Read>(
    input: &mut Input<R>,
    logical_offset: u64,
    metadata_remaining: usize,
) -> io::Result<(Member, usize)> {
    let header_offset = input.offset;
    let limit = input.options.max_header_bytes.min(metadata_remaining);
    let mut reader = HeaderReader {
        input,
        bytes: 0,
        checksum: Hasher::new(),
        limit,
    };
    let mut fixed = [0; 10];
    reader.exact(&mut fixed)?;
    if !check_signature(&fixed) {
        return Err(invalid(format!(
            "invalid GZIP signature at source offset {header_offset}"
        )));
    }
    if fixed[2] != 8 {
        return Err(invalid(format!(
            "unsupported GZIP compression method {}",
            fixed[2]
        )));
    }
    if fixed[3] & 0xe0 != 0 {
        return Err(invalid("reserved GZIP header flags are set"));
    }
    let flags = fixed[3];
    let extra = if flags & 4 != 0 {
        let mut length = [0; 2];
        reader.exact(&mut length)?;
        let length = usize::from(u16::from_le_bytes(length));
        if length > reader.limit.saturating_sub(reader.bytes) {
            return Err(invalid("GZIP extra field exceeds header or metadata limit"));
        }
        let mut bytes = vec![0; length];
        reader.exact(&mut bytes)?;
        Some(bytes)
    } else {
        None
    };
    let name = if flags & 8 != 0 {
        Some(reader.string()?)
    } else {
        None
    };
    let comment = if flags & 16 != 0 {
        Some(reader.string()?)
    } else {
        None
    };
    let header_crc16 = if flags & 2 != 0 {
        let calculated = reader.checksum.clone().finalize() as u16;
        let mut bytes = [0; 2];
        reader.exact(&mut bytes)?;
        let stored = u16::from_le_bytes(bytes);
        if stored != calculated {
            return Err(invalid(format!(
                "GZIP header CRC mismatch at source offset {header_offset}"
            )));
        }
        Some(stored)
    } else {
        None
    };
    let bytes = reader.bytes;
    Ok((
        Member {
            header_offset,
            compressed_data_offset: reader.input.offset,
            compressed_data_size: 0,
            trailer_offset: 0,
            end_offset: 0,
            uncompressed_offset: logical_offset,
            uncompressed_size: 0,
            flags,
            modification_time: u32::from_le_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]),
            extra_flags: fixed[8],
            operating_system: fixed[9],
            extra,
            name,
            comment,
            header_crc16,
            crc32: 0,
            isize: 0,
        },
        bytes,
    ))
}
