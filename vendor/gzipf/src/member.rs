/// Metadata and source geometry of one fully validated GZIP member.
/// Names/comments retain their exact Latin-1 bytes, excluding the terminating
/// NUL. `None` and an explicitly present empty field remain distinct.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub header_offset: u64,
    pub compressed_data_offset: u64,
    pub compressed_data_size: u64,
    pub trailer_offset: u64,
    pub end_offset: u64,
    pub uncompressed_offset: u64,
    pub uncompressed_size: u64,
    pub flags: u8,
    /// POSIX seconds UTC; zero means no timestamp was recorded.
    pub modification_time: u32,
    pub extra_flags: u8,
    /// Preserve unknown OS codes instead of guessing the producing platform.
    pub operating_system: u8,
    /// Opaque XLEN bytes; application-specific extra subfields are not decoded.
    pub extra: Option<Vec<u8>>,
    pub name: Option<Vec<u8>>,
    pub comment: Option<Vec<u8>>,
    pub header_crc16: Option<u16>,
    pub crc32: u32,
    /// Trailer length modulo 2^32, independently checked against actual output.
    pub isize: u32,
}

impl Member {
    /// Decode RFC 1952's ISO-8859-1 name without UTF-8 replacement characters.
    pub fn name_utf8(&self) -> Option<String> {
        self.name
            .as_ref()
            .map(|bytes| bytes.iter().copied().map(char::from).collect())
    }

    pub fn name_utf16(&self) -> Option<Vec<u16>> {
        self.name
            .as_ref()
            .map(|bytes| bytes.iter().copied().map(u16::from).collect())
    }

    pub fn comment_utf8(&self) -> Option<String> {
        self.comment
            .as_ref()
            .map(|bytes| bytes.iter().copied().map(char::from).collect())
    }

    pub fn comment_utf16(&self) -> Option<Vec<u16>> {
        self.comment
            .as_ref()
            .map(|bytes| bytes.iter().copied().map(u16::from).collect())
    }

    pub fn is_text(&self) -> bool {
        self.flags & 1 != 0
    }
}
