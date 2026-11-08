//! EROFS reader, clean-roomed from the public on-disk format (kernel
//! `erofs_fs.h` is SPDX MIT).
//!
//! Layout: a 128-byte superblock at byte 1024; inodes addressed by `nid`
//! (`inode = (meta_blkaddr << blkszbits) + nid*32`), in a 32-byte compact or
//! 64-byte extended form; file/dir data in `FLAT_PLAIN` (contiguous at a block
//! address) or `FLAT_INLINE` (whole blocks at a block address, the final partial
//! block packed in-line right after the inode). Directory data is an array of
//! 12-byte dirents followed by their names.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use lamfold::{
    checked_block_len, checked_full_read_len, decode, lz4_block_with_dict, microlzma_block_decode,
    BlockSource, Codec, DirEntry, FileKind, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

const SUPER_OFFSET: u64 = 1024;
const MAGIC: u32 = 0xE0F5_E1E2;

// Superblock fields read at open, as offsets into the 128-byte superblock.
const SB_FEATURE_INCOMPAT: usize = 80;
const SB_EXTRA_DEVICES: usize = 86;
const SB_PACKED_NID: usize = 96;

// `feature_incompat` bits. Three values carry two features each (0x2 is
// COMPR_CFGS and BIG_PCLUSTER, 0x8 DEVICE_TABLE and COMPR_HEAD2, 0x20 FRAGMENTS
// and DEDUPE), so for those the superblock fields decide: an extra device
// count for the device table, a packed inode for fragments.
const INCOMPAT_LZ4_0PADDING: u32 = 0x0001;
const INCOMPAT_48BIT: u32 = 0x0080;
const INCOMPAT_METABOX: u32 = 0x0100;
const INCOMPAT_KNOWN: u32 = 0x01ff;

// datalayout (bits 1..=3 of i_format)
const FLAT_PLAIN: u8 = 0;
const FLAT_INLINE: u8 = 2;
/// Chunk-based (`mkfs.erofs --chunksize`, composefs): The data sits in chunks
/// of `block size << (format & CHUNK_BLKBITS_MASK)` bytes, located through a
/// table after the inode.
const CHUNK_BASED: u8 = 4;
const CHUNK_BLKBITS_MASK: u16 = 0x001f;
/// 8-byte chunk indexes rather than 4-byte block addresses.
const CHUNK_INDEXES: u16 = 0x0020;
const CHUNK_48BIT: u16 = 0x0040;
const CHUNK_FORMAT_ALL: u16 = 0x007f;
/// A chunk at this block address is a hole: It reads as zeros.
const NULL_BLKADDR: u32 = u32::MAX;
const COMPRESSED_FULL: u8 = 1;
// COMPRESSED_COMPACT (3) is the bit-packed `mkfs.erofs` default index.
const COMPRESSED_COMPACT: u8 = 3;

// z_erofs_map_header `h_advise` bits. COMPACTED_2B selects the 2-byte amortized
// packing within the compact index; clear ⇒ all-4B. BIG_PCLUSTER_{1,2} are set by
// default for deflate/zstd/lzma (a pcluster may span several blocks, and a NONHEAD
// carries a cblkcnt). INLINE_PCLUSTER (ztailpacking) and FRAGMENT change where the
// data lives and are refused until validated.
const ADVISE_COMPACTED_2B: u16 = 0x0001;
const ADVISE_BIG_PCLUSTER: u16 = 0x0002 | 0x0004;
const ADVISE_INLINE_PCLUSTER: u16 = 0x0008;
// Set with fragments: an uncompressed pcluster's data is rotated by its
// clusterofs rather than stored left-aligned.
const ADVISE_INTERLACED_PCLUSTER: u16 = 0x0010;
const ADVISE_FRAGMENT: u16 = 0x0020;
// `h_clusterbits` bit 7: the whole file lives in the packed inode, and the
// first four header bytes are its offset there.
const CLUSTERBITS_WHOLE_FILE_FRAGMENT: u8 = 0x80;

// Compact per-lcluster entry: `encodebits` = 2-bit type + `lobits` of value, where
// lobits is lclusterbits but never below 12 (the validated range is up to 12). A NONHEAD whose value has
// `CBLKCNT_FLAG` set encodes its pcluster's compressed-block count in the low bits.
const COMPACT_LOBITS: u32 = 12;
const COMPACT_ENCBITS: u32 = 14;
const COMPACT_CBLKCNT_FLAG: u16 = 0x0800;

// Head algorithm nibbles (`h_algorithmtype`).
const ALGO_LZ4: u8 = 0;
const ALGO_LZMA: u8 = 1;
const ALGO_DEFLATE: u8 = 2;
const ALGO_ZSTD: u8 = 3;

// The largest valid LZMA properties byte: (pb * 5 + lp) * 9 + lc with lc < 9,
// lp < 5 and pb < 5.
const LZMA_PROPS_MAX: u8 = 224;
// EROFS compression-config region: `available_compr_algs` bitmap, then the per-algo
// config blobs right after the 128-byte superblock.
const COMPR_ALGS_OFF: u64 = 1024 + 84;
/// The superblock is 128 bytes plus `sb_extslots` slots of 16; the configs
/// follow it.
const SB_EXTSLOTS: usize = 13;
const SB_BASE_SIZE: u64 = 128;
const SB_EXTSLOT_SIZE: u64 = 16;
const COMPR_ALG_LZMA_BIT: u16 = 0x0002;

// z_erofs compressed-cluster geometry. The 8-byte `z_erofs_map_header` sits at
// ALIGN(inode_end + xattr, 8); for the FULL (legacy) index the fixed-size
// `z_erofs_lcluster_index` array begins 16 bytes later (an 8-byte header plus 8
// reserved bytes — the legacy header size). Each entry's low 2 advise bits give
// the lcluster type.
const Z_EROFS_LEGACY_HEADER_SIZE: u64 = 16;
const LC_TYPE_PLAIN: u16 = 0;
const LC_TYPE_HEAD1: u16 = 1;
const LC_TYPE_NONHEAD: u16 = 2;
const LC_TYPE_HEAD2: u16 = 3;
// Full-index HEAD advise bit 14, added in Linux 7.2: The pcluster is a hole.
// No released erofs-utils writes it yet, so it's refused rather than guessed.
const LI_HOLE: u16 = 1 << 14;
// `lz4_max_distance` is ≤ 65535; a 64 KiB window covers every back-reference.
const LZ4_WINDOW: usize = 65_536;

struct ErofsInode {
    kind: FileKind,
    size: u64,
    mode: u16,
    layout: u8,
    raw_blkaddr: u32,
    inline_off: u64,
    modified: (i64, u32),
}

/// One pcluster, as the index walk yields it: the logical offset where its
/// decompressed output starts, its lcluster type (PLAIN/HEAD1/HEAD2), the absolute
/// block where its compressed data begins, and how many blocks that data spans
/// (1 unless big-pcluster).
struct Head {
    start: u64,
    ty: u16,
    blkaddr: u32,
    cblkcnt: u32,
}

/// A mounted EROFS volume.
struct VolumeSource<S> {
    source: S,
    length: u64,
}
impl<S: BlockSource> BlockSource for VolumeSource<S> {
    fn len(&self) -> u64 {
        self.length
    }
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(buffer.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(FoldError::Corrupt("erofs: read outside declared volume"));
        }
        self.source.read_at(offset, buffer)
    }
}

pub struct Erofs<S: BlockSource> {
    src: VolumeSource<S>,
    block_size: u64,
    blkszbits: u8,
    /// `LZ4_0PADDING`: an LZ4 stream starts at its pcluster's first non-zero
    /// byte, any bytes before it being padding.
    lz4_zero_padding: bool,
    meta_off: u64,
    root_nid: u64,
    /// inode cache keyed by nid (parse once).
    inodes: BTreeMap<u64, ErofsInode>,
    /// fully-decoded data for compressed inodes, keyed by nid. The LZ4 sliding
    /// dictionary makes forward, whole-file decode the natural unit; cache it so
    /// repeated block reads (and verification) don't re-decompress.
    decoded: BTreeMap<u64, Vec<u8>>,
    /// MicroLZMA window from `z_erofs_lzma_cfgs`, parsed once at open. `None` when
    /// the volume declares no LZMA or an unvalidated config `format`, which makes
    /// any LZMA-head inode surface a clean `Unsupported` rather than a guess.
    lzma_dict_size: Option<u32>,
    /// Where the compression configs start: after the superblock and its
    /// extension slots.
    compr_cfgs_off: u64,
    build_time: (i64, u32),
}

impl<S: BlockSource> Erofs<S> {
    fn parse_inode(&mut self, nid: u64) -> Result<()> {
        if self.inodes.contains_key(&nid) {
            return Ok(());
        }
        if self.inodes.len() >= 100_000 {
            return Err(FoldError::Corrupt("erofs: inode cache limit"));
        }
        // `nid` is an attacker-controlled `le_u64` straight from a dirent, so guard
        // the inode-offset arithmetic — an out-of-range nid must surface as Corrupt,
        // not panic (debug / overflow-checked builds) or wrap (release).
        let off = nid
            .checked_mul(32)
            .and_then(|x| self.meta_off.checked_add(x))
            .ok_or(FoldError::Corrupt("erofs: inode offset overflow"))?;
        let mut hdr = [0u8; 64];
        // compact inodes are 32 B; read 32 first, then the rest if extended.
        self.src.read_at(off, &mut hdr[..32])?;
        let format = le_u16(&hdr, 0)?;
        let extended = format & 1 != 0;
        let inode_size = if extended { 64 } else { 32 };
        if extended {
            self.src.read_at(off + 32, &mut hdr[32..64])?;
        }
        let xattr_icount = le_u16(&hdr, 2)?;
        let xattr_size = if xattr_icount == 0 {
            0u64
        } else {
            12 + (u64::from(xattr_icount) - 1) * 4
        };
        let mode = le_u16(&hdr, 4)?;
        let size = if extended {
            le_u64(&hdr, 8)?
        } else {
            u64::from(le_u32(&hdr, 8)?)
        };
        let raw_blkaddr = le_u32(&hdr, 16)?;
        let layout = ((format >> 1) & 7) as u8;
        let kind = match mode & 0xF000 {
            0x8000 => FileKind::Regular,
            0x4000 => FileKind::Directory,
            0xA000 => FileKind::Symlink,
            _ => FileKind::Other,
        };
        if kind != FileKind::Other && matches!(layout, FLAT_PLAIN | FLAT_INLINE) {
            let inline_off = off + inode_size + xattr_size;
            let allocated = if layout == FLAT_INLINE {
                size / self.block_size * self.block_size
            } else {
                size
            };
            if allocated > 0
                && u64::from(raw_blkaddr)
                    .checked_mul(self.block_size)
                    .and_then(|base| base.checked_add(allocated))
                    .is_none_or(|end| end > self.src.len())
            {
                return Err(FoldError::Corrupt(
                    "erofs: allocated file range outside volume",
                ));
            }
            if layout == FLAT_INLINE
                && inline_off % self.block_size + size % self.block_size > self.block_size
            {
                return Err(FoldError::Corrupt(
                    "erofs: inline tail crosses metadata block",
                ));
            }
        }
        self.inodes.insert(
            nid,
            ErofsInode {
                kind,
                size,
                mode,
                layout,
                raw_blkaddr,
                inline_off: off + inode_size + xattr_size,
                modified: if extended {
                    (le_u64(&hdr, 32)? as i64, le_u32(&hdr, 40)?)
                } else {
                    self.build_time
                },
            },
        );
        Ok(())
    }

    fn inode(&self, nid: u64) -> Result<&ErofsInode> {
        self.inodes.get(&nid).ok_or(FoldError::NotFound)
    }

    /// Read `buf.len()` bytes of an inode's data starting at logical `off`,
    /// resolving the FLAT_PLAIN / FLAT_INLINE split. Returns bytes read (clamped
    /// to the inode size). Does *not* verify — callers that surface file data
    /// (`read_at`) layer verification on top.
    fn read_inode_data(&mut self, nid: u64, off: u64, buf: &mut [u8]) -> Result<usize> {
        let inode = self.inode(nid)?;
        let layout = inode.layout;
        let size = inode.size;
        let raw_blkaddr = inode.raw_blkaddr;
        let inline_off = inode.inline_off;
        if off >= size {
            return Ok(0);
        }
        let end = off + core::cmp::min(buf.len() as u64, size - off);
        let total = (end - off) as usize;
        let bs = self.block_size;
        match layout {
            FLAT_PLAIN => {
                let base = u64::from(raw_blkaddr) * bs;
                self.src.read_at(base + off, &mut buf[..total])?;
                Ok(total)
            }
            FLAT_INLINE => {
                let full_bytes = (size / bs) * bs;
                let base = u64::from(raw_blkaddr) * bs;
                let mut p = off;
                while p < end {
                    let (disk, seg_end) = if p < full_bytes {
                        (base + p, core::cmp::min(end, full_bytes))
                    } else {
                        (inline_off + (p - full_bytes), end)
                    };
                    let n = (seg_end - p) as usize;
                    let bo = (p - off) as usize;
                    self.src.read_at(disk, &mut buf[bo..bo + n])?;
                    p = seg_end;
                }
                Ok(total)
            }
            COMPRESSED_FULL | COMPRESSED_COMPACT => {
                self.materialize_compressed(nid)?;
                let data = self
                    .decoded
                    .get(&nid)
                    .ok_or(FoldError::Corrupt("erofs: decode cache miss"))?;
                let lo = off as usize;
                let seg = data
                    .get(lo..lo + total)
                    .ok_or(FoldError::Corrupt("erofs: decoded read past end"))?;
                buf[..total].copy_from_slice(seg);
                Ok(total)
            }
            CHUNK_BASED => self.read_chunked(raw_blkaddr, inline_off, off, end, buf),
            _ => Err(FoldError::Unsupported("erofs: unknown datalayout")),
        }
    }

    /// Read `off..end` of a chunk-based inode. Its `i_u` holds the chunk
    /// format; the chunk table, 4-byte block addresses, follows the inode and
    /// its xattrs, 4-byte aligned. A chunk at [`NULL_BLKADDR`] is a hole.
    /// mkfs.erofs writes 8-byte chunk indexes only for chunks on a blob
    /// device, so they're refused like extra devices.
    fn read_chunked(
        &mut self,
        raw_blkaddr: u32,
        inline_off: u64,
        off: u64,
        end: u64,
        buf: &mut [u8],
    ) -> Result<usize> {
        let format = (raw_blkaddr & 0xffff) as u16;
        if format & !CHUNK_FORMAT_ALL != 0 {
            return Err(FoldError::Corrupt("erofs: unknown chunk format bits"));
        }
        if format & CHUNK_48BIT != 0 {
            return Err(FoldError::Unsupported(
                "erofs: 48-bit chunk indexes not supported",
            ));
        }
        let chunkbits = u32::from(self.blkszbits) + u32::from(format & CHUNK_BLKBITS_MASK);
        if chunkbits > 40 {
            return Err(FoldError::Corrupt("erofs: implausible chunk size"));
        }
        if format & CHUNK_INDEXES != 0 {
            return Err(FoldError::Unsupported("erofs: chunk indexes not supported"));
        }
        let chunk = 1u64 << chunkbits;
        let table = (inline_off + 3) & !3;
        let bs = self.block_size;
        let mut p = off;
        while p < end {
            let idx = p / chunk;
            let within = p % chunk;
            let seg_end = core::cmp::min(end, (idx + 1) * chunk);
            let n = (seg_end - p) as usize;
            let bo = (p - off) as usize;
            let mut e = [0u8; 4];
            self.src.read_at(table + idx * 4, &mut e)?;
            let blkaddr = u32::from_le_bytes(e);
            if blkaddr == NULL_BLKADDR {
                buf[bo..bo + n].fill(0);
            } else {
                let disk = u64::from(blkaddr) * bs + within;
                self.src.read_at(disk, &mut buf[bo..bo + n])?;
            }
            p = seg_end;
        }
        Ok((end - off) as usize)
    }

    /// Parse the MicroLZMA window from `z_erofs_lzma_cfgs` once at open. Leaves
    /// `lzma_dict_size` `None` when the volume declares no LZMA or an unvalidated
    /// config `format`, so an LZMA-head inode refuses cleanly instead of guessing.
    fn parse_lzma_cfg(&mut self) {
        let mut algs = [0u8; 2];
        if self.src.read_at(COMPR_ALGS_OFF, &mut algs).is_err() {
            return;
        }
        let algs = u16::from_le_bytes(algs);
        if algs & COMPR_ALG_LZMA_BIT == 0 {
            return;
        }
        // Config blobs follow the superblock in ascending algorithm-bit order,
        // each `{ __le16 size }{ payload[size] }`. Walk to the LZMA (bit 1) blob.
        let mut cur = self.compr_cfgs_off;
        for alg in 0..16u32 {
            if algs & (1 << alg) == 0 {
                continue;
            }
            let mut sz = [0u8; 2];
            if self.src.read_at(cur, &mut sz).is_err() {
                return;
            }
            let size = u64::from(u16::from_le_bytes(sz));
            if alg == u32::from(ALGO_LZMA) {
                let mut pay = [0u8; 6];
                if self.src.read_at(cur + 2, &mut pay).is_err() {
                    return;
                }
                let dict = u32::from_le_bytes([pay[0], pay[1], pay[2], pay[3]]);
                let format = u16::from_le_bytes([pay[4], pay[5]]);
                // A non-zero format would put the props byte on disk (unvalidated).
                if format == 0 && dict > 0 && dict <= 16 * 1024 * 1024 {
                    self.lzma_dict_size = Some(dict);
                }
                return;
            }
            cur += 2 + size;
        }
    }

    /// Decode a compressed inode (datalayout FULL or COMPACT) in full and cache it.
    /// The pcluster chain is decoded forward — LZ4 needs the prior output as a
    /// sliding dictionary — and caching the whole file backs `read_inode_data`.
    fn materialize_compressed(&mut self, nid: u64) -> Result<()> {
        if self.decoded.contains_key(&nid) {
            return Ok(());
        }
        let inode = self.inode(nid)?;
        let i_size = inode.size;
        if i_size > 64 * 1024 * 1024 {
            return Err(FoldError::FileTooLarge {
                size: i_size,
                max: 64 * 1024 * 1024,
            });
        }
        let layout = inode.layout;
        let mh = (inode.inline_off + 7) & !7;

        // Cap i_size FIRST: every downstream size (n_lc, the compact-index
        // allocations it drives in build_heads_*, and the output buffer) is derived
        // from it, so an attacker-controlled multi-TiB i_size must be refused before
        // any allocation — not just before the final output Vec.
        let cap = checked_full_read_len(i_size)?;

        let mut hdr = [0u8; 8];
        self.src.read_at(mh, &mut hdr)?;
        let advise = u16::from_le_bytes([hdr[4], hdr[5]]);
        let head1_algo = hdr[6] & 0x0f;
        let head2_algo = (hdr[6] >> 4) & 0x0f;

        // Refuse layout variants whose data is not where the index points. The
        // superblock check already refuses fragments; these guard an inode that
        // claims one anyway.
        if hdr[7] & CLUSTERBITS_WHOLE_FILE_FRAGMENT != 0
            || advise & (ADVISE_INLINE_PCLUSTER | ADVISE_INTERLACED_PCLUSTER | ADVISE_FRAGMENT) != 0
        {
            return Err(FoldError::Unsupported(
                "erofs: ztailpacking / fragment / interlaced pclusters not supported",
            ));
        }
        // A logical cluster is the block size shifted by the header's low bits.
        let lcbits = u32::from(self.blkszbits) + u32::from(hdr[7] & 0x0f);
        if lcbits > 30 {
            return Err(FoldError::Corrupt("erofs: implausible lclusterbits"));
        }
        let lcsize = 1u64 << lcbits;
        let n_lc = i_size.div_ceil(lcsize);

        let heads = match layout {
            COMPRESSED_FULL => self.build_heads_full(mh, lcsize, n_lc)?,
            COMPRESSED_COMPACT => {
                // Compact entries keep at least 12 value bits (room for a cblkcnt),
                // so their packing is the same for every lclusterbits up to 12 (4
                // KiB clusters and the smaller blocks of -b 512/1024/2048 images);
                // only the cluster size differs. Above 12 the packing widens and
                // is refused until validated. The all-4B layout (COMPACTED_2B
                // clear) uses a different zone shape and is refused too.
                if lcbits > COMPACT_LOBITS {
                    return Err(FoldError::Unsupported(
                        "erofs: compact index with lclusterbits above 12",
                    ));
                }
                if advise & ADVISE_COMPACTED_2B == 0 {
                    return Err(FoldError::Unsupported(
                        "erofs: all-4B compact index (COMPACTED_2B clear) not supported",
                    ));
                }
                self.build_heads_compact(mh + 8, lcsize, n_lc, advise)?
            }
            _ => return Err(FoldError::Corrupt("erofs: inode is not compressed")),
        };

        let mut out: Vec<u8> = Vec::with_capacity(cap);
        for k in 0..heads.len() {
            let h = &heads[k];
            let next = heads.get(k + 1).map_or(i_size, |n| n.start);
            if next < h.start {
                return Err(FoldError::Corrupt("erofs: non-monotonic pcluster starts"));
            }
            let outlen = (next - h.start) as usize;
            // mkfs emits a final HEAD/PLAIN lcluster whose clusterofs lands its
            // logical start exactly at i_size when the last pcluster boundary
            // coincides with EOF — a zero-length trailing extent. The reference
            // index walk yields this head too; it contributes no bytes, and its
            // blkaddr can point one block past the compressed data (EOF), so it
            // must not reach `read_span`. (A zero-length head is only ever the
            // last one — an interior extent always advances the logical offset.)
            if outlen == 0 {
                continue;
            }
            let algo = if h.ty == LC_TYPE_HEAD2 {
                head2_algo
            } else {
                head1_algo
            };
            let span = self.read_span(h.blkaddr, h.cblkcnt)?;
            let seg = decode_pcluster(
                h.ty,
                algo,
                &span,
                outlen,
                &out,
                self.lzma_dict_size,
                self.lz4_zero_padding,
            )?;
            out.extend_from_slice(&seg);
        }
        self.decoded.clear();
        self.decoded.insert(nid, out);
        Ok(())
    }

    /// Read a pcluster's compressed span: the `cblkcnt` contiguous blocks at
    /// `blkaddr` (one block unless big-pcluster), clamped to the device.
    fn read_span(&mut self, blkaddr: u32, cblkcnt: u32) -> Result<Vec<u8>> {
        let bs = self.block_size;
        let phys = u64::from(blkaddr) * bs;
        let want = u64::from(cblkcnt) * bs;
        if want == 0
            || phys
                .checked_add(want)
                .is_none_or(|end| end > self.src.len())
        {
            return Err(FoldError::Corrupt("erofs: pcluster span out of range"));
        }
        let mut span = vec![0u8; checked_block_len(want)?];
        self.src.read_at(phys, &mut span)?;
        Ok(span)
    }

    /// Build the pcluster list from a FULL (legacy, datalayout 1) index: a fixed
    /// 8-byte `z_erofs_lcluster_index` per lcluster, `blkaddr` read straight from
    /// each HEAD/PLAIN entry, one block per pcluster.
    fn build_heads_full(&mut self, mh: u64, lcsize: u64, n_lc: u64) -> Result<Vec<Head>> {
        let idx0 = mh + Z_EROFS_LEGACY_HEADER_SIZE;
        let mut heads = Vec::new();
        for i in 0..n_lc {
            let mut e = [0u8; 8];
            self.src.read_at(idx0 + i * 8, &mut e)?;
            let advise = u16::from_le_bytes([e[0], e[1]]);
            let ty = advise & 3;
            if ty == LC_TYPE_NONHEAD {
                continue;
            }
            if advise & LI_HOLE != 0 {
                return Err(FoldError::Unsupported("erofs: hole pcluster not supported"));
            }
            let clusterofs = u64::from(u16::from_le_bytes([e[2], e[3]]));
            let blkaddr = u32::from_le_bytes([e[4], e[5], e[6], e[7]]);
            let start = if heads.is_empty() {
                0
            } else {
                i * lcsize + clusterofs
            };
            heads.push(Head {
                start,
                ty,
                blkaddr,
                cblkcnt: 1,
            });
        }
        Ok(heads)
    }

    /// Build the pcluster list from a COMPACT (datalayout 3) index: read the
    /// bit-packed region once, unpack the zone-amortized lcluster entries, then
    /// resolve each pcluster's clusterofs, cblkcnt, and absolute blkaddr.
    fn build_heads_compact(
        &mut self,
        ebase: u64,
        lcsize: u64,
        n_lc: u64,
        advise: u16,
    ) -> Result<Vec<Head>> {
        // Zone partition: an initial run of 4-byte-amortized groups aligns the
        // 2-byte-amortized packs to 32 bytes, then a final 4-byte run.
        let ebrem = (ebase & 31) as u32;
        let c4i = core::cmp::min(u64::from(((32 - ebrem) / 4) & 7), n_lc);
        let c2b = (n_lc - c4i) / 16 * 16;
        let c4f = n_lc - c4i - c2b;
        let idx_len = c4i.div_ceil(2) * 8 + c2b / 16 * 32 + c4f.div_ceil(2) * 8;
        let cap = checked_full_read_len(idx_len)?;
        let mut buf = vec![0u8; cap];
        self.src.read_at(ebase, &mut buf)?;

        let mut raw = Vec::with_capacity(n_lc as usize);
        let mut pos = 0usize;
        let mut rid = 0u32;
        read_4b_groups(&buf, &mut pos, c4i, &mut rid, &mut raw)?;
        read_2b_packs(&buf, &mut pos, c2b, &mut rid, &mut raw)?;
        read_4b_groups(&buf, &mut pos, c4f, &mut rid, &mut raw)?;
        resolve_compact_heads(&raw, lcsize, advise)
    }

    /// Parse the dirents in one directory data block.
    fn parse_dir_block(block: &[u8], out: &mut Vec<(String, u64, u8)>) -> Result<()> {
        if block.len() < 12 {
            return Ok(());
        }
        let first_nameoff = usize::from(le_u16(block, 8)?);
        let count = first_nameoff / 12;
        for i in 0..count {
            let base = i * 12;
            let nid = le_u64(block, base)?;
            let nameoff = usize::from(le_u16(block, base + 8)?);
            let file_type = *block
                .get(base + 10)
                .ok_or(FoldError::Corrupt("erofs: dirent"))?;
            let name_end = if i + 1 < count {
                usize::from(le_u16(block, (i + 1) * 12 + 8)?)
            } else {
                block.len()
            };
            let raw = block
                .get(nameoff..name_end)
                .ok_or(FoldError::Corrupt("erofs: dirent name OOB"))?;
            // names are not NUL-terminated, but the final name in a block may be
            // zero-padded to the block boundary.
            let name = raw.split(|&b| b == 0).next().unwrap_or(raw);
            if name.is_empty() || name == b"." || name == b".." {
                continue;
            }
            let name = core::str::from_utf8(name)
                .map_err(|_| FoldError::Unsupported("erofs: non-UTF-8 name"))?;
            if name.is_empty() || name.contains(['/', '\\', '\0']) {
                return Err(FoldError::Corrupt("erofs: invalid name"));
            }
            out.push((String::from(name), nid, file_type));
        }
        Ok(())
    }
}

impl<S: BlockSource> FoldFrontend<S> for Erofs<S> {
    const TAG: &'static str = "erofs";

    fn probe(src: &mut S) -> Result<bool> {
        if src.len() < SUPER_OFFSET + 4 {
            return Ok(false);
        }
        let mut m = [0u8; 4];
        src.read_at(SUPER_OFFSET, &mut m)?;
        Ok(u32::from_le_bytes(m) == MAGIC)
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        if src.len() < SUPER_OFFSET + 128 {
            return Err(FoldError::Corrupt("erofs: source shorter than superblock"));
        }
        let mut src = src;
        let mut sb = [0u8; 128];
        src.read_at(SUPER_OFFSET, &mut sb)?;
        if le_u32(&sb, 0)? != MAGIC {
            return Err(FoldError::Corrupt("erofs: bad magic"));
        }
        let blkszbits = sb[12];
        if !(9..=12).contains(&blkszbits) {
            return Err(FoldError::Corrupt("erofs: implausible blkszbits"));
        }
        let block_size = 1u64 << blkszbits;
        let blocks = u64::from(le_u32(&sb, 36)?);
        if blocks == 0
            || blocks
                .checked_mul(block_size)
                .is_none_or(|end| end > src.len())
        {
            return Err(FoldError::Corrupt("erofs: declared volume exceeds source"));
        }
        if le_u32(&sb, 8)? & 1 != 0 {
            let mut region = vec![0; (block_size - (SUPER_OFFSET % block_size)) as usize];
            src.read_at(SUPER_OFFSET, &mut region)?;
            let expected = le_u32(&region, 4)?;
            region[4..8].fill(0);
            let mut crc = u32::MAX;
            for byte in region {
                crc ^= byte as u32;
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0x82f63b78u32 & 0u32.wrapping_sub(crc & 1));
                }
            }
            if crc != expected {
                return Err(FoldError::Corrupt("erofs: superblock checksum"));
            }
        }
        let incompat = le_u32(&sb, SB_FEATURE_INCOMPAT)?;
        refuse_unsupported_features(&sb, incompat)?;
        let root_nid = u64::from(le_u16(&sb, 14)?);
        let meta_off = u64::from(le_u32(&sb, 40)?) * block_size;
        let mut me = Erofs {
            src: VolumeSource {
                source: src,
                length: blocks * block_size,
            },
            block_size,
            blkszbits,
            lz4_zero_padding: incompat & INCOMPAT_LZ4_0PADDING != 0,
            meta_off,
            root_nid,
            inodes: BTreeMap::new(),
            decoded: BTreeMap::new(),
            lzma_dict_size: None,
            compr_cfgs_off: SUPER_OFFSET
                + SB_BASE_SIZE
                + u64::from(sb[SB_EXTSLOTS]) * SB_EXTSLOT_SIZE,
            build_time: (le_u64(&sb, 24)? as i64, le_u32(&sb, 32)?),
        };
        me.parse_lzma_cfg();
        me.parse_inode(root_nid)?;
        Ok(me)
    }

    fn root(&self) -> NodeId {
        self.root_nid
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        cx: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .read_dir(dir, cx)?
            .into_iter()
            .find(|e| e.name == name)
            .map(|e| e.node))
    }

    fn read_dir(&mut self, dir: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.parse_inode(dir)?;
        let inode = self.inode(dir)?;
        if inode.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        if inode.size > 16 * 1024 * 1024 {
            return Err(FoldError::FileTooLarge {
                size: inode.size,
                max: 16 * 1024 * 1024,
            });
        }
        let size = checked_full_read_len(inode.size)?;
        let mut data = vec![0u8; size];
        self.read_inode_data(dir, 0, &mut data)?;

        let bs = self.block_size as usize;
        let mut raw = Vec::new();
        let mut start = 0;
        while start < data.len() {
            let block_end = core::cmp::min(start + bs, data.len());
            Self::parse_dir_block(&data[start..block_end], &mut raw)?;
            start = block_end;
        }

        let mut out = Vec::with_capacity(raw.len());
        for (name, nid, file_type) in raw {
            out.push(DirEntry {
                name,
                node: nid,
                kind: ft_kind(file_type),
            });
        }
        Ok(out)
    }

    fn metadata(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        self.parse_inode(node)?;
        let inode = self.inode(node)?;
        Ok(Metadata {
            kind: inode.kind,
            size: inode.size,
            mode: u32::from(inode.mode) & 0o7777,
        })
    }

    fn timestamps(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<lamfold::FileTimes> {
        self.parse_inode(node)?;
        let modified = self.inode(node)?.modified;
        if modified.1 >= 1_000_000_000 {
            return Err(FoldError::Corrupt("erofs: timestamp nanoseconds"));
        }
        Ok(lamfold::FileTimes {
            modified: Some(modified),
            ..Default::default()
        })
    }

    fn read_at(
        &mut self,
        node: NodeId,
        off: u64,
        buf: &mut [u8],
        cx: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        self.parse_inode(node)?;
        let inode = self.inode(node)?;
        if inode.kind == FileKind::Directory {
            return Err(FoldError::IsDirectory);
        }
        let size = inode.size;
        if off >= size {
            return Ok(0);
        }
        let bs = self.block_size;
        let end = core::cmp::min(off + buf.len() as u64, size);
        let mut block = vec![0u8; bs as usize];
        let mut produced = 0;
        let mut block_start = (off / bs) * bs;
        while block_start < end {
            let block_len = core::cmp::min(bs, size - block_start) as usize;
            // Materialise the whole block so it can be verified as a unit, then
            // hand it to the shepherd before any byte is surfaced.
            self.read_inode_data(node, block_start, &mut block[..block_len])?;
            cx.verifier
                .verify_block(node, block_start, &block[..block_len])?;

            let copy_start = core::cmp::max(off, block_start);
            let copy_end = core::cmp::min(end, block_start + block_len as u64);
            if copy_end > copy_start {
                let src_lo = (copy_start - block_start) as usize;
                let dst_lo = (copy_start - off) as usize;
                let cnt = (copy_end - copy_start) as usize;
                buf[dst_lo..dst_lo + cnt].copy_from_slice(&block[src_lo..src_lo + cnt]);
                produced += cnt;
            }
            block_start += bs;
        }
        Ok(produced)
    }

    fn read_link(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        self.parse_inode(node)?;
        let inode = self.inode(node)?;
        if inode.kind != FileKind::Symlink {
            return Ok(None);
        }
        let len = checked_full_read_len(inode.size)?;
        let mut target = vec![0u8; len];
        self.read_inode_data(node, 0, &mut target)?;
        Ok(Some(target))
    }
}

/// One unpacked compact-index lcluster: its type, the 12-bit value (clusterofs for
/// HEAD/PLAIN; a back-delta or cblkcnt for NONHEAD), and the amortization region it
/// came from plus that region's per-group `u32` base blkaddr.
struct RawEntry {
    ty: u16,
    value: u16,
    region: u32,
    base: u32,
}

/// Unpack a run of 4-byte-amortized lclusters: 8-byte groups of
/// `[u16 even][u16 odd][u32 base]`, two lclusters sharing one base blkaddr.
fn read_4b_groups(
    buf: &[u8],
    pos: &mut usize,
    count: u64,
    rid: &mut u32,
    raw: &mut Vec<RawEntry>,
) -> Result<()> {
    let mut produced = 0u64;
    while produced < count {
        let g = buf
            .get(*pos..*pos + 8)
            .ok_or(FoldError::Corrupt("erofs: compact 4b group out of range"))?;
        let base = u32::from_le_bytes([g[4], g[5], g[6], g[7]]);
        for half in [
            u16::from_le_bytes([g[0], g[1]]),
            u16::from_le_bytes([g[2], g[3]]),
        ] {
            if produced >= count {
                break;
            }
            raw.push(RawEntry {
                ty: (half >> COMPACT_LOBITS) & 3,
                value: half & 0x0fff,
                region: *rid,
                base,
            });
            produced += 1;
        }
        *pos += 8;
        *rid += 1;
    }
    Ok(())
}

/// Unpack a run of 2-byte-amortized lclusters: 32-byte packs of sixteen 14-bit
/// LSB-first entries followed by the pack's `u32` base blkaddr at offset 28.
fn read_2b_packs(
    buf: &[u8],
    pos: &mut usize,
    count: u64,
    rid: &mut u32,
    raw: &mut Vec<RawEntry>,
) -> Result<()> {
    let mut produced = 0u64;
    while produced < count {
        let pack = buf
            .get(*pos..*pos + 32)
            .ok_or(FoldError::Corrupt("erofs: compact 2b pack out of range"))?;
        let base = u32::from_le_bytes([pack[28], pack[29], pack[30], pack[31]]);
        for s in 0..16u32 {
            if produced >= count {
                break;
            }
            let v = read_bits_lsb(pack, s * COMPACT_ENCBITS)?;
            raw.push(RawEntry {
                ty: (v >> COMPACT_LOBITS) as u16 & 3,
                value: (v & 0x0fff) as u16,
                region: *rid,
                base,
            });
            produced += 1;
        }
        *pos += 32;
        *rid += 1;
    }
    Ok(())
}

/// Read `COMPACT_ENCBITS` (14) bits LSB-first from `buf` at bit offset `bit_pos`.
fn read_bits_lsb(buf: &[u8], bit_pos: u32) -> Result<u32> {
    let mut v = 0u32;
    for k in 0..COMPACT_ENCBITS {
        let bp = bit_pos + k;
        let byte = *buf
            .get((bp >> 3) as usize)
            .ok_or(FoldError::Corrupt("erofs: compact bit offset out of range"))?;
        v |= u32::from((byte >> (bp & 7)) & 1) << k;
    }
    Ok(v)
}

/// Resolve the unpacked lcluster entries into pclusters: each HEAD/PLAIN opens one
/// (the first at logical 0). NONHEADs carry no extent. blkaddr resolution differs
/// by layout: non-big uses each pack's `base + 1 + (HEADs before it in the pack)`;
/// big-pcluster uses a global running sum of cblkcnt starting at the first base
/// itself, whichever kind of region it came from.
fn resolve_compact_heads(raw: &[RawEntry], lcsize: u64, advise: u16) -> Result<Vec<Head>> {
    let big = advise & ADVISE_BIG_PCLUSTER != 0;
    let first_base = raw.first().map_or(0, |e| e.base);

    let mut heads: Vec<Head> = Vec::new();
    let mut region_heads: BTreeMap<u32, u32> = BTreeMap::new();
    let mut cur_head: Option<usize> = None;
    for (i, e) in raw.iter().enumerate() {
        if e.ty == LC_TYPE_NONHEAD {
            // Big-pcluster encodes a pcluster's compressed-block count in the
            // NONHEAD whose value has the cblkcnt flag set.
            if big && e.value & COMPACT_CBLKCNT_FLAG != 0 {
                if let Some(h) = cur_head {
                    heads[h].cblkcnt = u32::from(e.value & 0x07ff);
                    // A count of 0 is Linux 7.2's sparse pcluster (a hole).
                    if heads[h].cblkcnt == 0 {
                        return Err(FoldError::Unsupported("erofs: hole pcluster not supported"));
                    }
                }
            }
            continue;
        }
        let clusterofs = u64::from(e.value);
        let start = if heads.is_empty() {
            0
        } else {
            i as u64 * lcsize + clusterofs
        };
        let blkaddr = if big {
            0 // filled by the running sum below
        } else {
            let c = region_heads.entry(e.region).or_insert(0);
            // Checked, mirroring the big-pcluster running sum below: a crafted
            // base near u32::MAX must surface Corrupt, not panic / wrap to block 0.
            let b = e
                .base
                .checked_add(1)
                .and_then(|x| x.checked_add(*c))
                .ok_or(FoldError::Corrupt("erofs: compact blkaddr overflow"))?;
            *c += 1;
            b
        };
        heads.push(Head {
            start,
            ty: e.ty,
            blkaddr,
            cblkcnt: 1,
        });
        cur_head = Some(heads.len() - 1);
    }

    if big {
        // An index whose base is 32-byte aligned opens with a 2B pack rather than
        // a 4B group; 0.1.1 added one block for that case, which misplaced every
        // pcluster of such a file (any file whose inline xattrs bring the index
        // base to a 32-byte boundary, as a 52- or 56-byte SELinux label does).
        let mut blk = first_base;
        for k in 0..heads.len() {
            if k > 0 {
                blk = blk
                    .checked_add(heads[k - 1].cblkcnt)
                    .ok_or(FoldError::Corrupt("erofs: compact blkaddr overflow"))?;
            }
            heads[k].blkaddr = blk;
        }
    }
    Ok(heads)
}

/// Decode one pcluster's `span` (its `cblkcnt` blocks) to `outlen` bytes. PLAIN
/// pclusters are raw-stored (left-aligned); HEAD pclusters dispatch on the head
/// algorithm. `prior_out` is the decoded-so-far output (LZ4's sliding dictionary).
fn decode_pcluster(
    ty: u16,
    algo: u8,
    span: &[u8],
    outlen: usize,
    prior_out: &[u8],
    lzma_dict: Option<u32>,
    lz4_zero_padding: bool,
) -> Result<Vec<u8>> {
    match ty {
        LC_TYPE_PLAIN => {
            // Apply the same per-block decompression-bomb cap the codec arms get
            // via `decode` — a PLAIN copy is raw, but `outlen` is still attacker-
            // controlled and must not allocate past the 16 MiB block cap.
            let _ = checked_block_len(outlen as u64)?;
            span.get(..outlen)
                .map(<[u8]>::to_vec)
                .ok_or(FoldError::Corrupt(
                    "erofs: plain pcluster shorter than output",
                ))
        }
        LC_TYPE_HEAD1 | LC_TYPE_HEAD2 => {
            decode_head(algo, span, outlen, prior_out, lzma_dict, lz4_zero_padding)
        }
        _ => Err(FoldError::Corrupt("erofs: non-head pcluster in decode")),
    }
}

/// Decode a HEAD pcluster by its algorithm. LZ4 fills the (single) block and reads
/// the sliding-window dictionary; deflate/zstd/lzma are self-contained,
/// right-aligned streams spanning `cblkcnt` blocks (no cross-pcluster dictionary).
fn decode_head(
    algo: u8,
    span: &[u8],
    outlen: usize,
    prior_out: &[u8],
    lzma_dict: Option<u32>,
    lz4_zero_padding: bool,
) -> Result<Vec<u8>> {
    match algo {
        ALGO_LZ4 => {
            let win = &prior_out[prior_out.len().saturating_sub(LZ4_WINDOW)..];
            lz4_pcluster(span, outlen, win, lz4_zero_padding)
        }
        ALGO_DEFLATE => deflate_pcluster(span, outlen),
        ALGO_ZSTD => zstd_pcluster(span, outlen),
        ALGO_LZMA => {
            let dict = lzma_dict.ok_or(FoldError::Unsupported("erofs: lzma config unavailable"))?;
            lzma_pcluster(span, outlen, dict)
        }
        _ => Err(FoldError::Unsupported(
            "erofs: unknown compressed head algorithm",
        )),
    }
}

/// LZ4 pcluster. On an `LZ4_0PADDING` volume mkfs right-aligns the stream within
/// the pcluster's blocks, and the stream starts at the first non-zero byte: The
/// kernel skips every leading zero byte, so that is the only start it reads.
/// Trying offset 0 as well is wrong, because a single leading padding byte also
/// decodes "successfully" (0x00 is a valid LZ4 token) into the wrong bytes. A
/// volume without the feature stores the stream at offset 0. (`win` is the
/// sliding-window dictionary: the prior decoded output.)
fn lz4_pcluster(span: &[u8], outlen: usize, win: &[u8], zero_padding: bool) -> Result<Vec<u8>> {
    let start = if zero_padding {
        span.iter()
            .position(|&b| b != 0)
            .ok_or(FoldError::Corrupt("erofs lz4: all-zero pcluster"))?
    } else {
        0
    };
    match lz4_block_with_dict(&span[start..], outlen, win) {
        Ok(v) if v.len() == outlen => Ok(v),
        Ok(_) => Err(FoldError::Corrupt(
            "erofs lz4: stream decoded to the wrong length",
        )),
        Err(e) => Err(e),
    }
}

/// Raw-DEFLATE pcluster. The stream ends at the span's last byte; any leading
/// bytes (on a short/partial-final pcluster) are zero padding. A full pcluster
/// starts at offset 0; a padded one at its first non-zero byte. Accept the start
/// whose bounded inflate yields exactly `outlen` bytes.
fn deflate_pcluster(span: &[u8], outlen: usize) -> Result<Vec<u8>> {
    let fnz = span.iter().position(|&b| b != 0).unwrap_or(0);
    let mut last = FoldError::Corrupt("erofs deflate: no valid stream start");
    for start in [0usize, fnz] {
        let tail = span.get(start..).unwrap_or(&[]);
        match decode(Codec::Deflate, tail, outlen) {
            Ok(v) if v.len() == outlen => return Ok(v),
            Ok(_) => {}
            // A disabled-codec / bomb-cap refusal is the real cause — surface it
            // rather than masking it as a generic "no valid stream start".
            Err(e @ (FoldError::Unsupported(_) | FoldError::FileTooLarge { .. })) => return Err(e),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// ZSTD pcluster: one full frame, right-aligned to the span. The frame start is
/// the unique offset of the magic (zero padding can never form it).
fn zstd_pcluster(span: &[u8], outlen: usize) -> Result<Vec<u8>> {
    const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
    let start = span
        .windows(4)
        .position(|w| w == MAGIC)
        .ok_or(FoldError::Corrupt("erofs zstd: frame magic not found"))?;
    decode(Codec::Zstd, &span[start..], outlen)
}

/// MicroLZMA pcluster: a raw LZMA1 stream right-aligned to the span. Its first
/// non-zero byte holds the LZMA properties inverted (`!props`), which the kernel
/// reads back; mkfs writes `!0x5d` unless given custom lc/lp/pb (erofs-utils
/// 1.9). `microlzma_block_decode` swaps that byte for LZMA1's leading `0x00`.
fn lzma_pcluster(span: &[u8], outlen: usize, dict_size: u32) -> Result<Vec<u8>> {
    let start = span
        .iter()
        .position(|&b| b != 0)
        .ok_or(FoldError::Corrupt("erofs lzma: all-zero span"))?;
    let props = !span[start];
    if props > LZMA_PROPS_MAX {
        return Err(FoldError::Corrupt("erofs lzma: invalid properties byte"));
    }
    microlzma_block_decode(&span[start..], props, dict_size, outlen)
}

/// Refuse at open what this reader can't read correctly anywhere in the
/// volume, rather than misreading files one by one.
fn refuse_unsupported_features(sb: &[u8], incompat: u32) -> Result<()> {
    if incompat & !INCOMPAT_KNOWN != 0 {
        return Err(FoldError::Unsupported(
            "erofs: unknown incompatible feature",
        ));
    }
    // Metabox needs the 48-bit layout, so check it first to name the cause.
    if incompat & INCOMPAT_METABOX != 0 {
        return Err(FoldError::Unsupported("erofs: metabox not supported"));
    }
    if incompat & INCOMPAT_48BIT != 0 {
        return Err(FoldError::Unsupported("erofs: 48-bit layout not supported"));
    }
    if le_u16(sb, SB_EXTRA_DEVICES)? != 0 {
        return Err(FoldError::Unsupported("erofs: extra devices not supported"));
    }
    // Fragments keep file tails, or whole files, in a packed inode this reader
    // doesn't implement yet; mkfs -Efragments and -Eall-fragments (Fedora's
    // live images, lorax) make one.
    if le_u64(sb, SB_PACKED_NID)? != 0 {
        return Err(FoldError::Unsupported(
            "erofs: fragments (packed inode) not supported",
        ));
    }
    Ok(())
}

fn ft_kind(file_type: u8) -> FileKind {
    match file_type {
        1 => FileKind::Regular,
        2 => FileKind::Directory,
        7 => FileKind::Symlink,
        _ => FileKind::Other,
    }
}

fn le_u16(b: &[u8], o: usize) -> Result<u16> {
    b.get(o..o + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(FoldError::Corrupt("erofs: truncated u16"))
}
fn le_u32(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(FoldError::Corrupt("erofs: truncated u32"))
}
fn le_u64(b: &[u8], o: usize) -> Result<u64> {
    b.get(o..o + 8)
        .and_then(|s| s.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(FoldError::Corrupt("erofs: truncated u64"))
}
