//! Opening a volume and reading its plaintext.

use rayon::prelude::*;
use zeroize::Zeroizing;

use crate::cipher::CipherChain;
use crate::error::{Result, VeraError};
use crate::header::{Flavor, Header, FLAG_IN_PLACE, FLAG_SYSTEM, HEADER_SECTOR_LEN};
use crate::kdf::{self, Kdf, Prf};
use crate::layout::{self, Geometry, HeaderKind, HeaderSelection, SystemRange};
use crate::search::{self, Candidate, Control};
use crate::source::ReadAt;
use crate::xts::{Xts, DATA_UNIT};

/// Argon2id passes an automatic search still tries: PIM 31, 13 passes over
/// 1 GiB. Higher PIMs only run when Argon2id is asked for by name.
pub const AUTO_ARGON2_MAX_PASSES: u32 = 13;

/// Spans at least this long decrypt on several threads.
const PARALLEL_SPAN: usize = 256 * 1024;
/// Bytes each parallel decryption task takes.
const PARALLEL_CHUNK: usize = 64 * 1024;

/// The first eight bytes of an NTFS boot sector.
const NTFS_BOOT_SIGNATURE: [u8; 8] = [0xEB, 0x52, 0x90, b'N', b'T', b'F', b'S', b' '];

/// What to unlock with.
pub struct UnlockRequest<'a> {
    /// The password, already mixed with keyfiles (see
    /// [`crate::keyfile::KeyfilePool`]); at most 128 bytes, or exactly the
    /// keyfile pool.
    pub password: &'a [u8],
    /// Personal Iterations Multiplier; 0 for VeraCrypt's default.
    pub pim: u32,
    /// Only this key derivation; every one when `None`.
    pub prf: Option<Prf>,
    /// Which headers to read.
    pub headers: HeaderSelection,
}

/// The facts of an unlocked volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// VeraCrypt or TrueCrypt.
    pub flavor: Flavor,
    /// The header that opened.
    pub header: HeaderKind,
    /// The derivation that opened it.
    pub kdf: Kdf,
    /// The encryption algorithm.
    pub chain: CipherChain,
    /// Header format version.
    pub header_version: u16,
    /// The oldest program version that opens the volume.
    pub required_program_version: u16,
    /// The volume's sector size.
    pub sector_size: u32,
    /// Header flags.
    pub flags: u32,
    /// TrueCrypt's volume creation time (Windows FILETIME); zero in VeraCrypt.
    pub volume_created: u64,
    /// TrueCrypt's header creation time (Windows FILETIME); zero in VeraCrypt.
    pub header_created: u64,
    /// Volume size from the header.
    pub volume_size: u64,
    /// Hidden volume size from the header (non-zero in hidden volumes).
    pub hidden_volume_size: u64,
    /// Encrypted area, from the start of the source.
    pub encrypted_area_start: u64,
    /// Encrypted area length.
    pub encrypted_area_length: u64,
    /// Where the plaintext view starts in the source.
    pub view_start: u64,
    /// Plaintext bytes.
    pub size: u64,
    /// Whether the whole volume is encrypted; a system volume whose
    /// encryption was paused reads its unencrypted tail as stored.
    pub encryption_complete: bool,
    /// The primary and secondary master keys are equal (VeraCrypt warns
    /// about such volumes).
    pub weak_master_key: bool,
    /// The first sector holds a plaintext NTFS boot sector, which VeraCrypt
    /// passes through on system partitions (Windows repair rewrites it).
    pub plaintext_boot_sector: bool,
}

impl VolumeInfo {
    /// Whether the header is a system encryption header.
    #[must_use]
    pub fn system(&self) -> bool {
        self.header == HeaderKind::System || self.flags & FLAG_SYSTEM != 0
    }

    /// Whether non-system in-place encryption made the volume.
    #[must_use]
    pub fn in_place(&self) -> bool {
        self.flags & FLAG_IN_PLACE != 0
    }
}

/// An unlocked volume over its source. Reads take `&self` and may run from
/// several threads at once.
pub struct Volume<S> {
    source: S,
    xts: Xts,
    geometry: Geometry,
    plaintext_sector: Option<(u64, u64)>,
    master_key: Zeroizing<Vec<u8>>,
    info: VolumeInfo,
}

/// Unlock the volume `source` holds.
///
/// # Errors
/// [`VeraError::AuthenticationFailed`] when no header opens,
/// [`VeraError::Cancelled`], [`VeraError::TooSmall`] when no requested
/// header fits the source, and the header or geometry errors of the header
/// that opened.
pub fn unlock<S: ReadAt>(
    source: S,
    request: &UnlockRequest,
    control: &Control,
) -> Result<Volume<S>> {
    if request.password.len() > crate::keyfile::MAX_PASSWORD_LEN {
        return Err(VeraError::InvalidRequest(format!(
            "VeraCrypt passwords are at most {} bytes",
            crate::keyfile::MAX_PASSWORD_LEN
        )));
    }
    if request.password.is_empty() {
        return Err(VeraError::InvalidRequest("no password or keyfile".into()));
    }
    let size = source.size();
    let mut candidates = Vec::new();
    let mut any_header_fits = false;
    for &kind in request.headers.kinds() {
        let Some(offset) = kind.offset(size).filter(|offset| {
            offset
                .checked_add(HEADER_SECTOR_LEN as u64)
                .is_some_and(|end| end <= size)
        }) else {
            continue;
        };
        any_header_fits = true;
        let mut kdfs = kdf::candidates(
            kind.kdf_target(),
            request.pim,
            request.prf,
            AUTO_ARGON2_MAX_PASSES,
        )?;
        if kind.truecrypt_only() {
            kdfs.retain(|kdf| kdf.family == kdf::Family::TrueCrypt);
        }
        if kdfs.is_empty() {
            continue;
        }
        let mut sector = [0u8; HEADER_SECTOR_LEN];
        source.read_exact_at(offset, &mut sector)?;
        candidates.push(Candidate { kind, sector, kdfs });
    }
    if !any_header_fits {
        let what = match request.headers {
            HeaderSelection::System(_) => "system encryption",
            HeaderSelection::Backup => "backup",
            HeaderSelection::Auto | HeaderSelection::Normal | HeaderSelection::Hidden => "volume",
        };
        return Err(VeraError::TooSmall { what, len: size });
    }
    let found = search::search(&candidates, request.password, control)?;
    let system = match request.headers {
        HeaderSelection::System(range) => Some(range),
        HeaderSelection::Auto
        | HeaderSelection::Normal
        | HeaderSelection::Hidden
        | HeaderSelection::Backup => None,
    };
    Volume::open(
        source,
        found.kind,
        found.kdf,
        found.chain,
        &found.header,
        system,
    )
}

impl<S: ReadAt> Volume<S> {
    fn open(
        source: S,
        kind: HeaderKind,
        kdf: Kdf,
        chain: CipherChain,
        header: &Header,
        system: Option<SystemRange>,
    ) -> Result<Self> {
        let geometry = layout::geometry(kind, header, source.size(), system)?;
        let key_len = chain.key_len();
        let master_key = Zeroizing::new(header.master_keys[..key_len].to_vec());
        let xts = Xts::new(chain, &master_key)?;
        let half = key_len / 2;
        let weak_master_key = master_key[..half] == master_key[half..];
        let mut plaintext_sector = None;
        if let Some(candidate) = geometry.boot_sector_candidate {
            let mut first = [0u8; 8];
            source.read_exact_at(candidate, &mut first)?;
            if first == NTFS_BOOT_SIGNATURE {
                plaintext_sector = Some((candidate, candidate + u64::from(header.sector_size)));
            }
        }
        let info = VolumeInfo {
            flavor: header.flavor,
            header: kind,
            kdf,
            chain,
            header_version: header.version,
            required_program_version: header.required_program_version,
            sector_size: header.sector_size,
            flags: header.flags,
            volume_created: header.volume_created,
            header_created: header.header_created,
            volume_size: header.volume_size,
            hidden_volume_size: header.hidden_volume_size,
            encrypted_area_start: header.encrypted_area_start,
            encrypted_area_length: header.encrypted_area_length,
            view_start: geometry.view_start,
            size: geometry.view_len,
            encryption_complete: geometry.complete,
            weak_master_key,
            plaintext_boot_sector: plaintext_sector.is_some(),
        };
        Ok(Self {
            source,
            xts,
            geometry,
            plaintext_sector,
            master_key,
            info,
        })
    }

    /// The facts of the volume.
    #[must_use]
    pub fn info(&self) -> &VolumeInfo {
        &self.info
    }

    /// Plaintext bytes.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.geometry.view_len
    }

    /// The master key material of the cipher chain (sensitive).
    #[must_use]
    pub fn master_key(&self) -> &[u8] {
        &self.master_key
    }

    /// The source the volume reads.
    #[must_use]
    pub fn source(&self) -> &S {
        &self.source
    }

    /// Fill `buf` with plaintext from `offset`.
    ///
    /// # Errors
    /// [`VeraError::InvalidRequest`] for a range past the end, and source
    /// read errors (a short read is never padded).
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|end| *end <= self.size())
            .ok_or_else(|| {
                VeraError::InvalidRequest(format!(
                    "read of {} bytes at {offset} leaves the {}-byte volume",
                    buf.len(),
                    self.size()
                ))
            })?;
        if buf.is_empty() {
            return Ok(());
        }
        let unit = DATA_UNIT as u64;
        let absolute = self.geometry.view_start + offset;
        let absolute_end = self.geometry.view_start + end;
        let first = absolute / unit * unit;
        let last = absolute_end.div_ceil(unit) * unit;
        if first == absolute && last == absolute_end {
            self.source.read_exact_at(first, buf)?;
            self.decrypt_span(buf, first);
            return Ok(());
        }
        let span = usize::try_from(last - first)
            .map_err(|_| VeraError::InvalidRequest("read too large".into()))?;
        let mut scratch = Zeroizing::new(vec![0u8; span]);
        self.source.read_exact_at(first, &mut scratch)?;
        self.decrypt_span(&mut scratch, first);
        let skip = (absolute - first) as usize;
        buf.copy_from_slice(&scratch[skip..skip + buf.len()]);
        Ok(())
    }

    /// Decrypt whole data units at absolute offset `start`, leaving units
    /// outside the encrypted area (and a plaintext boot sector) as stored.
    fn decrypt_span(&self, data: &mut [u8], start: u64) {
        let end = start + data.len() as u64;
        let mut from = start.max(self.geometry.encrypted_start);
        let to = end.min(self.geometry.encrypted_end);
        if let Some((skip_start, skip_end)) = self.plaintext_sector {
            if skip_start < to && skip_end > from {
                if from < skip_start {
                    self.decrypt_run(data, start, from, skip_start);
                }
                from = from.max(skip_end);
            }
        }
        if from < to {
            self.decrypt_run(data, start, from, to);
        }
    }

    fn decrypt_run(&self, data: &mut [u8], start: u64, from: u64, to: u64) {
        let run = &mut data[(from - start) as usize..(to - start) as usize];
        let first_unit = from / DATA_UNIT as u64;
        if run.len() < PARALLEL_SPAN {
            self.xts.decrypt(run, first_unit);
            return;
        }
        let units_per_chunk = (PARALLEL_CHUNK / DATA_UNIT) as u64;
        run.par_chunks_mut(PARALLEL_CHUNK)
            .enumerate()
            .for_each(|(index, chunk)| {
                self.xts
                    .decrypt(chunk, first_unit + index as u64 * units_per_chunk)
            });
    }
}
