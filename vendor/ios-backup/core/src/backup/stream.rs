//! LovelyCarve extension: bounded streaming without materializing whole blobs.
use std::io::{self, Read, Seek, SeekFrom};

use aes::cipher::{BlockDecryptMut, KeyIvInit};
use zeroize::Zeroizing;

use super::Backup;
use crate::{crypto, BackupFile};

impl Backup {
    fn file_decryptor(
        &self,
        entry: &BackupFile,
        iv: &[u8; 16],
    ) -> io::Result<cbc::Decryptor<aes::Aes256>> {
        let classes = self
            .class_keys
            .as_ref()
            .ok_or_else(|| io::Error::other("backup is locked"))?;
        let wrapped = entry
            .encryption_key
            .as_deref()
            .ok_or_else(|| io::Error::other("missing file key"))?;
        let class = entry
            .protection_class
            .ok_or_else(|| io::Error::other("missing protection class"))?;
        let key = Zeroizing::new(
            crypto::unwrap_file_key(classes, class, wrapped).map_err(io::Error::other)?,
        );
        cbc::Decryptor::<aes::Aes256>::new_from_slices(key.as_slice(), iv).map_err(io::Error::other)
    }

    /// Resolve the logical size, including the upstream padding fallback when
    /// Size is absent. Only the last two blocks need to be read for that case.
    pub fn content_size(&self, entry: &BackupFile) -> io::Result<u64> {
        if let Some(size) = entry.size {
            return Ok(size);
        }
        let path = self
            .blob_path(entry)
            .ok_or_else(|| io::Error::other("unsafe iOS file ID"))?;
        let mut file = std::fs::File::open(path)?;
        let size = file.metadata()?.len();
        if !self.is_encrypted() || size == 0 {
            return Ok(size);
        }
        if !size.is_multiple_of(16) {
            return Err(io::Error::other("iOS ciphertext is not block aligned"));
        }
        let mut iv = [0; 16];
        if size >= 32 {
            file.seek(SeekFrom::Start(size - 32))?;
            file.read_exact(&mut iv)?;
        } else {
            file.seek(SeekFrom::Start(0))?;
        }
        let mut tail = Zeroizing::new([0; 16]);
        file.read_exact(tail.as_mut())?;
        self.file_decryptor(entry, &iv)?
            .decrypt_block_mut(tail.as_mut().into());
        let pad = usize::from(tail[15]);
        let padding = if pad > 0
            && pad <= 16
            && tail[16 - pad..]
                .iter()
                .all(|byte| usize::from(*byte) == pad)
        {
            pad as u64
        } else {
            0
        };
        Ok(size - padding)
    }

    /// Seek to plaintext bytes using the preceding ciphertext block as CBC IV.
    /// Does not scan or decrypt the prefix of a large file.
    pub fn read_range(
        &self,
        entry: &BackupFile,
        offset: u64,
        length: usize,
    ) -> io::Result<Vec<u8>> {
        if !entry.kind.has_content() {
            return Err(io::Error::other("iOS entry has no file content"));
        }
        let size = self.content_size(entry)?;
        let path = self
            .blob_path(entry)
            .ok_or_else(|| io::Error::other("unsafe iOS file ID"))?;
        let mut file = std::fs::File::open(path)?;
        let blob_size = file.metadata()?.len();
        if size > blob_size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "iOS blob shorter than manifest size",
            ));
        }
        if self.is_encrypted() && !blob_size.is_multiple_of(16) {
            return Err(io::Error::other("iOS ciphertext is not block aligned"));
        }
        let length = usize::try_from(size.saturating_sub(offset).min(length as u64))
            .map_err(io::Error::other)?;
        if length == 0 {
            return Ok(Vec::new());
        }
        if !self.is_encrypted() {
            file.seek(SeekFrom::Start(offset))?;
            let mut bytes = vec![0; length];
            file.read_exact(&mut bytes)?;
            return Ok(bytes);
        }
        let start = offset / 16 * 16;
        let skip = usize::try_from(offset - start).map_err(io::Error::other)?;
        let take = length
            .checked_add(skip)
            .and_then(|size| size.checked_add(15))
            .ok_or_else(|| io::Error::other("iOS range overflow"))?
            / 16
            * 16;
        let mut iv = [0; 16];
        if start > 0 {
            file.seek(SeekFrom::Start(start - 16))?;
            file.read_exact(&mut iv)?;
        } else {
            file.seek(SeekFrom::Start(0))?;
        }
        let mut bytes = Zeroizing::new(vec![0; take]);
        file.read_exact(&mut bytes)?;
        let mut cipher = self.file_decryptor(entry, &iv)?;
        for block in bytes.chunks_exact_mut(16) {
            cipher.decrypt_block_mut(block.into());
        }
        Ok(bytes[skip..skip + length].to_vec())
    }

    /// Stream file plaintext in bounded chunks. Return `false` from the visitor
    /// to stop after a prefix. Known manifest sizes reject truncated blobs;
    /// unknown sizes use the same padding policy as [`Self::read`].
    pub fn stream<F>(
        &self,
        entry: &BackupFile,
        chunk_size: usize,
        mut visitor: F,
    ) -> io::Result<u64>
    where
        F: FnMut(&[u8]) -> io::Result<bool>,
    {
        if !entry.kind.has_content() {
            return Err(io::Error::other("iOS entry has no file content"));
        }
        let path = self
            .blob_path(entry)
            .ok_or_else(|| io::Error::other("unsafe iOS file ID"))?;
        let mut file = std::fs::File::open(path)?;
        let blob_size = file.metadata()?.len();
        if entry.size.is_some_and(|size| size > blob_size) {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "iOS blob shorter than manifest size",
            ));
        }
        let mut decryptor = if self.is_encrypted() {
            if !blob_size.is_multiple_of(16) {
                return Err(io::Error::other("iOS ciphertext is not block aligned"));
            }
            Some(self.file_decryptor(entry, &[0; 16])?)
        } else {
            None
        };
        let chunk_size = chunk_size.clamp(64 * 1024, 1024 * 1024) / 16 * 16;
        let mut buffer = Zeroizing::new(vec![0; chunk_size]);
        let mut consumed = 0;
        let mut emitted = 0;
        while consumed < blob_size {
            let count = usize::try_from((blob_size - consumed).min(chunk_size as u64))
                .map_err(io::Error::other)?;
            file.read_exact(&mut buffer[..count])?;
            consumed += count as u64;
            if let Some(cipher) = decryptor.as_mut() {
                for block in buffer[..count].chunks_exact_mut(16) {
                    cipher.decrypt_block_mut(block.into());
                }
            }
            let mut count = entry.size.map_or(count, |size| {
                size.saturating_sub(emitted).min(count as u64) as usize
            });
            if self.is_encrypted() && entry.size.is_none() && consumed == blob_size {
                if let Some(&pad) = buffer[..count].last() {
                    let pad = usize::from(pad);
                    if pad > 0
                        && pad <= 16
                        && pad <= count
                        && buffer[count - pad..count]
                            .iter()
                            .all(|byte| usize::from(*byte) == pad)
                    {
                        count -= pad;
                    }
                }
            }
            if count > 0 {
                emitted += count as u64;
                if !visitor(&buffer[..count])? {
                    return Ok(emitted);
                }
            }
            if entry.size == Some(emitted) {
                break;
            }
        }
        if entry.size.is_some_and(|size| emitted != size) {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "iOS file ended before manifest size",
            ));
        }
        Ok(emitted)
    }
}
