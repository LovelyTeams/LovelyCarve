//! Public API: brute the VeraCrypt header PRF+cipher from a password, recover the
//! master key, and decrypt the data area.

use std::io::{Read, Seek, SeekFrom};

use crate::crypto::{cipher_chains, xts_decrypt_chain, Cipher, Prf, MAX_CHAIN_KEY_LEN};
use crate::error::{Result, VeraError};
use crate::header::{
    Flavor, VeraHeader, HEADER_LEN, HIDDEN_HEADER_OFFSET, NORMAL_HEADER_OFFSET, SALT_LEN,
    VOLUME_HEADER_LEN,
};

/// VeraCrypt data-area encryption sector size (bytes).
const DATA_SECTOR: usize = 512;

/// Namespace for opening a VeraCrypt volume.
pub struct VeraVolume;

/// The recovered facts about an unlocked volume.
#[derive(Debug, Clone)]
pub struct VolumeInfo {
    /// VeraCrypt or TrueCrypt.
    pub flavor: Flavor,
    /// The PRF that decrypted the header.
    pub prf: Prf,
    /// The cipher chain in cryptsetup array order (one entry for a single cipher,
    /// more for a cascade). Ciphers are *applied* in reverse of this order.
    pub ciphers: Vec<Cipher>,
    /// Header format version.
    pub version: u16,
    /// Byte offset where the encrypted data area begins.
    pub encrypted_area_start: u64,
    /// Size of the encrypted data area in bytes.
    pub encrypted_area_size: u64,
}

impl VolumeInfo {
    /// The cipher chain as a VeraCrypt display string (`aes`, `aes-twofish`, …),
    /// i.e. the ciphers in *application* (decrypt) order — the reverse of the
    /// cryptsetup array order stored in `ciphers`.
    #[must_use]
    pub fn cipher_display(&self) -> String {
        self.ciphers
            .iter()
            .rev()
            .map(|c| c.name())
            .collect::<Vec<_>>()
            .join("-")
    }
}

impl VeraVolume {
    /// Unlock a VeraCrypt/TrueCrypt volume with `password` (no PIM).
    ///
    /// # Errors
    /// [`VeraError::TooSmall`] if the container is under 512 bytes, or
    /// [`VeraError::AuthenticationFailed`] if no PRF/cipher yields a valid header.
    pub fn unlock_with_password<R: Read + Seek>(
        reader: R,
        password: &[u8],
    ) -> Result<DecryptedVolume<R>> {
        Self::unlock_at(reader, password, 0, NORMAL_HEADER_OFFSET)
    }

    /// Unlock with an explicit PIM (personal iterations multiplier; 0 = default).
    ///
    /// # Errors
    /// As [`Self::unlock_with_password`].
    pub fn unlock_with_pim<R: Read + Seek>(
        reader: R,
        password: &[u8],
        pim: u32,
    ) -> Result<DecryptedVolume<R>> {
        Self::unlock_at(reader, password, pim, NORMAL_HEADER_OFFSET)
    }

    /// Unlock the HIDDEN volume with `password` (its header is at 64 KiB). Used to
    /// access — or prove the presence of — a deniable hidden volume.
    ///
    /// # Errors
    /// As [`Self::unlock_with_password`].
    pub fn unlock_hidden_with_password<R: Read + Seek>(
        reader: R,
        password: &[u8],
    ) -> Result<DecryptedVolume<R>> {
        Self::unlock_at(reader, password, 0, HIDDEN_HEADER_OFFSET)
    }

    /// Unlock the hidden volume with an explicit PIM.
    ///
    /// # Errors
    /// As [`Self::unlock_with_password`].
    pub fn unlock_hidden_with_pim<R: Read + Seek>(
        reader: R,
        password: &[u8],
        pim: u32,
    ) -> Result<DecryptedVolume<R>> {
        Self::unlock_at(reader, password, pim, HIDDEN_HEADER_OFFSET)
    }

    /// Shared unlock: read the 512-byte volume header at `header_offset`, brute the
    /// PRF x cipher, and build the decrypting reader.
    fn unlock_at<R: Read + Seek>(
        mut reader: R,
        password: &[u8],
        pim: u32,
        header_offset: u64,
    ) -> Result<DecryptedVolume<R>> {
        let total_size = reader.seek(SeekFrom::End(0))?;
        if total_size < header_offset + VOLUME_HEADER_LEN as u64 {
            return Err(VeraError::TooSmall {
                got: total_size as usize,
            });
        }

        let mut hdr = [0u8; VOLUME_HEADER_LEN];
        reader.seek(SeekFrom::Start(header_offset))?;
        read_fill(&mut reader, &mut hdr)?;
        let salt = &hdr[0..SALT_LEN];
        let header_ct = &hdr[SALT_LEN..VOLUME_HEADER_LEN];

        // PBKDF2 is a prefix function, so derive the max cascade key length once
        // per PRF and slice per chain rather than re-deriving.
        for prf in Prf::all() {
            reader.stream_position()?;
            let hk = prf.derive(password, salt, prf.iterations_pim(pim), MAX_CHAIN_KEY_LEN);
            reader.stream_position()?;
            for chain in cipher_chains() {
                let klen = 64 * chain.len();
                let mut dec = header_ct.to_vec();
                xts_decrypt_chain(&chain, &hk[..klen], &mut dec, HEADER_LEN, 0)?;
                let Some(h) = VeraHeader::validate(&dec) else {
                    continue;
                };
                if h.flags != 0
                    || h.volume_size != h.encrypted_area_size
                    || h.sector_size != 512
                    || h.encrypted_area_size == 0
                    || h.encrypted_area_start % 512 != 0
                    || h.encrypted_area_size % 512 != 0
                    || h.encrypted_area_start
                        .checked_add(h.encrypted_area_size)
                        .map_or(true, |end| end > total_size)
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid VeraCrypt data geometry",
                    )
                    .into());
                }
                let master_key = h.master_keys[..klen].to_vec();
                return Ok(DecryptedVolume {
                    reader,
                    ciphers: chain.clone(),
                    master_key,
                    data_offset: h.encrypted_area_start,
                    base_unit: u128::from(h.encrypted_area_start / DATA_SECTOR as u64),
                    total_size,
                    position: 0,
                    info: VolumeInfo {
                        flavor: h.flavor,
                        prf,
                        ciphers: chain,
                        version: h.version,
                        encrypted_area_start: h.encrypted_area_start,
                        encrypted_area_size: h.encrypted_area_size,
                    },
                });
            }
        }
        Err(VeraError::AuthenticationFailed)
    }
}

/// A plaintext view of an unlocked VeraCrypt data area.
pub struct DecryptedVolume<R> {
    reader: R,
    ciphers: Vec<Cipher>,
    master_key: Vec<u8>,
    data_offset: u64,
    base_unit: u128,
    total_size: u64,
    position: u64,
    info: VolumeInfo,
}

impl<R: Read + Seek> DecryptedVolume<R> {
    /// The recovered volume facts (flavor, PRF, cipher, offsets).
    #[must_use]
    pub fn info(&self) -> &VolumeInfo {
        &self.info
    }

    /// The recovered master key (sensitive).
    #[must_use]
    pub fn master_key(&self) -> &[u8] {
        &self.master_key
    }

    /// Size of the decrypted data area in bytes.
    #[must_use]
    pub fn data_size(&self) -> u64 {
        if self.info.encrypted_area_size != 0 {
            self.info.encrypted_area_size
        } else {
            self.total_size.saturating_sub(self.data_offset)
        }
    }

    /// Read decrypted data at data-area-relative `offset` into `buf`, filling it
    /// completely; reads beyond the declared data area fail.
    ///
    /// # Errors
    /// Propagates I/O and cipher errors.
    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(buf.len() as u64)
            .map_or(true, |end| end > self.data_size())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "read exceeds decrypted data area",
            )
            .into());
        }
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let unit = pos / DATA_SECTOR as u64;
            let within = (pos % DATA_SECTOR as u64) as usize;
            let physical = self.data_offset + unit * DATA_SECTOR as u64;

            let mut ct = [0u8; DATA_SECTOR];
            self.reader.seek(SeekFrom::Start(physical))?;
            self.reader.read_exact(&mut ct)?;
            xts_decrypt_chain(
                &self.ciphers,
                &self.master_key,
                &mut ct,
                DATA_SECTOR,
                self.base_unit + u128::from(unit),
            )?; // cov:unreachable: chain+key came from a successful unlock

            let take = (DATA_SECTOR - within).min(buf.len() - done);
            buf[done..done + take].copy_from_slice(&ct[within..within + take]);
            done += take;
        }
        Ok(())
    }
}

impl<R: Read + Seek> Read for DecryptedVolume<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let size = self.data_size();
        if self.position >= size {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(size - self.position) as usize;
        self.read_at(self.position, &mut buf[..n])
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.position += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for DecryptedVolume<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let size = self.data_size();
        let new = match pos {
            SeekFrom::Start(o) => i128::from(o),
            SeekFrom::End(o) => i128::from(size) + i128::from(o),
            SeekFrom::Current(o) => i128::from(self.position) + i128::from(o),
        };
        if new < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.position = new as u64;
        Ok(self.position)
    }
}

fn read_fill<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<()> {
    reader.read_exact(buf)?;
    Ok(())
}

#[cfg(test)]
fn read_available<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    for b in &mut buf[filled..] {
        *b = 0;
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read as _, Seek as _, SeekFrom};

    use aes::cipher::KeyInit;
    use aes::Aes256;
    use xts_mode::Xts128;

    use super::*;
    use crate::crypto::Prf;

    const PASSWORD: &[u8] = b"correct horse";
    const DATA_START: u64 = 512; // encrypted area begins right after the 512-byte header
    const DATA_SECTORS: usize = 3;

    /// AES-256-XTS encrypt (the inverse of `crypto::decrypt_units`) for fixtures.
    fn xts_encrypt_aes(key64: &[u8; 64], buf: &mut [u8], unit_size: usize, base: u128) {
        let (k1, k2) = key64.split_at(32);
        let xts = Xts128::new(Aes256::new(k1.into()), Aes256::new(k2.into()));
        for (u, chunk) in buf.chunks_mut(unit_size).enumerate() {
            xts.encrypt_sector(chunk, (base + u as u128).to_le_bytes());
        }
    }

    /// Assemble a synthetic AES-256-XTS VeraCrypt container in memory and return
    /// `(container_bytes, plaintext_data_area)`. The header is a real VeraCrypt
    /// header (VERA magic + both CRC-32s) XTS-encrypted under the SHA-512 header
    /// key at PIM 1, so the crate's own brute recovers it. Correctness of the
    /// crypto itself is proven by the Tier-1 oracle; this only drives the paths.
    fn build_volume() -> (Vec<u8>, Vec<u8>) {
        build_volume_with(true)
    }

    fn build_volume_with(declare_size: bool) -> (Vec<u8>, Vec<u8>) {
        let salt = [0x11u8; SALT_LEN];
        let master_key = [0x24u8; 64];

        // Decrypted 448-byte header.
        let mut dec = [0u8; HEADER_LEN];
        dec[0..4].copy_from_slice(b"VERA");
        dec[4..6].copy_from_slice(&5u16.to_be_bytes());
        let data_size = (DATA_SECTORS * DATA_SECTOR) as u64;
        dec[36..44].copy_from_slice(&data_size.to_be_bytes()); // volume size
        dec[44..52].copy_from_slice(&DATA_START.to_be_bytes()); // encrypted-area start
                                                                // 0 = "size not declared" ⇒ reader falls back to total_size - data_offset.
        let declared = if declare_size { data_size } else { 0 };
        dec[52..60].copy_from_slice(&declared.to_be_bytes()); // encrypted-area size
        dec[64..68].copy_from_slice(&512u32.to_be_bytes());
        dec[192..256].copy_from_slice(&master_key); // master-key material
        let crc_mk = crc32fast::hash(&dec[192..448]);
        dec[8..12].copy_from_slice(&crc_mk.to_be_bytes());
        let crc_hdr = crc32fast::hash(&dec[0..188]);
        dec[188..192].copy_from_slice(&crc_hdr.to_be_bytes());

        // Encrypt the header with the SHA-512 header key (PIM 1 = 16000 iterations).
        let header_key = Prf::Sha512.derive(PASSWORD, &salt, Prf::Sha512.iterations_pim(1), 64);
        let mut header_ct = dec.to_vec();
        let hk: [u8; 64] = header_key.try_into().unwrap();
        xts_encrypt_aes(&hk, &mut header_ct, HEADER_LEN, 0);

        // Plaintext data area, then encrypt each sector at tweak base_unit + lba.
        let mut plain = vec![0u8; DATA_SECTORS * DATA_SECTOR];
        for (i, b) in plain.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7) ^ 0xa5;
        }
        let base_unit = u128::from(DATA_START / DATA_SECTOR as u64);
        let mut data_ct = plain.clone();
        xts_encrypt_aes(&master_key, &mut data_ct, DATA_SECTOR, base_unit);

        let mut container = Vec::new();
        container.extend_from_slice(&salt);
        container.extend_from_slice(&header_ct);
        container.extend_from_slice(&data_ct);
        (container, plain)
    }

    #[test]
    fn hermetic_unlock_and_read_roundtrip() {
        let (container, plain) = build_volume();
        let mut vol = VeraVolume::unlock_with_pim(Cursor::new(container), PASSWORD, 1)
            .expect("unlock synthetic volume");

        assert_eq!(vol.info().prf.name(), "sha512");
        assert_eq!(vol.info().cipher_display(), "aes");
        assert_eq!(vol.info().flavor, Flavor::VeraCrypt);
        assert_eq!(vol.info().version, 5);
        assert_eq!(vol.info().encrypted_area_start, DATA_START);
        assert_eq!(vol.master_key().len(), 64);
        assert_eq!(vol.data_size(), plain.len() as u64);

        // Sector-by-sector read_at.
        for lba in 0..DATA_SECTORS as u64 {
            let mut buf = [0u8; DATA_SECTOR];
            vol.read_at(lba * DATA_SECTOR as u64, &mut buf).unwrap();
            let want = &plain[(lba as usize) * DATA_SECTOR..(lba as usize + 1) * DATA_SECTOR];
            assert_eq!(&buf[..], want, "sector {lba}");
        }

        // Unaligned read spanning a sector boundary.
        let mut span = [0u8; 10];
        vol.read_at(510, &mut span).unwrap();
        assert_eq!(&span[..], &plain[510..520]);
    }

    #[test]
    fn hermetic_read_and_seek_traits() {
        let (container, plain) = build_volume();
        let mut vol =
            VeraVolume::unlock_with_pim(Cursor::new(container), PASSWORD, 1).expect("unlock");

        // Read the whole data area via the Read impl.
        let mut all = Vec::new();
        vol.read_to_end(&mut all).unwrap();
        assert_eq!(all, plain);
        // At EOF the Read impl yields 0.
        assert_eq!(vol.read(&mut [0u8; 16]).unwrap(), 0);

        // Seek from End then Current, and reject a negative seek.
        let pos = vol.seek(SeekFrom::End(-512)).unwrap();
        assert_eq!(pos, (plain.len() - 512) as u64);
        assert_eq!(vol.seek(SeekFrom::Current(0)).unwrap(), pos);
        assert_eq!(vol.seek(SeekFrom::Start(0)).unwrap(), 0);
        assert!(vol.seek(SeekFrom::Current(-1)).is_err());
    }

    #[test]
    fn hidden_offset_too_small_errors() {
        // A container large enough for a normal header but not the hidden one.
        let small = vec![0u8; VOLUME_HEADER_LEN];
        assert!(matches!(
            VeraVolume::unlock_hidden_with_password(Cursor::new(small), PASSWORD),
            Err(VeraError::TooSmall { .. })
        ));
    }

    #[test]
    fn hidden_pim_offset_too_small_errors() {
        // Big enough for a normal header but not the hidden one at 64 KiB.
        let small = vec![0u8; VOLUME_HEADER_LEN];
        assert!(matches!(
            VeraVolume::unlock_hidden_with_pim(Cursor::new(small), PASSWORD, 1),
            Err(VeraError::TooSmall { .. })
        ));
    }

    #[test]
    fn too_small_container_errors() {
        assert!(matches!(
            VeraVolume::unlock_with_password(Cursor::new(vec![0u8; 100]), PASSWORD),
            Err(VeraError::TooSmall { got }) if got == 100
        ));
    }

    #[test]
    fn wrong_password_fails() {
        let (container, _) = build_volume();
        assert!(matches!(
            VeraVolume::unlock_with_pim(Cursor::new(container), b"wrong", 1),
            Err(VeraError::AuthenticationFailed)
        ));
    }

    #[test]
    fn undeclared_size_is_rejected() {
        let (container, _) = build_volume_with(false);
        assert!(VeraVolume::unlock_with_pim(Cursor::new(container), PASSWORD, 1).is_err());
    }

    #[test]
    fn read_available_handles_eof_interrupt_and_hard_error() {
        use std::io;

        // EOF immediately -> break, then zero-fill the whole buffer.
        struct Eof;
        impl io::Read for Eof {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Ok(0)
            }
        }
        let mut buf = [0xffu8; 8];
        assert_eq!(read_available(&mut Eof, &mut buf).unwrap(), 0);
        assert_eq!(buf, [0u8; 8]);

        // Interrupted once, then one byte, then EOF (covers the Interrupted arm).
        struct Flaky(u8);
        impl io::Read for Flaky {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                self.0 += 1;
                match self.0 {
                    1 => Err(io::Error::new(io::ErrorKind::Interrupted, "eintr")),
                    2 => {
                        b[0] = 0xAB;
                        Ok(1)
                    }
                    _ => Ok(0),
                }
            }
        }
        let mut b2 = [0u8; 4];
        assert_eq!(read_available(&mut Flaky(0), &mut b2).unwrap(), 1);
        assert_eq!(b2[0], 0xAB);

        // A hard error propagates.
        struct Boom;
        impl io::Read for Boom {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("boom"))
            }
        }
        let mut b3 = [0u8; 4];
        assert!(read_available(&mut Boom, &mut b3).is_err());
    }
}
