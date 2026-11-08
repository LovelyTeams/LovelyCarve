use std::io::Read;
use std::sync::Arc;
use std::{fmt::Debug, path::Path};

use byteorder::{ByteOrder, LittleEndian};
use memmap2::Mmap;
use tracing::instrument;

use crate::array::{Array, RealmRef};
use crate::traits::Node;
use crate::{Group, RealmFileError};

/// The header for a Realm file.
#[derive(Clone, Copy)]
pub(crate) struct Header {
    /// The two possible top references. Each element points to a [`crate::group::Group`].
    /// The least significant bit of flags indicates which of the two references is active.
    top_ref: [u64; 2],
    magic: [u8; 4],
    fmt_ver: [u8; 2],
    _reserved: u8,
    flags: u8,
}

impl Debug for Header {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Header")
            .field("top_ref", &self.top_ref)
            .field("fmt_ver", &self.fmt_ver)
            .field("flags", &self.flags)
            .finish()
    }
}

impl Header {
    const SIZE: usize = 24;
    const MAGIC: [u8; 4] = *b"T-DB";

    fn parse(buf: &[u8]) -> crate::RealmResult<Self> {
        if buf.len() < Self::SIZE {
            return Err(RealmFileError::InvalidRealmFile {
                reason: "file too small for Realm header".to_string(),
            });
        }

        let h = Header {
            top_ref: [
                LittleEndian::read_u64(&buf[0..8]),
                LittleEndian::read_u64(&buf[8..16]),
            ],
            magic: buf[16..20].try_into().unwrap(),
            fmt_ver: buf[20..22].try_into().unwrap(),
            _reserved: buf[22],
            flags: buf[23],
        };
        if h.magic != Self::MAGIC {
            return Err(RealmFileError::InvalidRealmFile {
                reason: "not a Realm file (magic mismatch)".to_string(),
            });
        }

        Ok(h)
    }

    /// Choose the active top ref using the switch bit (bit 0 of `flags`).
    pub(crate) fn current_top_ref(&self) -> RealmRef {
        let idx = (self.flags & 1) as usize;
        RealmRef::new(self.top_ref[idx] as usize)
    }

    fn is_encrypted(&self) -> bool {
        self.flags & 0x80 != 0
    }

    fn file_format_version(&self) -> (u8, u8) {
        (self.fmt_ver[0], self.fmt_ver[1])
    }
}

/// The header for a single node in a Realm file.
#[derive(Clone, Copy)]
pub(crate) struct NodeHeader {
    pub(crate) flags: u8,
    pub(crate) size: u32, // 24-bit little-endian count
}

impl Debug for NodeHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeHeader")
            .field("is_inner_btree", &self.is_inner_bptree())
            .field("has_refs", &self.has_refs())
            .field("context_flag", &self.context_flag())
            .field("width", &self.width())
            .field("size", &self.size)
            .finish()
    }
}

impl NodeHeader {
    pub(crate) const SIZE: usize = 8;
    pub(crate) const DUMMY_CHECKSUM: u32 = 0x4141_4141;

    /// Parse a node header from a buffer.
    ///
    /// Returns an error if the buffer is too small.
    ///
    /// Panics if the checksum is invalid.
    pub(crate) fn parse(buf: &[u8]) -> crate::RealmResult<Self> {
        if buf.len() < Self::SIZE {
            return Err(RealmFileError::InvalidRealmFile {
                reason: format!("buffer is too small to contain a node ({})", buf.len()),
            });
        }

        let checksum = LittleEndian::read_u32(&buf[0..4]);
        let flags = buf[4];
        let size = ((buf[5] as u32) << 16) | ((buf[6] as u32) << 8) | (buf[7] as u32);

        if checksum != Self::DUMMY_CHECKSUM {
            return Err(RealmFileError::InvalidRealmFile {
                reason: "invalid checksum".to_string(),
            });
        }

        Ok(Self { flags, size })
    }

    /// Returns true if the node is an inner B+Tree node.
    pub(crate) fn is_inner_bptree(&self) -> bool {
        self.flags & 0x80 != 0
    }

    /// Returns true if the node has references.
    pub(crate) fn has_refs(&self) -> bool {
        self.flags & 0x40 != 0
    }

    /// Returns true if the context flag is set.
    ///
    /// The meaning of this value varies by context.
    pub(crate) fn context_flag(&self) -> bool {
        self.flags & 0x20 != 0
    }

    #[inline]
    fn width_scheme(&self) -> u8 {
        (self.flags & 0x18) >> 3
    }

    #[inline]
    pub(crate) fn width(&self) -> u8 {
        (1 << (self.flags & 0x07)) >> 1
    }

    pub(crate) fn payload_len(&self) -> usize {
        let width = self.width() as u32;
        let num_bytes = match self.width_scheme() {
            0 => {
                // Current assumption is that size is at most 2^24 and that width is at most 64.
                // In that case the following will never overflow. (Assuming that size_t is at least 32 bits)
                assert!(self.size < 0x1000000);
                let num_bits = self.size * width;
                (num_bits + 7) >> 3
            }
            1 => self.size * width,
            2 => self.size,
            _ => {
                unreachable!("invalid width scheme");
            }
        };

        // Ensure 8-byte alignment
        // ((num_bytes + 7) & !7) as usize
        num_bytes as usize
    }
}

/// A reference to a Realm database.
pub struct Realm {
    data: RealmData,
    pub(crate) hdr: Header,
}

impl Debug for Realm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Realm").field("hdr", &self.hdr).finish()
    }
}

enum RealmData {
    Mmap(Mmap),
    Bytes(Arc<[u8]>),
}

impl RealmData {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Mmap(mmap) => mmap,
            Self::Bytes(bytes) => bytes,
        }
    }
}

impl Realm {
    /// Open a Realm database.
    ///
    /// Returns an error if:
    /// - The header is invalid.
    /// - The file is encrypted.
    /// - The file format version is not supported.
    #[instrument(level = "debug")]
    pub fn open(path: impl AsRef<Path> + Debug) -> crate::RealmResult<Self> {
        let file = std::fs::File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        Self::from_data(RealmData::Mmap(mmap))
    }

    /// Open a Realm database from in-memory bytes.
    ///
    /// This is useful when the Realm file has already been materialised by a
    /// caller, for example from an archive, forensic image, or other virtual
    /// filesystem where no stable host path exists.
    #[instrument(level = "debug", skip(bytes))]
    pub fn from_bytes(bytes: Vec<u8>) -> crate::RealmResult<Self> {
        Self::from_arc_bytes(Arc::from(bytes.into_boxed_slice()))
    }

    /// Open a Realm database from shared in-memory bytes without copying.
    ///
    /// The bytes must remain immutable for the lifetime of the returned
    /// [`Realm`]. `Arc<[u8]>` makes that ownership explicit while allowing
    /// callers to cache the same materialised file across paged reads.
    #[instrument(level = "debug", skip(bytes))]
    pub fn from_arc_bytes(bytes: Arc<[u8]>) -> crate::RealmResult<Self> {
        Self::from_data(RealmData::Bytes(bytes))
    }

    /// Read a Realm database from any byte stream into memory and open it.
    ///
    /// The reader is fully consumed. Use [`Realm::from_bytes`] when the bytes
    /// are already available as a `Vec<u8>`.
    #[instrument(level = "debug", skip(reader))]
    pub fn from_reader(mut reader: impl Read) -> crate::RealmResult<Self> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Self::from_bytes(bytes)
    }

    fn from_data(data: RealmData) -> crate::RealmResult<Self> {
        let hdr = Header::parse(data.as_slice())?;

        if hdr.is_encrypted() {
            return Err(RealmFileError::Unsupported {
                reason: "Encrypted Realm files are not supported".to_string(),
            });
        }

        if hdr.file_format_version() != (9, 9) {
            return Err(RealmFileError::Unsupported {
                reason: format!(
                    "Unsupported Realm format version ({}.{}, supported: 9.9)",
                    hdr.file_format_version().0,
                    hdr.file_format_version().1,
                ),
            });
        }

        Ok(Realm { data, hdr })
    }

    pub(crate) fn slice(&self, ref_: RealmRef, len: usize) -> &[u8] {
        let o = ref_.to_offset();
        let data = self.data.as_slice();
        if o + len > data.len() {
            panic!("offset 0x{o:X} outside file");
        }
        &data[o..o + len]
    }

    pub(crate) fn payload(&self, ref_: RealmRef, payload_len: usize) -> &[u8] {
        let payload_offset = ref_ + NodeHeader::SIZE;
        self.slice(payload_offset, payload_len)
    }

    pub(crate) fn header(&self, ref_: RealmRef) -> crate::RealmResult<NodeHeader> {
        let bytes = self.slice(ref_, NodeHeader::SIZE);
        NodeHeader::parse(bytes)
    }

    pub(crate) fn top_ref(&self) -> RealmRef {
        self.hdr.current_top_ref()
    }

    /// Create a reference to the [`Group`] in this Realm database. The
    /// [`Group`] is the main entrypoint for interacting with the tables.
    pub fn into_group(self) -> crate::RealmResult<Group> {
        let ref_ = self.top_ref();
        let realm = Arc::new(self);
        let array = Array::from_ref(Arc::clone(&realm), ref_)?;

        Group::build(array)
    }
}

#[derive(Clone)]
pub(crate) struct RealmNode {
    pub(crate) realm: Arc<Realm>,
    pub(crate) ref_: RealmRef,
    pub(crate) header: NodeHeader,
    cached_payload_len: usize,
}

impl Debug for RealmNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let payload = self.payload();

        f.debug_struct("RealmNode")
            .field("ref_", &self.ref_)
            .field("header", &self.header)
            .field(
                "payload",
                &format!(
                    "<{} byte{}>",
                    payload.len(),
                    if payload.len() == 1 { "" } else { "s" }
                ),
            )
            .finish()
    }
}

impl Node for RealmNode {
    fn from_ref(realm: Arc<Realm>, ref_: RealmRef) -> crate::RealmResult<Self> {
        let header = realm.header(ref_)?;
        let cached_payload_len = header.payload_len();

        Ok(Self {
            realm,
            ref_,
            header,
            cached_payload_len,
        })
    }
}

impl RealmNode {
    pub(crate) fn payload(&self) -> &[u8] {
        self.realm.payload(self.ref_, self.cached_payload_len)
    }
}

#[cfg(test)]
mod tests {
    use crate::realm::NodeHeader;

    #[test]
    fn test_node_header() {
        // let bytes = 0x41414141_02000002_0A000000_00000000u128.to_be_bytes();
        let bytes = [0x41, 0x41, 0x41, 0x41, 0b10, 0x00, 0x00, 0x02];
        dbg!(&bytes);
        let header = NodeHeader::parse(&bytes).unwrap();

        dbg!(&header);
        eprintln!("flags: {:08b}", header.flags);

        assert!(!header.is_inner_bptree());
        assert!(!header.has_refs());
        assert!(!header.context_flag());
        assert!(header.width_scheme() == 0);
        eprintln!("element width: {}", header.width());
        assert_eq!(header.width(), 2);
        assert_eq!(header.size, 2);
        eprintln!("payload length: {}", header.payload_len());
        // 10 bits -> 2 bytes -> align to 8
        // assert!(header.payload_len() == 8);

        // let bytes = 0x41414141_4600000Au64.to_be_bytes();
        let bytes = [0x41, 0x41, 0x41, 0x41, 0b01000110, 0x00, 0x00, 0x0A];
        dbg!(&bytes);
        let header = NodeHeader::parse(&bytes).unwrap();

        dbg!(&header);
        eprintln!(
            "flags: {:08b} width_ndx: {} width_scheme: {}",
            header.flags,
            header.width(),
            header.width_scheme()
        );

        assert!(!header.is_inner_bptree());
        assert!(header.has_refs());
        assert!(!header.context_flag());
        assert_eq!(header.width_scheme(), 0);
        eprintln!("element width: {}", header.width());
        assert_eq!(header.width(), 32);
        assert_eq!(header.size, 10);
        eprintln!("payload length: {}", header.payload_len());
        // 32 bits -> 4 bytes -> *10 = 40 -> align to 8
        assert_eq!(header.payload_len(), 40);
    }
}
