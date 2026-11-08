//! Apple `decmpfs` transparent filesystem compression.
//!
//! macOS stores a compressed file's bytes in the `com.apple.decmpfs` extended
//! attribute, or in the file's resource fork, and leaves the data fork empty.
//! A reader that walks only the data fork returns nothing for such a file
//! while `stat` still reports the full logical size — most files on a macOS
//! system volume are stored this way.
//!
//! This crate turns that attribute, plus the resource fork when the attribute
//! points at one, back into file contents. It is filesystem agnostic: callers
//! such as `hfsplus` and `apfs` supply the bytes.
//!
//! ```
//! # fn main() -> Result<(), cmpfs::CmpfsError> {
//! let xattr = /* contents of com.apple.decmpfs */
//! #     { let mut v = vec![0x66, 0x70, 0x6d, 0x63, 1, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0];
//! #       v.extend_from_slice(b"hello"); v };
//! let header = cmpfs::Header::parse(&xattr)?;
//! if header.storage() == cmpfs::Storage::Xattr {
//!     assert_eq!(cmpfs::decompress(&xattr, None)?, b"hello");
//! }
//! # Ok(()) }
//! ```
//!
//! # Format
//!
//! The attribute opens with a 16-byte little-endian header — magic, type,
//! uncompressed size — described in `bsd/sys/decmpfs.h` in xnu. Only type 1 is
//! named there; types 3 and above live in Apple's closed AppleFSCompression,
//! so the layouts below are taken from two independent readers, `libarchive`
//! and `apfs-fuse`. Where only one covers a case it is marked provisional in
//! the source. See `docs/FORMATS.md`.

mod error;

pub use error::{CmpfsError, Result};

use std::io::Read;

/// Extended attribute holding the compression header.
pub const XATTR_NAME: &str = "com.apple.decmpfs";

/// Extended attribute holding the resource fork, on filesystems that store it
/// as one. HFS+ has a real resource fork instead.
pub const RESOURCE_FORK_XATTR_NAME: &str = "com.apple.ResourceFork";

/// `cmpf`, little-endian, at the start of the attribute.
pub const MAGIC: u32 = 0x636d_7066;

/// Size of the attribute header preceding any inline payload.
pub const HEADER_SIZE: usize = 16;

/// Uncompressed size of one compression block.
pub const BLOCK_SIZE: usize = 0x1_0000;

/// What an extended attribute is, once transparent compression is accounted
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XattrKind {
    /// An ordinary attribute — a quarantine tag, Finder info, a code
    /// signature. Part of the file as the user sees it.
    User,
    /// Machinery for transparent compression. macOS hides these from
    /// userspace; a reader that resolves compression has already consumed
    /// them, and writing one onto an extracted file makes that file
    /// unreadable on macOS, because the attribute declares a data fork that
    /// is no longer empty.
    Compression,
}

/// Classify an extended attribute by name.
///
/// [`XATTR_NAME`] is always machinery. [`RESOURCE_FORK_XATTR_NAME`] is only
/// machinery on a compressed file: on an uncompressed one it is user data — an
/// icon, a classic resource map — so a name alone cannot decide. Darwin draws
/// the same line in `decmpfs_hides_xattr` (xnu `bsd/kern/decmpfs.c`), which
/// returns 0 for the resource fork when
/// `!decmpfs_fast_file_is_compressed(cp)`.
pub fn classify_xattr(name: &str, file_is_compressed: bool) -> XattrKind {
    let is_machinery =
        name == XATTR_NAME || (file_is_compressed && name == RESOURCE_FORK_XATTR_NAME);
    if is_machinery {
        XattrKind::Compression
    } else {
        XattrKind::User
    }
}

/// Where a compression type keeps its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// Immediately after the header, in the `com.apple.decmpfs` attribute.
    Xattr,
    /// In the file's resource fork.
    ResourceFork,
    /// Nowhere on this volume: the file is a placeholder for content stored
    /// remotely.
    Dataless,
    /// Type not recognised. Reported rather than guessed, since guessing wrong
    /// means reading an unrelated fork.
    Unknown,
}

/// The `com.apple.decmpfs` attribute header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Compression type. See [`Storage`] for the ones this crate places.
    pub compression_type: u32,
    /// Logical size of the file once decompressed.
    pub uncompressed_size: u64,
}

impl Header {
    /// Parse the header from the start of a `com.apple.decmpfs` attribute.
    pub fn parse(xattr: &[u8]) -> Result<Self> {
        let magic = le_u32(xattr, 0)?;
        if magic != MAGIC {
            return Err(CmpfsError::InvalidMagic(magic));
        }
        Ok(Header {
            compression_type: le_u32(xattr, 4)?,
            uncompressed_size: le_u64(xattr, 8)?,
        })
    }

    /// Where this type's payload lives.
    ///
    /// Call this before fetching a resource fork, so the fork is read only
    /// when it is needed.
    pub fn storage(&self) -> Storage {
        match self.compression_type {
            1 | 3 | 7 | 9 | 11 | 13 => Storage::Xattr,
            4 | 8 | 10 | 12 | 14 => Storage::ResourceFork,
            // DATALESS_CMPFS_TYPE and DATALESS_PKG_CMPFS_TYPE, xnu
            // `bsd/sys/decmpfs.h`.
            0x8000_0001 | 0x8000_0002 => Storage::Dataless,
            _ => Storage::Unknown,
        }
    }
}

/// Decompress a file from its `com.apple.decmpfs` attribute.
///
/// `resource_fork` is required when [`Header::storage`] returns
/// [`Storage::ResourceFork`] and ignored otherwise.
///
/// Fails rather than returning a short buffer when the payload does not
/// produce exactly [`Header::uncompressed_size`] bytes: the return type cannot
/// express "complete except for a hole", so a caller could not tell the
/// difference.
pub fn decompress(xattr: &[u8], resource_fork: Option<&[u8]>) -> Result<Vec<u8>> {
    let header = Header::parse(xattr)?;
    let size = usize::try_from(header.uncompressed_size).map_err(|_| {
        CmpfsError::CorruptedData(format!(
            "uncompressed size {} exceeds this platform's address space",
            header.uncompressed_size
        ))
    })?;

    if size > 512 * 1024 * 1024 {
        return Err(CmpfsError::CorruptedData(
            "decoded file exceeds 512 MiB memory budget".into(),
        ));
    }
    let data = match header.storage() {
        Storage::Xattr => {
            let payload = xattr.get(HEADER_SIZE..).ok_or(CmpfsError::Truncated {
                need: HEADER_SIZE,
                have: xattr.len(),
            })?;
            decompress_inline(header.compression_type, payload, size)?
        }
        Storage::ResourceFork => {
            let rsrc =
                resource_fork.ok_or(CmpfsError::MissingResourceFork(header.compression_type))?;
            decompress_resource_fork(header.compression_type, rsrc, size)?
        }
        Storage::Dataless => return Err(CmpfsError::Dataless(header.compression_type)),
        Storage::Unknown => return Err(CmpfsError::Unsupported(header.compression_type)),
    };

    if data.len() != size {
        return Err(CmpfsError::CorruptedData(format!(
            "decompressed {} bytes, header declared {size}",
            data.len()
        )));
    }
    Ok(data)
}

// --- inline payloads -------------------------------------------------------

fn decompress_inline(compression_type: u32, payload: &[u8], size: usize) -> Result<Vec<u8>> {
    match compression_type {
        // "uncompressed data in xattr" — xnu `bsd/sys/decmpfs.h`, `CMP_Type1`.
        // The only type xnu names, and it carries no marker byte.
        1 => Ok(payload.to_vec()),
        3 => decode_zlib(payload, size),
        7 => decode_lzvn(payload, size),
        // PROVISIONAL: apfs-fuse alone covers type 9, and asserts the marker
        // rather than testing it. Verified here so an unexpected byte fails
        // instead of silently shifting the output by one.
        9 => strip_marker(payload, 0xCC),
        11 => decode_lzfse(payload, size),
        // LZBITMAP. No Rust decoder exists; reported rather than guessed.
        13 => Err(CmpfsError::Unsupported(compression_type)),
        other => Err(CmpfsError::Unsupported(other)),
    }
}

// --- resource fork payloads ------------------------------------------------

fn decompress_resource_fork(compression_type: u32, rsrc: &[u8], size: usize) -> Result<Vec<u8>> {
    match compression_type {
        4 => zlib_resource_fork(rsrc, size),
        8 | 10 | 12 => offset_table_resource_fork(compression_type, rsrc, size),
        14 => Err(CmpfsError::Unsupported(compression_type)),
        other => Err(CmpfsError::Unsupported(other)),
    }
}

/// Type 4: zlib blocks behind a resource-fork header.
///
/// The fork opens with a big-endian offset to its resource data, which starts
/// with its own big-endian length; the block table follows. libarchive writes
/// `0x100` there and reads the table back at a hardcoded 260
/// (`RSRC_H_SIZE`, `archive_write_disk_posix.c`); apfs-fuse follows the stored
/// offset (`Decmpfs.cpp`). Following it agrees with libarchive on every fork
/// Apple writes and tolerates a different header size.
fn zlib_resource_fork(rsrc: &[u8], size: usize) -> Result<Vec<u8>> {
    let data_offset = be_u32(rsrc, 0)? as usize;
    let table = data_offset
        .checked_add(4)
        .ok_or_else(|| CmpfsError::CorruptedData("resource data offset overflows".into()))?;

    let count = le_u32(rsrc, table)? as usize;
    let expected = block_count(size);
    if count != expected {
        return Err(CmpfsError::CorruptedData(format!(
            "resource fork declares {count} blocks, {size} bytes needs {expected}"
        )));
    }

    let mut out = Vec::with_capacity(size);
    for k in 0..count {
        let entry = table
            .checked_add(4)
            .and_then(|t| t.checked_add(k.checked_mul(8)?))
            .ok_or_else(|| CmpfsError::CorruptedData("block table overflows".into()))?;
        let offset = le_u32(rsrc, entry)? as usize;
        let length = le_u32(rsrc, entry + 4)? as usize;
        let start = table
            .checked_add(offset)
            .ok_or_else(|| CmpfsError::CorruptedData("block offset overflows".into()))?;

        let block = slice_block(rsrc, start, length)?;
        let expected = block_len(size, k);
        let decoded = decode_zlib(block, expected)?;
        if decoded.len() != expected {
            return Err(CmpfsError::CorruptedData(format!(
                "block {k} decoded {} bytes, expected {expected}",
                decoded.len()
            )));
        }
        out.extend_from_slice(&decoded);
    }
    Ok(out)
}

/// Types 8, 10 and 12: blocks delimited by a table of little-endian offsets at
/// the start of the fork, each block running to the next offset.
///
/// This layout is also used by libfsapfs; its compression.c confirms the
/// RAW/LZFSE stored markers.
fn offset_table_resource_fork(compression_type: u32, rsrc: &[u8], size: usize) -> Result<Vec<u8>> {
    let count = block_count(size);
    let table_size = count
        .checked_add(1)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(table_overflow)?;
    if rsrc.len() < table_size {
        return Err(CmpfsError::CorruptedData(
            "truncated resource-fork offset table".into(),
        ));
    }
    let mut out = Vec::with_capacity(size);

    for k in 0..count {
        let start = le_u32(rsrc, k.checked_mul(4).ok_or_else(table_overflow)?)? as usize;
        let end = le_u32(
            rsrc,
            k.checked_add(1)
                .and_then(|n| n.checked_mul(4))
                .ok_or_else(table_overflow)?,
        )? as usize;
        let length = end.checked_sub(start).ok_or_else(|| {
            CmpfsError::CorruptedData(format!("block {k} offsets run backwards: {start} > {end}"))
        })?;
        if start < table_size {
            return Err(CmpfsError::CorruptedData(
                "resource-fork payload overlaps its offset table".into(),
            ));
        }

        let block = slice_block(rsrc, start, length)?;
        let expected = block_len(size, k);
        let decoded = match compression_type {
            8 => decode_lzvn(block, expected)?,
            // libfsapfs_compression.c uses 0xcc for RAW blocks, like type 9.
            10 => strip_marker(block, 0xcc)?,
            _ => decode_lzfse(block, expected)?,
        };
        if decoded.len() != expected {
            return Err(CmpfsError::CorruptedData(format!(
                "block {k} decoded {} bytes, expected {expected}",
                decoded.len()
            )));
        }
        out.extend_from_slice(&decoded);
    }
    Ok(out)
}

// --- codecs ----------------------------------------------------------------

/// zlib, or stored bytes behind a marker.
///
/// RFC 1950 fixes the low nibble of the zlib CMF byte at 8, so a byte whose
/// low nibble is `0xf` cannot open a zlib stream. libarchive writes exactly
/// `0xff` and tests for it; apfs-fuse tests the nibble. Testing the nibble
/// accepts both.
fn decode_zlib(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    let first = *data.first().ok_or(CmpfsError::Truncated {
        need: 1,
        have: data.len(),
    })?;
    if first & 0x0f == 0x0f {
        return Ok(data.get(1..).unwrap_or_default().to_vec());
    }
    let mut out = Vec::with_capacity(expected);
    flate2::read::ZlibDecoder::new(data)
        .take(expected as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| CmpfsError::Decompression(format!("zlib: {e}")))?;
    Ok(out)
}

/// LZVN, or stored bytes behind the `0x06` marker — the LZVN end-of-stream
/// opcode, which no compressed block can begin with.
fn decode_lzvn(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    let first = *data.first().ok_or(CmpfsError::Truncated {
        need: 1,
        have: data.len(),
    })?;
    if first == 0x06 {
        return Ok(data.get(1..).unwrap_or_default().to_vec());
    }
    let mut out = vec![0u8; expected];
    let n = lzvn::decode_into(data, &mut out)
        .map_err(|e| CmpfsError::Decompression(format!("lzvn: {e}")))?;
    out.truncate(n);
    Ok(out)
}

fn decode_lzfse(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    if let Some(payload) = data.strip_prefix(&[0xff]) {
        return Ok(payload.to_vec());
    }
    let mut out = Vec::with_capacity(expected);
    let mut decoder = lzfse_rust::LzfseRingDecoder::default();
    decoder
        .reader_bytes(data)
        .take(expected as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| CmpfsError::Decompression(format!("lzfse: {e:?}")))?;
    Ok(out)
}

fn strip_marker(data: &[u8], marker: u8) -> Result<Vec<u8>> {
    let first = *data.first().ok_or(CmpfsError::Truncated {
        need: 1,
        have: data.len(),
    })?;
    if first != marker {
        return Err(CmpfsError::CorruptedData(format!(
            "expected stored-data marker {marker:#04x}, found {first:#04x}"
        )));
    }
    Ok(data.get(1..).unwrap_or_default().to_vec())
}

// --- helpers ---------------------------------------------------------------

fn block_count(size: usize) -> usize {
    size.div_ceil(BLOCK_SIZE)
}

/// Uncompressed length of block `k`, which is short only for the last one.
fn block_len(size: usize, k: usize) -> usize {
    (size - k * BLOCK_SIZE).min(BLOCK_SIZE)
}

fn table_overflow() -> CmpfsError {
    CmpfsError::CorruptedData("block offset table overflows".into())
}

/// A block's compressed bytes, rejecting a length no block can have.
///
/// Both references cap this at `BLOCK_SIZE + 1`: a stored block is one marker
/// byte plus a full block, and a compressed one is smaller still.
fn slice_block(rsrc: &[u8], start: usize, length: usize) -> Result<&[u8]> {
    if length > BLOCK_SIZE + 1 {
        return Err(CmpfsError::CorruptedData(format!(
            "block length {length} exceeds {}",
            BLOCK_SIZE + 1
        )));
    }
    let end = start
        .checked_add(length)
        .ok_or_else(|| CmpfsError::CorruptedData("block extends past address space".into()))?;
    rsrc.get(start..end)
        .ok_or(CmpfsError::TruncatedResourceFork {
            need: end,
            have: rsrc.len(),
        })
}

fn le_u32(buf: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(fixed::<4>(buf, offset)?))
}

fn le_u64(buf: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(fixed::<8>(buf, offset)?))
}

fn be_u32(buf: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(fixed::<4>(buf, offset)?))
}

fn fixed<const N: usize>(buf: &[u8], offset: usize) -> Result<[u8; N]> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| CmpfsError::CorruptedData(format!("offset {offset} overflows")))?;
    let bytes = buf.get(offset..end).ok_or(CmpfsError::Truncated {
        need: end,
        have: buf.len(),
    })?;
    bytes.try_into().map_err(|_| CmpfsError::Truncated {
        need: end,
        have: buf.len(),
    })
}

#[cfg(test)]
mod tests;
