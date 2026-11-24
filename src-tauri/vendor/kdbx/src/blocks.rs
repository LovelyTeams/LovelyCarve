//! Payload block streams: KDBX 3 SHA-256 hashed blocks and KDBX 4 HMAC-SHA-256
//! blocks.

use cipher::generic_array::GenericArray;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

use crate::bytes::Cursor;
use crate::error::{Error, Result};

type HmacSha256 = Hmac<Sha256>;

/// The block index of the KDBX 4 header HMAC.
pub(crate) const HEADER_BLOCK: u64 = u64::MAX;

/// The HMAC key of block `index`: SHA-512 of the index and the database HMAC
/// key.
fn block_key(index: u64, hmac_key: &[u8; 64]) -> Zeroizing<[u8; 64]> {
    let mut key = Zeroizing::new([0u8; 64]);
    key.copy_from_slice(
        &Sha512::new()
            .chain_update(index.to_le_bytes())
            .chain_update(hmac_key)
            .finalize(),
    );
    key
}

/// Whether HMAC-SHA-256 of `parts` under block `index`'s key equals `stored`,
/// compared in constant time.
pub(crate) fn hmac_matches(
    index: u64,
    hmac_key: &[u8; 64],
    parts: &[&[u8]],
    stored: &[u8; 32],
) -> bool {
    let key = block_key(index, hmac_key);
    let mut mac = HmacSha256::new(GenericArray::from_slice(&key[..]));
    for part in parts {
        mac.update(part);
    }
    mac.verify_slice(stored).is_ok()
}

/// Read a KDBX 3 hashed block stream. Returns the joined block data and the
/// count of bytes left after the final block.
pub(crate) fn read_hashed_blocks(data: &[u8], limit: usize) -> Result<(Zeroizing<Vec<u8>>, usize)> {
    let mut cursor = Cursor::new(data);
    let mut out = Zeroizing::new(Vec::new());
    let mut index: u32 = 0;
    loop {
        let stored_index = cursor.u32("hashed block")?;
        if stored_index != index {
            return Err(Error::Malformed(format!(
                "hashed block {stored_index} where {index} was expected"
            )));
        }
        let hash = cursor.bytes32("hashed block")?;
        let size = cursor.length_i32("hashed block")?;
        if size == 0 {
            if hash != [0; 32] {
                return Err(Error::BlockChecksum {
                    index: u64::from(index),
                });
            }
            break;
        }
        let block = cursor.take(size, "hashed block")?;
        if <[u8; 32]>::from(Sha256::digest(block)) != hash {
            return Err(Error::BlockChecksum {
                index: u64::from(index),
            });
        }
        push(&mut out, block, limit)?;
        index = index
            .checked_add(1)
            .ok_or_else(|| Error::Malformed("hashed block count".into()))?;
    }
    Ok((out, cursor.remaining().len()))
}

/// Read a KDBX 4 HMAC block stream, checking every block's HMAC. Returns the
/// joined (still encrypted) block data and the count of bytes left after the
/// final block.
pub(crate) fn read_hmac_blocks(
    data: &[u8],
    hmac_key: &[u8; 64],
    limit: usize,
) -> Result<(Zeroizing<Vec<u8>>, usize)> {
    let mut cursor = Cursor::new(data);
    let mut out = Zeroizing::new(Vec::new());
    let mut index: u64 = 0;
    loop {
        let stored = cursor.bytes32("HMAC block")?;
        let size_field = cursor.i32("HMAC block")?;
        let size = usize::try_from(size_field)
            .map_err(|_| Error::Malformed("HMAC block: negative length".into()))?;
        let block = cursor.take(size, "HMAC block")?;
        let parts: [&[u8]; 3] = [&index.to_le_bytes(), &size_field.to_le_bytes(), block];
        if !hmac_matches(index, hmac_key, &parts, &stored) {
            return Err(Error::BlockChecksum { index });
        }
        if size == 0 {
            break;
        }
        push(&mut out, block, limit)?;
        index += 1;
    }
    Ok((out, cursor.remaining().len()))
}

fn push(out: &mut Vec<u8>, block: &[u8], limit: usize) -> Result<()> {
    if out.len().saturating_add(block.len()) > limit {
        return Err(Error::Limit(format!("payload larger than {limit} bytes")));
    }
    out.extend_from_slice(block);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hashed_block(index: u32, data: &[u8]) -> Vec<u8> {
        let mut out = index.to_le_bytes().to_vec();
        if data.is_empty() {
            out.extend([0; 32]);
        } else {
            out.extend(Sha256::digest(data));
        }
        out.extend((data.len() as i32).to_le_bytes());
        out.extend(data);
        out
    }

    #[test]
    fn hashed_blocks_join_and_check() {
        let mut stream = hashed_block(0, b"hello ");
        stream.extend(hashed_block(1, b"world"));
        stream.extend(hashed_block(2, b""));
        stream.extend([0xaa; 3]);
        let (data, trailing) = read_hashed_blocks(&stream, 1 << 20).unwrap();
        assert_eq!(&data[..], b"hello world");
        assert_eq!(trailing, 3);

        let mut flipped = stream.clone();
        flipped[40] ^= 1;
        assert!(matches!(
            read_hashed_blocks(&flipped, 1 << 20),
            Err(Error::BlockChecksum { index: 0 })
        ));
        let mut reordered = hashed_block(1, b"x");
        reordered.extend(hashed_block(2, b""));
        assert!(matches!(
            read_hashed_blocks(&reordered, 1 << 20),
            Err(Error::Malformed(_))
        ));
        assert!(matches!(
            read_hashed_blocks(&stream, 4),
            Err(Error::Limit(_))
        ));
    }

    #[test]
    fn hmac_blocks_check_index_and_length() {
        let key = [7u8; 64];
        let block = |index: u64, data: &[u8]| {
            let size = data.len() as i32;
            let block_key = block_key(index, &key);
            let mut mac = HmacSha256::new(GenericArray::from_slice(&block_key[..]));
            mac.update(&index.to_le_bytes());
            mac.update(&size.to_le_bytes());
            mac.update(data);
            let mut out = mac.finalize().into_bytes().to_vec();
            out.extend(size.to_le_bytes());
            out.extend(data);
            out
        };
        let mut stream = block(0, b"cipher");
        stream.extend(block(1, b"text"));
        stream.extend(block(2, b""));
        let (data, trailing) = read_hmac_blocks(&stream, &key, 1 << 20).unwrap();
        assert_eq!(&data[..], b"ciphertext");
        assert_eq!(trailing, 0);

        // A block moved to another position fails its HMAC.
        let mut swapped = block(1, b"text");
        swapped.extend(block(0, b"cipher"));
        swapped.extend(block(2, b""));
        assert!(matches!(
            read_hmac_blocks(&swapped, &key, 1 << 20),
            Err(Error::BlockChecksum { index: 0 })
        ));
    }
}
