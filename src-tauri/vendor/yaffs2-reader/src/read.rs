use crate::{FileType, ReadAt, YaffsImage, exact};
use anyhow::{Context, Result, ensure};

impl YaffsImage {
    pub fn read_range(
        &self,
        source: &dyn ReadAt,
        number: u32,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        ensure!(source.size() == self.length, "YAFFS2 source size changed");
        ensure!(
            length <= 256 * 1024 * 1024,
            "YAFFS2 range exceeds read budget"
        );
        let node = self.content_node(number)?;
        ensure!(
            matches!(node.kind, FileType::File | FileType::Symlink),
            "YAFFS2 entry has no file content"
        );
        let count = (length as u64).min(node.size.saturating_sub(offset)) as usize;
        let mut output = vec![0; count];
        if count == 0 {
            return Ok(output);
        }
        if node.kind == FileType::Symlink {
            output.copy_from_slice(
                &node.link_target.as_bytes()[offset as usize..offset as usize + count],
            );
            return Ok(output);
        }
        let object = self
            .objects
            .get(&node.object_id)
            .context("YAFFS2 content object missing")?;
        let page_size = self.geometry.page_size as u64;
        let end = offset + count as u64;
        let first = u32::try_from(offset / page_size + 1).context("YAFFS2 chunk index overflow")?;
        let last =
            u32::try_from((end - 1) / page_size + 1).context("YAFFS2 chunk index overflow")?;
        for (&number, chunk) in object.chunks.range(first..=last) {
            let start = u64::from(number - 1) * page_size;
            let overlap_start = start.max(offset);
            let overlap_end = (start + u64::from(chunk.length)).min(end);
            if overlap_end <= overlap_start {
                continue;
            }
            let bytes = exact(
                source,
                chunk.offset + (overlap_start - start),
                (overlap_end - overlap_start) as usize,
            )?;
            let position = (overlap_start - offset) as usize;
            output[position..position + bytes.len()].copy_from_slice(&bytes);
        }
        Ok(output)
    }
}
