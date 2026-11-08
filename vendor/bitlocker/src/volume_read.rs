//! Logical sector mapping, plaintext reads and Read/Seek implementation.
use super::*;

impl<R: Read + Seek> DecryptedVolume<R> {
    pub(super) fn open(
        mut reader: R,
        mut metadata: FveMetadata,
        cipher: SectorCipher,
    ) -> Result<Self> {
        let total_size = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;
        let mut boot = [0; 512];
        reader.read_exact(&mut boot)?;
        let header = VolumeHeader::parse(&boot)?;
        let sector_size = usize::from(header.bytes_per_sector);
        if total_size % sector_size as u64 != 0 {
            return Err(BdeError::InvalidData("volume length is not sector aligned"));
        }
        if metadata.volume_header_size > 0
            && (metadata.volume_header_offset % sector_size as u64 != 0
                || metadata.volume_header_size % sector_size as u64 != 0
                || metadata
                    .volume_header_offset
                    .checked_add(metadata.volume_header_size)
                    .is_none_or(|end| end > total_size))
        {
            return Err(BdeError::InvalidData(
                "invalid relocated volume header range",
            ));
        }
        for (index, offset) in metadata.metadata_offsets.iter_mut().enumerate() {
            if *offset == 0 {
                *offset = header.fve_metadata_offsets[index];
            }
        }
        let eow = crate::eow::EowMap::read(&mut reader, &header, total_size)?;
        Ok(Self {
            reader,
            cipher,
            metadata,
            total_size,
            position: 0,
            sector_size,
            eow,
        })
    }

    /// The parsed FVE metadata (cipher, protectors, volume GUID, …).
    #[must_use]
    pub fn metadata(&self) -> &FveMetadata {
        &self.metadata
    }

    /// The total size of the volume in bytes.
    #[must_use]
    pub fn volume_size(&self) -> u64 {
        self.total_size
    }

    /// Read decrypted bytes at logical `offset` into `buf`, filling it completely
    /// (out-of-range or truncated source data returns an error).
    ///
    /// # Errors
    /// Propagates I/O errors from the underlying reader.
    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|end| end > self.total_size)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "BitLocker read outside volume",
            )
            .into());
        }
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let sector_start = pos - (pos % self.sector_size as u64);
            let within = (pos - sector_start) as usize;
            let plain = self.decrypt_logical_sector(sector_start)?;
            let take = (self.sector_size - within).min(buf.len() - done);
            buf[done..done + take].copy_from_slice(&plain[within..within + take]);
            done += take;
        }
        Ok(())
    }

    /// Decrypt the logical sector starting at aligned `sector_start`, applying metadata-region blanking, volume-header relocation, and
    /// the encrypted-volume-size boundary exactly as BitLocker's read path does.
    fn decrypt_logical_sector(&mut self, sector_start: u64) -> Result<Vec<u8>> {
        // Metadata block regions read back as zero in the decrypted view.
        let meta_size = if self.metadata.version == 1 {
            16 * 1024
        } else {
            64 * 1024
        };
        for &m in &self.metadata.metadata_offsets {
            if m != 0 && sector_start >= m && sector_start - m < meta_size {
                return Ok(vec![0; self.sector_size]);
            }
        }

        if self.eow.is_metadata(sector_start)
            || (self.metadata.volume_header_size > 0
                && sector_start >= self.metadata.volume_header_offset
                && sector_start - self.metadata.volume_header_offset
                    < self.metadata.volume_header_size)
        {
            return Ok(vec![0; self.sector_size]);
        }

        // The first `volume_header_size` bytes are stored, encrypted, elsewhere;
        // the physical location doubles as the IV sector address.
        let physical = if sector_start < self.metadata.volume_header_size {
            self.metadata
                .volume_header_offset
                .checked_add(sector_start)
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "BitLocker header relocation overflow",
                    )
                })?
        } else {
            sector_start
        };

        let mut ct = vec![0; self.sector_size];
        self.reader.seek(SeekFrom::Start(physical))?;
        read_fill(&mut self.reader, &mut ct)?;

        if self.metadata.version == 1 && physical < 8192 {
            if physical == 0 {
                ct[3..11].copy_from_slice(b"NTFS    ");
                ct[56..64].copy_from_slice(&self.metadata.mft_mirror_cluster.to_le_bytes());
            }
            return Ok(ct);
        }
        // Not encrypted: NONE method, or past the still-encrypted region.
        let evs = self.metadata.encrypted_volume_size;
        if self.metadata.encryption_method == METHOD_NONE
            || (evs != 0 && physical >= evs)
            || self.eow.is_unencrypted(physical)
        {
            return Ok(ct);
        }

        Ok(self.cipher.decrypt_sector(&ct, physical))
    }
}

impl<R: Read + Seek> Read for DecryptedVolume<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.position >= self.total_size {
            return Ok(0);
        }
        let remaining = self.total_size - self.position;
        let n = (buf.len() as u64).min(remaining) as usize;
        self.read_at(self.position, &mut buf[..n])
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        self.position += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for DecryptedVolume<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(o) => i128::from(o),
            SeekFrom::End(o) => i128::from(self.total_size) + i128::from(o),
            SeekFrom::Current(o) => i128::from(self.position) + i128::from(o),
        };
        if new < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.position = u64::try_from(new).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "BitLocker seek overflow")
        })?;
        Ok(self.position)
    }
}
