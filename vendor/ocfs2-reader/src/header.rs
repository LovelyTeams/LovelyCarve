//! Superblock positions and prefix fields adapted from unearth's MIT detector.
//! Geometry, feature policy, whole-block CRC and identities are checked here.
use crate::{exact, field, le16, le32, le64, require};
use lamfold::{BlockSource, FoldError, Result};

#[derive(Clone, Copy)]
pub(crate) enum Record {
    Inode,
    Extent,
    Group,
    IndexRoot,
    IndexLeaf,
    Xattr,
    Refcount,
}
impl Record {
    pub fn layout(self) -> (&'static [u8], usize, usize, usize) {
        match self {
            Self::Inode => (b"INODE01\0", 80, 96, 128),
            Self::Extent => (b"EXBLK01\0", 24, 20, 8),
            Self::Group => (b"GROUP01\0", 40, 16, 48),
            Self::IndexRoot => (b"DXDIR01\0", 24, 20, 8),
            Self::IndexLeaf => (b"DXLEAF1\0", 16, 24, 8),
            Self::Xattr => (b"XATTR01\0", 16, 12, 24),
            Self::Refcount => (b"REFCNT1\0", 16, 12, 32),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Header {
    pub block_size: u32,
    pub cluster_size: u32,
    pub clusters: u32,
    pub generation: u32,
    pub root: u64,
    pub system: u64,
    pub slots: u16,
    pub compatible: u32,
    pub incompatible: u32,
    pub readonly_compatible: u32,
    pub first_cluster_group: u64,
    pub label: String,
    pub uuid: [u8; 16],
    pub uuid_hash: u32,
    pub directory_seed: [u32; 3],
    pub(crate) blocks: u64,
}
impl Header {
    pub fn inspect(source: &mut impl BlockSource) -> Result<Self> {
        let mut found = None;
        for block_size in [512, 1024, 2048, 4096] {
            if source.len() < block_size as u64 * 3 {
                continue;
            }
            let mut raw = vec![0; block_size as usize];
            exact(source, block_size as u64 * 2, &mut raw)?;
            if raw.get(..7) != Some(b"OCFSV2\0") {
                continue;
            }
            let header = Self::inspect_block(&raw, block_size, source.len())?;
            require(found.is_none(), "ocfs2: ambiguous superblock geometries")?;
            found = Some(header);
        }
        found.ok_or(FoldError::Unsupported("ocfs2: no supported superblock"))
    }
    /// `raw` is the full block #2 for `block_size`, not a shortened prefix.
    pub fn inspect_block(raw: &[u8], block_size: u32, source_length: u64) -> Result<Self> {
        require(
            matches!(block_size, 512 | 1024 | 2048 | 4096) && raw.len() == block_size as usize,
            "ocfs2: superblock buffer/block size",
        )?;
        require(
            raw.get(..7) == Some(b"OCFSV2\0")
                && le64(raw, 80)? == 2
                && le32(raw, 44)? & 0x21 == 0x21,
            "ocfs2: superblock identity",
        )?;
        if le16(raw, 192)? != 0 || le16(raw, 194)? != 90 {
            return Err(FoldError::Unsupported(
                "ocfs2: revision (OCFS1 is separate)",
            ));
        }
        require(
            le32(raw, 248)? == block_size.trailing_zeros(),
            "ocfs2: block-size shift",
        )?;
        let cluster_shift = le32(raw, 252)?;
        require(
            (12..=20).contains(&cluster_shift),
            "ocfs2: cluster-size shift",
        )?;
        let cluster_size = 1u32 << cluster_shift;
        require(
            cluster_size >= block_size,
            "ocfs2: cluster smaller than block",
        )?;
        let clusters = le32(raw, 20)?;
        let length = clusters as u64 * cluster_size as u64;
        require(
            clusters != 0 && length <= source_length && length >= block_size as u64 * 4,
            "ocfs2: volume source geometry",
        )?;
        let incompatible = le32(raw, 224)?;
        let readonly_compatible = le32(raw, 228)?;
        if incompatible & !0xffd8 != 0
            || readonly_compatible & !7 != 0
            || le16(raw, 200)? != 0
            || le16(raw, 258)? != 0
        {
            return Err(FoldError::Unsupported(
                "ocfs2: unsupported features, heartbeat-only, resize/tunefs or error state",
            ));
        }
        let blocks = length / block_size as u64;
        let root = le64(raw, 232)?;
        let system = le64(raw, 240)?;
        require(
            root > 2 && root < blocks && system > 2 && system < blocks && root != system,
            "ocfs2: root/system inode references",
        )?;
        let slots = le16(raw, 256)?;
        require((1..=255).contains(&slots), "ocfs2: slot count")?;
        let label = &raw[272..336];
        let length = label
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(label.len());
        let label = String::from_utf8(label[..length].to_vec())
            .map_err(|_| FoldError::Unsupported("ocfs2: non-UTF-8 volume label"))?;
        let header = Self {
            block_size,
            cluster_size,
            clusters,
            generation: le32(raw, 96)?,
            root,
            system,
            slots,
            compatible: le32(raw, 220)?,
            incompatible,
            readonly_compatible,
            first_cluster_group: le64(raw, 264)?,
            label,
            uuid: field(raw, 336)?,
            uuid_hash: le32(raw, 260)?,
            directory_seed: [le32(raw, 380)?, le32(raw, 384)?, le32(raw, 388)?],
            blocks,
        };
        header.verify(raw, 128)?;
        header.block_offset(header.first_cluster_group)?;
        Ok(header)
    }
    pub fn has_metadata_crc(&self) -> bool {
        self.incompatible & 0x0800 != 0
    }
    /// Validate original bytes. CRC mismatch is an error even when native ECC
    /// might repair a bit; this reader never edits or silently repairs evidence.
    pub(crate) fn verify(&self, raw: &[u8], check: usize) -> Result<()> {
        if !self.has_metadata_crc() {
            return Ok(());
        }
        let expected = le32(raw, check)?;
        let mut original = raw.to_vec();
        let check = original
            .get_mut(check..check + 8)
            .ok_or(FoldError::Corrupt("ocfs2: checksum field bounds"))?;
        check.fill(0);
        require(
            !crc32fast::hash(&original) == expected,
            "ocfs2: metadata CRC mismatch (ECC repair not applied)",
        )
    }
    pub(crate) fn block_offset(&self, block: u64) -> Result<u64> {
        require(
            block > 2 && block < self.blocks,
            "ocfs2: block outside volume data span",
        )?;
        Ok(block * self.block_size as u64)
    }
    pub(crate) fn read_block(&self, source: &mut impl BlockSource, block: u64) -> Result<Vec<u8>> {
        let mut raw = vec![0; self.block_size as usize];
        exact(source, self.block_offset(block)?, &mut raw)?;
        Ok(raw)
    }
    pub(crate) fn record(
        &self,
        source: &mut impl BlockSource,
        block: u64,
        record: Record,
    ) -> Result<Vec<u8>> {
        let raw = self.read_block(source, block)?;
        let (signature, identity, generation, check) = record.layout();
        require(
            raw.get(..8) == Some(signature)
                && le64(&raw, identity)? == block
                && le32(&raw, generation)? == self.generation,
            "ocfs2: metadata signature/address/generation",
        )?;
        self.verify(&raw, check)?;
        Ok(raw)
    }
}
