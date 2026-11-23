//! VeraCrypt / TrueCrypt keyfiles.
//!
//! Each keyfile (its first MiB) feeds a running CRC-32 whose four bytes are
//! added into a pool, four pool bytes per input byte; the pool is then added
//! byte by byte to the password, which becomes the pool's length. The pool
//! holds 64 bytes, or 128 when the typed password is longer than 64 bytes
//! (VeraCrypt 1.24 and later). Keyfile order does not matter.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use crate::error::{Result, VeraError};

/// Bytes of a keyfile that count.
pub const KEYFILE_READ_LIMIT: usize = 1024 * 1024;
/// The longest password VeraCrypt accepts.
pub const MAX_PASSWORD_LEN: usize = 128;
const LEGACY_PASSWORD_LEN: usize = 64;

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ 0xEDB8_8320
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// The keyfile pool of one unlock attempt.
pub struct KeyfilePool {
    pool: Zeroizing<[u8; MAX_PASSWORD_LEN]>,
    size: usize,
    count: usize,
}

impl KeyfilePool {
    /// A pool for a password of `password_len` bytes.
    #[must_use]
    pub fn new(password_len: usize) -> Self {
        Self {
            pool: Zeroizing::new([0u8; MAX_PASSWORD_LEN]),
            size: if password_len <= LEGACY_PASSWORD_LEN {
                LEGACY_PASSWORD_LEN
            } else {
                MAX_PASSWORD_LEN
            },
            count: 0,
        }
    }

    /// Mix one keyfile's contents (only its first MiB counts).
    ///
    /// # Errors
    /// [`VeraError::InvalidRequest`] for an empty keyfile, which VeraCrypt
    /// refuses.
    pub fn add(&mut self, contents: &[u8]) -> Result<()> {
        if contents.is_empty() {
            return Err(VeraError::InvalidRequest("a keyfile is empty".into()));
        }
        let mut crc = 0xFFFF_FFFFu32;
        let mut position = 0;
        for &byte in contents.iter().take(KEYFILE_READ_LIMIT) {
            crc = CRC_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8);
            for shift in [24u32, 16, 8, 0] {
                self.pool[position] = self.pool[position].wrapping_add((crc >> shift) as u8);
                position += 1;
            }
            if position >= self.size {
                position = 0;
            }
        }
        self.count += 1;
        Ok(())
    }

    /// Keyfiles mixed so far.
    #[must_use]
    pub fn count(&self) -> usize {
        self.count
    }

    /// The password VeraCrypt derives the header key from.
    ///
    /// # Errors
    /// [`VeraError::InvalidRequest`] for a password above 128 bytes.
    pub fn apply(&self, password: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if password.len() > MAX_PASSWORD_LEN {
            return Err(VeraError::InvalidRequest(format!(
                "VeraCrypt passwords are at most {MAX_PASSWORD_LEN} bytes"
            )));
        }
        let mut mixed = Zeroizing::new(self.pool[..self.size].to_vec());
        for (slot, &byte) in mixed.iter_mut().zip(password) {
            *slot = slot.wrapping_add(byte);
        }
        Ok(mixed)
    }
}

/// The keyfiles a path names: the file itself, or the files directly in a
/// directory, skipping hidden ones and subdirectories, as VeraCrypt does.
///
/// # Errors
/// I/O errors, or [`io::ErrorKind::NotFound`] for a directory without usable
/// files.
pub fn expand(path: &Path) -> io::Result<Vec<PathBuf>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_dir() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() || is_hidden(&entry.path(), &metadata) {
            continue;
        }
        files.push(entry.path());
    }
    if files.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("keyfile directory {} holds no keyfiles", path.display()),
        ));
    }
    files.sort();
    Ok(files)
}

#[cfg(windows)]
fn is_hidden(_path: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
fn is_hidden(path: &Path, _metadata: &fs::Metadata) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

/// The bytes of a keyfile that count: its first MiB.
///
/// # Errors
/// I/O errors.
pub fn read(path: &Path) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut contents = Zeroizing::new(Vec::new());
    fs::File::open(path)?
        .take(KEYFILE_READ_LIMIT as u64)
        .read_to_end(&mut contents)?;
    Ok(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A running CRC-32 without the final inversion equals the inverted
    /// standard CRC of the prefix.
    fn running_crcs(file: &[u8]) -> Vec<u32> {
        (1..=file.len())
            .map(|end| !crc32fast::hash(&file[..end]))
            .collect()
    }

    #[test]
    fn pool_bytes_follow_the_running_crc() {
        let file: Vec<u8> = (0..40u8).collect();
        let mut pool = KeyfilePool::new(0);
        pool.add(&file).unwrap();
        let mut expected = vec![0u8; 64];
        for (index, crc) in running_crcs(&file).into_iter().enumerate() {
            let position = (index * 4) % 64;
            for (offset, shift) in [24u32, 16, 8, 0].into_iter().enumerate() {
                expected[position + offset] =
                    expected[position + offset].wrapping_add((crc >> shift) as u8);
            }
        }
        assert_eq!(pool.apply(b"").unwrap().as_slice(), expected.as_slice());
    }

    #[test]
    fn password_is_added_and_order_does_not_matter() {
        let first: Vec<u8> = (0..300u32).map(|index| (index * 13) as u8).collect();
        let second = b"second keyfile".to_vec();
        let mut forward = KeyfilePool::new(4);
        forward.add(&first).unwrap();
        forward.add(&second).unwrap();
        let mut backward = KeyfilePool::new(4);
        backward.add(&second).unwrap();
        backward.add(&first).unwrap();
        let mixed = forward.apply(b"abcd").unwrap();
        assert_eq!(mixed, backward.apply(b"abcd").unwrap());
        let pool = forward.apply(b"").unwrap();
        assert_eq!(mixed.len(), 64);
        assert_eq!(mixed[0], pool[0].wrapping_add(b'a'));
        assert_eq!(mixed[4..], pool[4..]);
    }

    #[test]
    fn long_passwords_use_the_128_byte_pool() {
        let mut pool = KeyfilePool::new(72);
        pool.add(b"k").unwrap();
        assert_eq!(pool.apply(&[b'a'; 72]).unwrap().len(), 128);
        assert!(pool.apply(&[b'a'; 129]).is_err());
        assert!(KeyfilePool::new(0).add(b"").is_err());
    }

    #[test]
    fn only_the_first_mebibyte_counts() {
        let big: Vec<u8> = (0..KEYFILE_READ_LIMIT + 4096)
            .map(|index| (index * 7) as u8)
            .collect();
        let mut whole = KeyfilePool::new(0);
        whole.add(&big).unwrap();
        let mut prefix = KeyfilePool::new(0);
        prefix.add(&big[..KEYFILE_READ_LIMIT]).unwrap();
        assert_eq!(whole.apply(b"").unwrap(), prefix.apply(b"").unwrap());
    }
}
