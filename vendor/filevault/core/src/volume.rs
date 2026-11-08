//! Decrypted logical volumes use complete validated segment maps and logical-sector tweaks.
use crate::{unlock::VolumeKeys, xts, FileVaultError};
use std::io::{self, Read, Seek, SeekFrom};
const SECTOR_SIZE: usize = 512;

/// A contiguous mapping from logical volume bytes to physical evidence bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub logical_offset: u64,
    pub physical_offset: u64,
    pub length: u64,
}

#[derive(Debug)]
pub struct DecryptedVolume<R: Read + Seek> {
    reader: R,
    keys: VolumeKeys,
    segments: Vec<Segment>,
    size: u64,
    position: u64,
}
impl<R: Read + Seek> DecryptedVolume<R> {
    #[cfg(test)]
    pub(crate) fn new(reader: R, keys: VolumeKeys, physical_base: u64, size: u64) -> Self {
        Self {
            reader,
            keys,
            segments: vec![Segment {
                logical_offset: 0,
                physical_offset: physical_base,
                length: size,
            }],
            size,
            position: 0,
        }
    }
    /// Validate complete, non-overlapping, sector-aligned mappings before publishing bytes.
    pub fn with_segments(
        reader: R,
        keys: VolumeKeys,
        mut segments: Vec<Segment>,
        size: u64,
        source_size: u64,
    ) -> Result<Self, FileVaultError> {
        if size == 0 || !size.is_multiple_of(512) {
            return Err(crate::storage_metadata::invalid(
                "logical volume sector alignment",
            ));
        }
        segments.sort_by_key(|segment| segment.logical_offset);
        let mut covered = 0u64;
        for segment in &segments {
            if segment.logical_offset != covered
                || segment.length == 0
                || !segment.length.is_multiple_of(512)
                || !segment.physical_offset.is_multiple_of(512)
                || segment
                    .physical_offset
                    .checked_add(segment.length)
                    .is_none_or(|end| end > source_size)
            {
                return Err(crate::storage_metadata::invalid(
                    "segment gap, overlap, alignment or physical bound",
                ));
            }
            covered = covered
                .checked_add(segment.length)
                .ok_or_else(|| crate::storage_metadata::invalid("segment length overflow"))?;
        }
        if covered < size {
            return Err(crate::storage_metadata::invalid(
                "segments do not cover logical volume",
            ));
        }
        Ok(Self {
            reader,
            keys,
            segments,
            size,
            position: 0,
        })
    }
    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }
    /// Read logical bytes. Every enclosing ciphertext sector must be completely readable.
    pub fn read_at(&mut self, offset: u64, output: &mut [u8]) -> Result<usize, FileVaultError> {
        let wanted = (output.len() as u64).min(self.size.saturating_sub(offset)) as usize;
        let mut filled = 0usize;
        while filled < wanted {
            let logical = offset + filled as u64;
            let segment = self
                .segments
                .iter()
                .find(|segment| {
                    logical >= segment.logical_offset
                        && logical - segment.logical_offset < segment.length
                })
                .ok_or_else(|| crate::storage_metadata::invalid("unmapped logical sector"))?;
            let aligned = logical / 512 * 512;
            let inner = (logical - aligned) as usize;
            let count = (wanted - filled).min(
                (segment.length - (logical - segment.logical_offset)).min(1024 * 1024) as usize,
            );
            let sectors = (inner + count).div_ceil(512);
            let mut ciphertext = vec![0; sectors * 512];
            let physical = segment
                .physical_offset
                .checked_add(aligned - segment.logical_offset)
                .ok_or_else(|| crate::storage_metadata::invalid("physical offset overflow"))?;
            self.reader.seek(SeekFrom::Start(physical))?;
            read_full_or_eof(&mut self.reader, &mut ciphertext)?;
            xts::decrypt_units(
                &mut ciphertext,
                &self.keys.vmk,
                &self.keys.tweak_key,
                SECTOR_SIZE,
                u128::from(aligned / 512),
            );
            output[filled..filled + count].copy_from_slice(&ciphertext[inner..inner + count]);
            filled += count;
        }
        Ok(filled)
    }
}
fn read_full_or_eof<R: Read>(reader: &mut R, bytes: &mut [u8]) -> io::Result<()> {
    reader.read_exact(bytes)
}
impl<R: Read + Seek> Read for DecryptedVolume<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self
            .read_at(self.position, output)
            .map_err(io::Error::other)?;
        self.position += count as u64;
        Ok(count)
    }
}
impl<R: Read + Seek> Seek for DecryptedVolume<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.position = match from {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.size.checked_add_signed(delta),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid logical seek"))?;
        Ok(self.position)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn keys() -> VolumeKeys {
        VolumeKeys {
            vmk: [0x55u8; 16],
            tweak_key: [0x66u8; 16],
        }
    }

    /// Build a backing image: `physical_base` bytes of padding, then XTS-encrypted
    /// sectors whose plaintext is known. Logical sector N uses tweak N.
    fn build_backing(base: u64, plaintext: &[u8], keys: &VolumeKeys) -> Vec<u8> {
        let mut cipher = plaintext.to_vec();
        crate::xts::encrypt_units(&mut cipher, &keys.vmk, &keys.tweak_key, SECTOR_SIZE, 0);
        let mut image = vec![0u8; base as usize];
        image.extend_from_slice(&cipher);
        image
    }

    #[test]
    fn read_at_decrypts_aligned_sector() {
        let k = keys();
        let plaintext: Vec<u8> = (0..1024u32).map(|i| (i & 0xff) as u8).collect();
        let image = build_backing(4096, &plaintext, &k);
        let mut vol =
            DecryptedVolume::new(Cursor::new(image), keys(), 4096, plaintext.len() as u64);
        let mut buf = [0u8; 512];
        assert_eq!(vol.read_at(0, &mut buf).unwrap(), 512);
        assert_eq!(&buf[..], &plaintext[..512]);
        assert_eq!(vol.read_at(512, &mut buf).unwrap(), 512);
        assert_eq!(&buf[..], &plaintext[512..1024]);
    }

    #[test]
    fn read_at_handles_unaligned_window() {
        let k = keys();
        let plaintext: Vec<u8> = (0..1024u32).map(|i| (i & 0xff) as u8).collect();
        let image = build_backing(0, &plaintext, &k);
        let mut vol = DecryptedVolume::new(Cursor::new(image), keys(), 0, plaintext.len() as u64);
        let mut buf = [0u8; 300];
        // Offset 600 spans sector 1 and sector 2 boundary crossing.
        assert_eq!(vol.read_at(600, &mut buf).unwrap(), 300);
        assert_eq!(&buf[..], &plaintext[600..900]);
    }

    #[test]
    fn read_at_past_end_returns_zero() {
        let mut vol = DecryptedVolume::new(Cursor::new(vec![0u8; 4096]), keys(), 0, 512);
        let mut buf = [0u8; 16];
        assert_eq!(vol.read_at(512, &mut buf).unwrap(), 0);
        assert_eq!(vol.read_at(1000, &mut buf).unwrap(), 0);
    }

    #[test]
    fn read_at_truncates_to_volume_end() {
        let k = keys();
        let plaintext: Vec<u8> = (0..512u32).map(|i| (i & 0xff) as u8).collect();
        let image = build_backing(0, &plaintext, &k);
        let mut vol = DecryptedVolume::new(Cursor::new(image), keys(), 0, 512);
        let mut buf = [0u8; 512];
        // Ask for 512 from offset 256 → only 256 available.
        assert_eq!(vol.read_at(256, &mut buf).unwrap(), 256);
        assert_eq!(&buf[..256], &plaintext[256..512]);
    }

    #[test]
    fn empty_buffer_reads_nothing() {
        let mut vol = DecryptedVolume::new(Cursor::new(vec![0u8; 4096]), keys(), 0, 512);
        assert_eq!(vol.read_at(0, &mut []).unwrap(), 0);
    }

    #[test]
    fn size_accessor() {
        let vol = DecryptedVolume::new(Cursor::new(vec![0u8; 16]), keys(), 0, 167_772_160);
        assert_eq!(vol.size(), 167_772_160);
    }

    #[test]
    fn read_and_seek_impls() {
        let k = keys();
        let plaintext: Vec<u8> = (0..1024u32).map(|i| (i & 0xff) as u8).collect();
        let image = build_backing(0, &plaintext, &k);
        let mut vol = DecryptedVolume::new(Cursor::new(image), keys(), 0, plaintext.len() as u64);
        let mut buf = [0u8; 512];
        assert_eq!(std::io::Read::read(&mut vol, &mut buf).unwrap(), 512);
        assert_eq!(&buf[..], &plaintext[..512]);
        // Seek to sector 1 and read.
        assert_eq!(vol.seek(SeekFrom::Start(512)).unwrap(), 512);
        assert_eq!(std::io::Read::read(&mut vol, &mut buf).unwrap(), 512);
        assert_eq!(&buf[..], &plaintext[512..1024]);
        // SeekFrom::End / Current.
        assert_eq!(vol.seek(SeekFrom::End(0)).unwrap(), 1024);
        assert_eq!(vol.seek(SeekFrom::Current(-1024)).unwrap(), 0);
    }

    #[test]
    fn seek_out_of_range_errors() {
        let mut vol = DecryptedVolume::new(Cursor::new(vec![0u8; 16]), keys(), 0, 512);
        assert!(vol.seek(SeekFrom::Current(-1)).is_err());
        assert!(vol.seek(SeekFrom::End(1)).is_ok());
    }

    /// A reader that yields one `Interrupted`, then one hard error after N bytes.
    struct FlakyReader {
        data: Vec<u8>,
        pos: usize,
        interrupted_once: bool,
    }

    impl Read for FlakyReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted_once {
                self.interrupted_once = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            if self.pos >= self.data.len() {
                return Err(io::Error::other("backing read failed"));
            }
            let n = (buf.len()).min(self.data.len() - self.pos).min(4);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn read_full_or_eof_retries_interrupt_and_propagates_error() {
        // Interrupted is retried; a subsequent hard error propagates.
        let mut r = FlakyReader {
            data: vec![1u8; 8],
            pos: 0,
            interrupted_once: false,
        };
        let mut buf = [0u8; 16];
        let err = read_full_or_eof(&mut r, &mut buf).unwrap_err();
        assert_eq!(err.to_string(), "backing read failed");
        // The 8 available bytes were read before the error (interrupt was retried).
        assert_eq!(&buf[..8], &[1u8; 8]);
    }

    /// A reader that returns clean EOF (Ok(0)) immediately.
    struct EofReader;
    impl Read for EofReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    #[test]
    fn read_full_or_eof_stops_at_clean_eof() {
        let mut r = EofReader;
        let mut buf = [0xffu8; 8];
        assert_eq!(
            read_full_or_eof(&mut r, &mut buf).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        // Untouched (zero bytes read, no error).
        assert_eq!(buf, [0xffu8; 8]);
    }
}
