/// Integrity hashes stored within the EWF image by the acquisition tool.
///
/// The `hash` section (present since `EnCase` 1) stores an MD5 of the acquired media.
/// The `digest` section (added in `EnCase` 6.12+) stores both MD5 and SHA-1.
/// When both sections are present, MD5 values should be identical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredHashes {
    /// MD5 hash of the acquired media (from `hash` or `digest` section).
    pub md5: Option<[u8; 16]>,
    /// SHA-1 hash of the acquired media (from `digest` section only).
    pub sha1: Option<[u8; 20]>,
}

/// Result of verifying the EWF image integrity by recomputing media hashes.
#[cfg(feature = "verify")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyResult {
    /// Computed MD5 of the full media stream.
    pub computed_md5: [u8; 16],
    /// Computed SHA-1 of the full media stream (only computed if a stored SHA-1 exists).
    pub computed_sha1: Option<[u8; 20]>,
    /// `Some(true)` if computed MD5 matches stored MD5, `Some(false)` if mismatch,
    /// `None` if no stored MD5 was present.
    pub md5_match: Option<bool>,
    /// `Some(true)` if computed SHA-1 matches stored SHA-1, `Some(false)` if mismatch,
    /// `None` if no stored SHA-1 was present.
    pub sha1_match: Option<bool>,
    /// Bytes the chunk table can actually address
    /// (`chunk_size x chunk_count`).
    pub addressable_bytes: u64,
    /// Bytes the volume section says the media contains.
    pub declared_bytes: u64,
}

#[cfg(feature = "verify")]
impl VerifyResult {
    /// Whether the chunk table covers the whole declared media.
    ///
    /// `false` means segments, or their `table` sections, are missing. EWF
    /// stores a segment's chunk table at the END of that segment, so a
    /// truncated segment loses the index for ALL of its chunks, not merely the
    /// bytes lost to truncation.
    ///
    /// This distinction is the difference between two very different findings:
    /// *"this image does not hash to its acquisition digest"* (a claim about the
    /// evidence) and *"this reader could only see part of the image"* (a claim
    /// about the read). A hash mismatch over an incomplete read must never be
    /// reported as the former.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.addressable_bytes >= self.declared_bytes
    }

    /// Bytes the chunk table cannot address; `0` when complete.
    #[must_use]
    pub fn missing_bytes(&self) -> u64 {
        self.declared_bytes.saturating_sub(self.addressable_bytes)
    }
}

/// Case and acquisition metadata extracted from EWF header sections.
///
/// Populated from the `header` section (ASCII, always present) or `header2` section
/// (UTF-16LE, `EnCase` 5+). Fields are `None` when the acquisition tool left them blank.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EwfMetadata {
    pub bytes_per_sector: Option<u32>,
    pub media_type: Option<u8>,
    pub compression: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub case_number: Option<String>,
    pub evidence_number: Option<String>,
    pub description: Option<String>,
    pub examiner: Option<String>,
    pub notes: Option<String>,
    pub acquiry_software: Option<String>,
    pub os_version: Option<String>,
    pub acquiry_date: Option<String>,
    pub system_date: Option<String>,
}

/// A range of sectors that had read errors during acquisition.
///
/// Extracted from the `error2` section, which records bad sectors encountered
/// by the imaging tool. Clean acquisitions have no entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquisitionError {
    /// First sector in the error range.
    pub first_sector: u32,
    /// Number of consecutive sectors in the error range.
    pub sector_count: u32,
}
