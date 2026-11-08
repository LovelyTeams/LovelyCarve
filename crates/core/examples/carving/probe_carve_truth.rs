//! Ground truth for carving quality on images with an intact ext4 filesystem.
//!
//! Usage: `just run-example-fast probe_carve_truth <image> <out.tsv>`
//!
//! Walks every ext4 partition and writes one line per regular file:
//! `first_physical_offset<TAB>size<TAB>contiguous<TAB>extension<TAB>path`.
//! `contiguous` is 1 when the whole file is one physical run, which is what
//! a header/footer carver can recover. Join it with `probe_carve --dump`
//! output on the offset column to measure recall and size accuracy.

use std::collections::HashSet;
use std::env;
use std::io::{BufWriter, Write};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use lovelycarve_core::evidence::image::{DiskImage, PartitionKind};
use lovelycarve_core::filesystem::tree::fs_tree::NodeKind;
use lovelycarve_core::filesystem::volumes::ext4::{open_ext4_source, Ext4Source};

fn main() -> anyhow::Result<()> {
    let mut arguments = env::args().skip(1);
    let (Some(image_path), Some(output_path)) = (arguments.next(), arguments.next()) else {
        anyhow::bail!("usage: probe_carve_truth <image> <out.tsv>");
    };
    let started = Instant::now();
    let image = Arc::new(DiskImage::open(&image_path).context("open image")?);
    let mut output = BufWriter::new(
        std::fs::File::create(&output_path).with_context(|| format!("create {output_path}"))?,
    );
    let mut files = 0u64;
    for partition in image
        .partitions
        .iter()
        .filter(|partition| partition.kind == PartitionKind::Ext4)
    {
        let volume = open_ext4_source(&image, Ext4Source::Partition(partition.id))
            .with_context(|| format!("open ext4 partition {}", partition.id))?;
        let mut seen = HashSet::new();
        let mut pending = vec![(2u64, String::new())];
        while let Some((directory, path)) = pending.pop() {
            let children = match volume.children(directory, false) {
                Ok(children) => children,
                Err(error) => {
                    eprintln!("skip directory {path}: {error:#}");
                    continue;
                }
            };
            for child in children {
                let Some(inode_number) = child.inode else {
                    continue;
                };
                let child_path = format!("{path}/{}", child.name);
                match child.kind {
                    NodeKind::Dir => pending.push((inode_number, child_path)),
                    NodeKind::File if child.size > 0 && seen.insert(inode_number) => {
                        let inode = volume.read_inode(inode_number)?;
                        let runs = volume.data_runs(&inode)?;
                        let Some(first) = runs.first() else {
                            continue;
                        };
                        let contiguous = runs.windows(2).all(|pair| {
                            pair[0].physical_offset + pair[0].length == pair[1].physical_offset
                        }) && runs.iter().map(|run| run.length).sum::<u64>()
                            >= child.size;
                        let extension = child
                            .name
                            .rsplit_once('.')
                            .map(|(_, extension)| extension.to_ascii_lowercase())
                            .unwrap_or_default();
                        writeln!(
                            output,
                            "{}\t{}\t{}\t{}\t{}",
                            first.physical_offset,
                            child.size,
                            u8::from(contiguous),
                            extension,
                            child_path.replace(['\t', '\n'], " ")
                        )?;
                        files += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    output.flush()?;
    println!("files: {files} in {:.2?}", started.elapsed());
    Ok(())
}
