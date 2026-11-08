//! Snapshot/delta chain reader: layers a delta VMDK on top of its parent chain.
//!
//! Given a delta VMDK (parentCID != `0xffff_ffff`), opens the parent referenced by
//! `parentFileNameHint`, validates that the parent's CID matches the delta's parentCID,
//! and presents a unified `Read + Seek` view where:
//! - allocated sectors in the delta are read from the delta
//! - sparse sectors in the delta are read from the parent (recursively)
//!
//! A chain depth limit (64 levels) guards against circular references in crafted images.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::{VmdkError, VmdkFileReader};

/// Maximum number of delta layers in a chain before returning an error.
pub const MAX_CHAIN_DEPTH: usize = 64;

/// A read-only view over a VMDK snapshot chain.
///
/// Implements `Read + Seek` over the merged virtual sector stream, walking from the
/// most-recent delta down to the base image for each sparse grain.
///
/// Opened via [`VmdkChainReader::open`].
pub struct VmdkChainReader {
    /// Layers from newest (index 0 = delta) to oldest (last = base image).
    layers: Vec<VmdkFileReader>,
    virtual_disk_size: u64,
    pos: u64,
    /// Smallest non-zero grain size (bytes) across the layers; reads are clamped
    /// to this boundary so a sparse grain can't zero-mask a following allocated
    /// grain. `0` for an all-flat chain, where no clamping is needed.
    clamp_bytes: u64,
}

impl VmdkChainReader {
    /// Open a (potentially chained) VMDK from `path`, following `parentFileNameHint`
    /// until a base image is reached or `MAX_CHAIN_DEPTH` is exceeded.
    ///
    /// If `path` is a base image (`parentCID == 0xffff_ffff`), this is equivalent to
    /// `VmdkReader::open_path` wrapped in a single-layer chain.
    pub fn open(path: &Path) -> Result<Self, VmdkError> {
        let mut layers: Vec<VmdkFileReader> = Vec::new();
        let mut current_path = path.to_path_buf();

        for depth in 0..=MAX_CHAIN_DEPTH {
            let reader = VmdkFileReader::open_path(&current_path)?;
            let parent_cid = reader.parent_cid();
            crate::diag::chain_layer(depth, reader.cid(), parent_cid);

            // A CID mismatch between a child's parentCID and its parent's CID means the
            // parent was modified after the snapshot was taken. QEMU warns but continues,
            // and so do we — the chain is still structurally usable — so there is no
            // separate branch to take here; every opened layer is simply pushed.
            layers.push(reader);

            if parent_cid == 0xffff_ffff {
                break; // reached base image
            }
            if depth == MAX_CHAIN_DEPTH {
                return Err(VmdkError::FieldOutOfRange {
                    field: "chain_depth",
                    value: MAX_CHAIN_DEPTH as u64,
                    reason: "snapshot chain exceeds the maximum supported depth",
                });
            }

            // Resolve the parent path relative to the current file's directory.
            let desc_text = layers
                .last()
                .map(|r| r.descriptor_text().to_owned())
                .unwrap_or_default();
            let parent_hint = extract_parent_file_name(&desc_text);
            if parent_hint.is_empty() {
                break; // no hint available — treat as base
            }
            // Resolve the parent strictly within the current file's directory —
            // an absolute or `..`-climbing parentFileNameHint is refused.
            let parent_dir = current_path.parent().unwrap_or(Path::new("."));
            current_path = crate::descriptor::resolve_extent_path(parent_dir, parent_hint)?;
        }

        let virtual_disk_size = layers
            .first()
            .map_or(0, super::VmdkReader::virtual_disk_size);
        let clamp_bytes = layers
            .iter()
            .map(|l| l.info().grain_size_bytes)
            .filter(|&g| g > 0)
            .min()
            .unwrap_or(0);
        Ok(VmdkChainReader {
            layers,
            virtual_disk_size,
            pos: 0,
            clamp_bytes,
        })
    }

    /// Total virtual disk size in bytes (from the delta/top layer).
    pub fn virtual_disk_size(&self) -> u64 {
        self.virtual_disk_size
    }

    /// Number of layers in the chain (1 = base image only, no parent).
    pub fn depth(&self) -> usize {
        self.layers.len()
    }
}

impl Read for VmdkChainReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.virtual_disk_size {
            return Ok(0);
        }
        // Try each layer from newest to oldest. Read from the first layer that has
        // data at this position. Sparse reads return zeros but we detect them by
        // checking is_allocated; if a layer doesn't have data, try the next.
        let mut to_read = buf.len().min((self.virtual_disk_size - self.pos) as usize);
        if self.clamp_bytes > 0 {
            // Clamp to a single grain so a sparse grain cannot zero-mask an
            // allocated grain that falls within the same read.
            let remaining_in_grain = (self.clamp_bytes - self.pos % self.clamp_bytes) as usize;
            to_read = to_read.min(remaining_in_grain);
        }
        let lba = self.pos / 512;

        for layer in &mut self.layers {
            let allocated = layer.is_allocated(lba)?;
            if allocated {
                layer.seek(SeekFrom::Start(self.pos))?;
                let n = layer.read(&mut buf[..to_read])?;
                self.pos += n as u64;
                return Ok(n);
            }
        }

        // All layers are sparse at this position — return zeros.
        buf[..to_read].fill(0);
        self.pos += to_read as u64;
        Ok(to_read)
    }
}

impl Seek for VmdkChainReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.pos as i64 + n,
            SeekFrom::End(n) => self.virtual_disk_size as i64 + n,
        };
        if new_pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}

/// Extract `parentFileNameHint` value from a raw descriptor text.
fn extract_parent_file_name(text: &str) -> &str {
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("parentFileNameHint=") {
            return rest.trim().trim_matches('"');
        }
    }
    ""
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom};

    use super::*;
    use crate::testutil::{write_chain_to_dir, GRAIN_SIZE_BYTES};

    #[test]
    fn extract_parent_file_name_parses_hint() {
        let desc = "# Disk DescriptorFile\nversion=1\nCID=00000001\nparentCID=ffffffff\nparentFileNameHint=\"../base.vmdk\"\ncreateType=\"monolithicSparse\"\n";
        assert_eq!(extract_parent_file_name(desc), "../base.vmdk");
    }

    #[test]
    fn extract_parent_file_name_returns_empty_when_absent() {
        let desc = "# Disk DescriptorFile\nversion=1\nCID=ffffffff\nparentCID=ffffffff\ncreateType=\"monolithicSparse\"\n";
        assert_eq!(extract_parent_file_name(desc), "");
    }

    #[test]
    fn chain_depth_one_for_base_image() {
        let dir = tempfile::tempdir().unwrap();
        let base_data = vec![0x42u8; 512];
        let (base_path, _) = write_chain_to_dir(dir.path(), &base_data);
        let chain = VmdkChainReader::open(&base_path).expect("open base image chain");
        assert_eq!(chain.depth(), 1, "base image has chain depth 1");
    }

    #[test]
    fn chain_depth_two_for_delta() {
        let dir = tempfile::tempdir().unwrap();
        let base_data = vec![0x42u8; 512];
        let (_, delta_path) = write_chain_to_dir(dir.path(), &base_data);
        let chain = VmdkChainReader::open(&delta_path).expect("open delta chain");
        assert_eq!(chain.depth(), 2, "delta over base has chain depth 2");
    }

    #[test]
    fn chain_reads_base_data_through_sparse_delta() {
        let dir = tempfile::tempdir().unwrap();
        let mut base_data = vec![0u8; GRAIN_SIZE_BYTES];
        base_data[0] = 0xDE;
        base_data[1] = 0xAD;
        let (_, delta_path) = write_chain_to_dir(dir.path(), &base_data);
        let mut chain = VmdkChainReader::open(&delta_path).expect("open chain");
        chain.seek(SeekFrom::Start(0)).expect("seek");
        let mut buf = [0u8; 2];
        chain.read_exact(&mut buf).expect("read");
        assert_eq!(
            buf,
            [0xDE, 0xAD],
            "chain must fall through to base data for sparse delta grain"
        );
    }

    #[test]
    fn chain_virtual_disk_size_from_delta() {
        let dir = tempfile::tempdir().unwrap();
        let (_, delta_path) = write_chain_to_dir(dir.path(), &[0u8; 512]);
        let chain = VmdkChainReader::open(&delta_path).expect("open");
        assert_eq!(chain.virtual_disk_size(), GRAIN_SIZE_BYTES as u64);
    }

    #[test]
    fn chain_seek_variants_and_read_edges() {
        let dir = tempfile::tempdir().unwrap();
        let (_, delta_path) = write_chain_to_dir(dir.path(), &[0u8; GRAIN_SIZE_BYTES]);
        let mut chain = VmdkChainReader::open(&delta_path).unwrap();
        let sz = chain.virtual_disk_size();
        assert_eq!(chain.seek(SeekFrom::Start(8)).unwrap(), 8);
        assert_eq!(chain.seek(SeekFrom::Current(-4)).unwrap(), 4);
        assert_eq!(chain.seek(SeekFrom::End(-2)).unwrap(), sz - 2);
        assert!(chain.seek(SeekFrom::End(-(sz as i64) - 1)).is_err());
        chain.seek(SeekFrom::Start(sz)).unwrap();
        assert_eq!(chain.read(&mut [0u8; 4]).unwrap(), 0);
        chain.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(chain.read(&mut []).unwrap(), 0);
    }

    #[test]
    fn chain_all_sparse_reads_zeros() {
        // A single all-sparse layer → no layer reports allocated → zero-fill path.
        let dir = tempfile::tempdir().unwrap();
        let bytes = crate::testutil::gd_at_end_stream_opt_vmdk();
        let p = dir.path().join("sparse.vmdk");
        std::fs::write(&p, &bytes).unwrap();
        let mut chain = VmdkChainReader::open(&p).unwrap();
        let mut buf = [0xFFu8; 512];
        chain.read_exact(&mut buf).unwrap();
        assert_eq!(buf, [0u8; 512]);
    }

    #[test]
    fn chain_breaks_when_parent_hint_missing() {
        // parentCID set but no parentFileNameHint → loop breaks, treated as base.
        let dir = tempfile::tempdir().unwrap();
        let desc = "# Disk DescriptorFile\nversion=1\nCID=00000002\nparentCID=00000001\ncreateType=\"monolithicSparse\"\n";
        let bytes = crate::testutil::test_sparse_vmdk_with_descriptor(&[0u8; 512], desc);
        let p = dir.path().join("d.vmdk");
        std::fs::write(&p, &bytes).unwrap();
        let chain = VmdkChainReader::open(&p).unwrap();
        assert_eq!(chain.depth(), 1, "missing hint → no parent layer");
    }

    #[test]
    fn chain_read_does_not_zero_mask_an_allocated_grain_after_a_sparse_one() {
        use std::io::Read as _;
        // A 2-grain image: grain 0 is sparse, grain 1 holds 0xBB. A single read
        // that spans both grains must not let the sparse grain 0 zero-mask the
        // allocated grain 1 (the un-clamped zero-fill bug).
        let dir = tempfile::tempdir().unwrap();
        let g = crate::testutil::GRAIN_SIZE_BYTES;
        let bytes = crate::testutil::test_sparse_vmdk_sparse_then_allocated(&vec![0xBBu8; g]);
        let p = dir.path().join("base.vmdk");
        std::fs::write(&p, &bytes).unwrap();
        let mut chain = VmdkChainReader::open(&p).unwrap();
        let mut buf = vec![0u8; 2 * g];
        chain.read_exact(&mut buf).unwrap();
        assert!(
            buf[..g].iter().all(|&b| b == 0),
            "grain 0 is sparse -> zeros"
        );
        assert!(
            buf[g..].iter().all(|&b| b == 0xBB),
            "grain 1 (allocated) must not be zero-masked by the sparse grain 0"
        );
    }

    #[test]
    fn chain_refuses_absolute_parent_hint() {
        let dir = tempfile::tempdir().unwrap();
        let base = crate::testutil::test_sparse_vmdk_with_descriptor(
            &[0x55u8; 512],
            "# Disk DescriptorFile\nversion=1\nCID=00000001\nparentCID=ffffffff\ncreateType=\"monolithicSparse\"\n",
        );
        let base_path = dir.path().join("base.vmdk");
        std::fs::write(&base_path, &base).unwrap();
        let delta_desc = format!(
            "# Disk DescriptorFile\nversion=1\nCID=00000002\nparentCID=00000001\nparentFileNameHint=\"{}\"\ncreateType=\"monolithicSparse\"\n",
            base_path.display()
        );
        let delta = crate::testutil::test_sparse_vmdk_with_descriptor(&[0u8; 512], &delta_desc);
        let delta_path = dir.path().join("delta.vmdk");
        std::fs::write(&delta_path, &delta).unwrap();
        // An absolute parentFileNameHint is a crafted-image attempt to read an
        // arbitrary host file; the chain reader must refuse it (secure by default).
        assert!(
            VmdkChainReader::open(&delta_path).is_err(),
            "an absolute parentFileNameHint must be refused"
        );
    }

    #[test]
    fn chain_depth_limit_on_self_reference() {
        // A delta whose parentFileNameHint points at itself loops until MAX_CHAIN_DEPTH.
        let dir = tempfile::tempdir().unwrap();
        let desc = "# Disk DescriptorFile\nversion=1\nCID=00000001\nparentCID=00000001\nparentFileNameHint=\"self.vmdk\"\ncreateType=\"monolithicSparse\"\n";
        let bytes = crate::testutil::test_sparse_vmdk_with_descriptor(&[0u8; 512], desc);
        let p = dir.path().join("self.vmdk");
        std::fs::write(&p, &bytes).unwrap();
        assert!(matches!(
            VmdkChainReader::open(&p),
            Err(VmdkError::FieldOutOfRange {
                field: "chain_depth",
                ..
            })
        ));
    }
}
