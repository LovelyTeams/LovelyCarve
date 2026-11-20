//! `-wal` and `-journal` decryption with the keys of their database.
//!
//! `SQLCipher` encrypts every page it writes to a sidecar exactly as it
//! encrypts the database page of the same number, and `SQLite` computes the
//! WAL frame and journal record checksums over those encrypted pages. The
//! decrypted sidecars keep every header byte and replace each checksum with
//! one over the plaintext page wherever the original verified, and with a
//! wrong one wherever it did not, so a reader trusts and replays the same
//! frames and records it would have in the encrypted file.

use super::Decrypted;
use crate::WalChecksumEndian;

/// `SQLite`'s WAL header and frame-header lengths (`wal.c`).
const WAL_HEADER_LEN: usize = 32;
const WAL_FRAME_HEADER_LEN: usize = 24;
/// The fields of a rollback-journal header (`pager.c`); the header itself
/// fills the first sector.
const JOURNAL_HEADER_LEN: usize = 28;
/// A journal page record: page number, page, checksum.
const JOURNAL_RECORD_OVERHEAD: usize = 8;

/// A `-wal` or `-journal` decrypted with its database's keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecryptedSidecar {
    /// The sidecar as `SQLite` writes it for an unencrypted database: a WAL
    /// for [`crate::Database::open_with_wal`], a journal for
    /// [`crate::Database::rollback_prior`].
    pub plaintext: Vec<u8>,
    /// Frames or page records left out: decryption stops at the first page
    /// that does not authenticate (a torn write, or another key).
    pub omitted: usize,
}

/// Why a sidecar could not be decrypted with its database's keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarError {
    /// Shorter than its header, or a WAL without a WAL magic.
    Malformed,
    /// A WAL whose page size is not the database's.
    PageSizeMismatch {
        /// The database's page size.
        database: u32,
        /// The page size the WAL header declares.
        wal: u32,
    },
}

impl Decrypted {
    /// Decrypt the database's `-wal` with the keys that decrypted the database.
    ///
    /// Frame checksums are recomputed while the encrypted frames' checksum
    /// chain verifies — seeded with the header's stored checksum and under the
    /// header's salts, as `SQLite` reads it — and are left wrong from the first
    /// frame where it does not. Frames after a salt change, from an earlier WAL
    /// generation, are decrypted too.
    ///
    /// # Errors
    /// [`SidecarError::Malformed`] without a WAL header, and
    /// [`SidecarError::PageSizeMismatch`] when its page size is not the
    /// database's.
    pub fn decrypt_wal(&self, wal: &[u8]) -> Result<DecryptedSidecar, SidecarError> {
        let page_size = self.cipher.profile.page_size;
        let header = wal.get(..WAL_HEADER_LEN).ok_or(SidecarError::Malformed)?;
        let endian = WalChecksumEndian::from_magic(crate::be_u32(header, 0))
            .ok_or(SidecarError::Malformed)?;
        let declared = crate::be_u32(header, 8);
        if usize::try_from(declared).ok() != Some(page_size) {
            return Err(SidecarError::PageSizeMismatch {
                database: self.page_size,
                wal: declared,
            });
        }
        let salts = &header[16..24];
        let seed = (crate::be_u32(header, 24), crate::be_u32(header, 28));
        let mut encrypted_chain = Some(seed);
        let mut plaintext_chain = seed;
        let mut plaintext = Vec::with_capacity(wal.len());
        plaintext.extend_from_slice(header);
        // A zero page number ends the log, as it does for every reader.
        let mut frames = wal[WAL_HEADER_LEN..]
            .chunks_exact(WAL_FRAME_HEADER_LEN + page_size)
            .take_while(|frame| crate::be_u32(frame, 0) != 0);
        while let Some(frame) = frames.next() {
            let (frame_header, page) = frame.split_at(WAL_FRAME_HEADER_LEN);
            encrypted_chain = encrypted_chain
                .filter(|_| &frame_header[8..16] == salts)
                .map(|chain| frame_checksum(endian, chain, frame_header, page))
                .filter(|&chain| chain == stored_frame_checksum(frame_header));
            let Some(page) = self
                .cipher
                .decrypt_page(page, crate::be_u32(frame_header, 0))
            else {
                return Ok(DecryptedSidecar {
                    plaintext,
                    omitted: 1 + frames.count(),
                });
            };
            let checksum = frame_checksum(endian, plaintext_chain, frame_header, &page);
            let checksum = if encrypted_chain.is_some() {
                plaintext_chain = checksum;
                checksum
            } else {
                (checksum.0 ^ 1, checksum.1)
            };
            plaintext.extend_from_slice(&frame_header[..16]);
            plaintext.extend_from_slice(&checksum.0.to_be_bytes());
            plaintext.extend_from_slice(&checksum.1.to_be_bytes());
            plaintext.extend_from_slice(&page);
        }
        Ok(DecryptedSidecar {
            plaintext,
            omitted: 0,
        })
    }

    /// Decrypt the database's rollback `-journal` with the keys that decrypted
    /// the database.
    ///
    /// The first segment is read where [`crate::RollbackJournal::parse`] reads
    /// it: behind an intact header, up to the record count it declares; behind
    /// a header `journal_mode=PERSIST` zeroed on commit, from the sector the
    /// parser's page-number scoring picks. Page numbers are not encrypted, so
    /// that choice is the same before and after decryption. Behind an intact
    /// header a record's checksum is recomputed over the plaintext page when it
    /// verified over the ciphertext, and left wrong when it did not; a zeroed
    /// header took the checksum nonce with it, so those records keep theirs.
    ///
    /// # Errors
    /// [`SidecarError::Malformed`] when the journal is shorter than its header.
    pub fn decrypt_rollback_journal(
        &self,
        journal: &[u8],
    ) -> Result<DecryptedSidecar, SidecarError> {
        if journal.len() < JOURNAL_HEADER_LEN {
            return Err(SidecarError::Malformed);
        }
        let page_size = self.cipher.profile.page_size;
        let (start, nonce, limit) = if journal.starts_with(&crate::JOURNAL_MAGIC) {
            let declared = crate::be_u32(journal, 8);
            let limit = if declared == 0 || declared == u32::MAX {
                crate::MAX_JOURNAL_RECORDS
            } else {
                declared as usize
            };
            let sector = crate::be_u32(journal, 20).max(1) as usize;
            (sector, Some(crate::be_u32(journal, 12)), limit)
        } else {
            let sector = zeroed_journal_sector(journal, self.page_size);
            (sector, None, crate::MAX_JOURNAL_RECORDS)
        };
        let Some(body) = journal.get(start..) else {
            return Ok(DecryptedSidecar {
                plaintext: journal.to_vec(),
                omitted: 0,
            });
        };
        let mut plaintext = Vec::with_capacity(journal.len());
        plaintext.extend_from_slice(&journal[..start]);
        let mut records = body
            .chunks_exact(page_size + JOURNAL_RECORD_OVERHEAD)
            .take(limit)
            .take_while(|record| crate::be_u32(record, 0) != 0);
        while let Some(record) = records.next() {
            let (number, rest) = record.split_at(4);
            let (page, _) = rest.split_at(page_size);
            let stored = crate::be_u32(rest, page_size);
            let Some(plain) = self.cipher.decrypt_page(page, crate::be_u32(number, 0)) else {
                return Ok(DecryptedSidecar {
                    plaintext,
                    omitted: 1 + records.count(),
                });
            };
            let checksum = nonce.map_or(stored, |nonce| {
                let checksum = crate::journal_cksum(nonce, &plain);
                if crate::journal_cksum(nonce, page) == stored {
                    checksum
                } else {
                    checksum ^ 1
                }
            });
            plaintext.extend_from_slice(number);
            plaintext.extend_from_slice(&plain);
            plaintext.extend_from_slice(&checksum.to_be_bytes());
        }
        Ok(DecryptedSidecar {
            plaintext,
            omitted: 0,
        })
    }
}

/// The cumulative WAL checksum after one frame: the first 8 bytes of its
/// header, then its page.
fn frame_checksum(
    endian: WalChecksumEndian,
    chain: (u32, u32),
    frame_header: &[u8],
    page: &[u8],
) -> (u32, u32) {
    let (first, second) = crate::wal_checksum(endian, chain.0, chain.1, &frame_header[..8]);
    crate::wal_checksum(endian, first, second, page)
}

/// The checksum a WAL frame header stores (always big-endian).
fn stored_frame_checksum(frame_header: &[u8]) -> (u32, u32) {
    (
        crate::be_u32(frame_header, 16),
        crate::be_u32(frame_header, 20),
    )
}

/// The sector a journal whose header was zeroed on commit keeps its records
/// at, chosen as [`crate::RollbackJournal::parse`] chooses it.
fn zeroed_journal_sector(journal: &[u8], page_size: u32) -> usize {
    let page_bound = u32::try_from(journal.len() / (page_size as usize).max(1)).unwrap_or(u32::MAX);
    let mut best: Option<(usize, u32)> = None;
    for candidate in crate::SECTOR_CANDIDATES {
        let sector = if candidate == 0 { page_size } else { candidate };
        let records = crate::walk_journal_records(
            journal,
            sector as usize,
            page_size as usize,
            None,
            0,
            crate::MAX_JOURNAL_RECORDS,
        );
        let score = crate::score_journal_candidate(&records, page_bound);
        if score > 0 && best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, sector));
        }
    }
    best.map_or(page_size, |(_, sector)| sector) as usize
}

#[cfg(test)]
mod tests;
