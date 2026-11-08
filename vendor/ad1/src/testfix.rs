//! Spec-faithful AD1 fixture builder (test-only; enable the `testfix` feature).
//!
//! Shared by `ad1-core`'s and `ad1-forensic`'s integration tests so the encoder
//! lives in one place. Never compiled into a normal/published build.
//!
//! Encodes a logical tree into AD1 bytes following the al3ks1s/AD1-tools layout
//! (see `docs/format.md`). File data is compressed with **flate2** and stored
//! hashes are computed with **RustCrypto md-5/sha1** — libraries independent of
//! `ad1-core`, so an addressing / chunk-table / hash bug is still caught even
//! though both encoder and decoder share the structural offsets.
//!
//! Layout order places the item tree + metadata *first* (so it lives in the
//! first segment) and bulk file data *after* it, mirroring how a missing later
//! segment breaks data reads while leaving the tree intact.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::needless_range_loop,
    clippy::cast_possible_truncation,
    clippy::items_after_statements,
    clippy::identity_op,
    dead_code
)]

use flate2::write::ZlibEncoder;
use flate2::Compression;
use md5::{Digest as _, Md5};
use sha1::Sha1;
use std::io::Write as _;

pub const MARGIN: usize = 512;
pub const CHUNK_SIZE: u32 = 0x1_0000; // 64 KiB, the typical AD1 chunk size

/// Logical input tree.
pub enum Node {
    File(&'static str, Vec<u8>),
    Dir(&'static str, Vec<Node>),
}

/// What the reader is expected to surface for one entry (DFS order).
#[derive(Debug, Clone)]
pub struct Expected {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub md5: Option<String>,
    pub sha1: Option<String>,
    pub data: Option<Vec<u8>>,
}

/// A built single-segment AD1 image plus the expected per-entry facts.
pub struct Built {
    pub bytes: Vec<u8>,
    pub expected: Vec<Expected>,
}

struct Item {
    name: String,
    is_dir: bool,
    data: Option<Vec<u8>>,
    parent: Option<usize>,
    first_child: Option<usize>,
    next_sibling: Option<usize>,
    path: String,
    offset: u64,
    meta_addr: u64,
    zlib_addr: u64,
    size: u64,
    md5: Option<String>,
    sha1: Option<String>,
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn flatten(tree: Vec<Node>) -> Vec<Item> {
    let mut items = Vec::new();
    fn rec(node: Node, parent: Option<usize>, parent_path: &str, items: &mut Vec<Item>) -> usize {
        let (name, is_dir, data, children) = match node {
            Node::File(n, d) => (n.to_string(), false, Some(d), Vec::new()),
            Node::Dir(n, c) => (n.to_string(), true, None, c),
        };
        let path = if parent_path.is_empty() {
            name.clone()
        } else {
            format!("{parent_path}/{name}")
        };
        let idx = items.len();
        items.push(Item {
            name,
            is_dir,
            data,
            parent,
            first_child: None,
            next_sibling: None,
            path,
            offset: 0,
            meta_addr: 0,
            zlib_addr: 0,
            size: 0,
            md5: None,
            sha1: None,
        });
        let my_path = items[idx].path.clone();
        let mut prev_child: Option<usize> = None;
        for child in children {
            let cidx = rec(child, Some(idx), &my_path, items);
            match prev_child {
                None => items[idx].first_child = Some(cidx),
                Some(p) => items[p].next_sibling = Some(cidx),
            }
            prev_child = Some(cidx);
        }
        idx
    }
    for node in tree {
        rec(node, None, "", &mut items);
    }
    items
}

fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

/// Build a single-segment AD1 image from a tree whose root is a single `Dir`.
pub fn build(tree: Node) -> Built {
    let mut items = flatten(vec![tree]);
    let header_area = 0x100usize; // logical header lives in [0, 0x100)
    let mut l = vec![0u8; header_area]; // logical region (index == logical addr)

    let append = |l: &mut Vec<u8>, bytes: &[u8]| -> u64 {
        let o = l.len() as u64;
        l.extend_from_slice(bytes);
        o
    };

    // 1. Per-file size + hashes (no layout dependency).
    for it in &mut items {
        if it.is_dir {
            continue;
        }
        let data = it.data.clone().unwrap_or_default();
        it.size = data.len() as u64;
        it.md5 = Some(hex(&Md5::digest(&data)));
        it.sha1 = Some(hex(&Sha1::digest(&data)));
    }

    // 2. Reserve item records FIRST so the tree lands in segment 1.
    for i in 0..items.len() {
        let name_len = items[i].name.len();
        let size = 0x30 + name_len + 8;
        let off = append(&mut l, &vec![0u8; size]);
        items[i].offset = off;
    }

    // 3. Metadata lists (append records in reverse to chain `next`).
    for i in 0..items.len() {
        let mut records: Vec<(u32, u32, Vec<u8>)> = Vec::new(); // (category, key, data)
        if !items[i].is_dir {
            if let Some(m) = &items[i].md5 {
                records.push((0x01, 0x5001, m.clone().into_bytes()));
            }
            if let Some(s) = &items[i].sha1 {
                records.push((0x01, 0x5002, s.clone().into_bytes()));
            }
            records.push((0x03, 0x03, items[i].size.to_le_bytes().to_vec()));
        }
        records.push((0x05, 0x08, b"20241130T101500".to_vec())); // modified ts
        let mut next: u64 = 0;
        let mut first: u64 = 0;
        for (cat, key, data) in records.into_iter().rev() {
            let mut rec = Vec::new();
            rec.extend_from_slice(&next.to_le_bytes());
            rec.extend_from_slice(&cat.to_le_bytes());
            rec.extend_from_slice(&key.to_le_bytes());
            rec.extend_from_slice(&(data.len() as u32).to_le_bytes());
            rec.extend_from_slice(&data);
            let addr = append(&mut l, &rec);
            next = addr;
            first = addr;
        }
        items[i].meta_addr = first;
    }

    // 4. Bulk file data: chunks + chunk table (may span later segments).
    for i in 0..items.len() {
        if items[i].is_dir {
            continue;
        }
        let data = items[i].data.clone().unwrap_or_default();
        if data.is_empty() {
            continue; // zero-byte file: no chunk table
        }
        let mut addrs: Vec<u64> = Vec::new();
        for chunk in data.chunks(CHUNK_SIZE as usize) {
            let comp = zlib(chunk);
            addrs.push(append(&mut l, &comp));
        }
        addrs.push(l.len() as u64); // closing address = end of last chunk
        let count = (addrs.len() - 1) as u64;
        let mut table = Vec::new();
        table.extend_from_slice(&count.to_le_bytes());
        for a in &addrs {
            table.extend_from_slice(&a.to_le_bytes());
        }
        items[i].zlib_addr = append(&mut l, &table);
    }

    // 5. Write item fields now that all offsets are known.
    for i in 0..items.len() {
        let it = &items[i];
        let o = it.offset as usize;
        let next = it.next_sibling.map_or(0, |s| items[s].offset);
        let child = it.first_child.map_or(0, |c| items[c].offset);
        let parent = it.parent.map_or(0, |p| items[p].offset);
        put_u64(&mut l, o + 0x00, next);
        put_u64(&mut l, o + 0x08, child);
        put_u64(&mut l, o + 0x10, it.meta_addr);
        put_u64(&mut l, o + 0x18, it.zlib_addr);
        put_u64(&mut l, o + 0x20, it.size);
        put_u32(&mut l, o + 0x28, if it.is_dir { 5 } else { 0 });
        put_u32(&mut l, o + 0x2c, it.name.len() as u32);
        l[o + 0x30..o + 0x30 + it.name.len()].copy_from_slice(it.name.as_bytes());
        put_u64(&mut l, o + 0x30 + it.name.len(), parent);
    }

    // 6. Logical header (physical 0x200 == logical 0).
    let root_off = items[0].offset;
    l[0..15].copy_from_slice(b"ADLOGICALIMAGE\0");
    put_u32(&mut l, 0x10, 4); // image_version = 4
    put_u32(&mut l, 0x18, CHUNK_SIZE);
    put_u64(&mut l, 0x1c, 0); // logical_metadata_addr
    put_u64(&mut l, 0x24, root_off); // first_item_addr
    let dsn = b"TestSource";
    put_u32(&mut l, 0x2c, dsn.len() as u32);
    l[0x30..0x33].copy_from_slice(b"AD\0");
    put_u64(&mut l, 0x34, 0x5c); // data_source_name_addr (logical)
    l[0x5c..0x5c + dsn.len()].copy_from_slice(dsn);

    // 7. Segment header + final assembly.
    let total = (MARGIN + l.len()) as u64;
    let fragments_size = total.div_ceil(0x1_0000).max(1) as u32;
    let mut seg = vec![0u8; MARGIN];
    seg[0..16].copy_from_slice(b"ADSEGMENTEDFILE\0");
    put_u32(&mut seg, 0x18, 1); // segment_index
    put_u32(&mut seg, 0x1c, 1); // segment_number (single segment)
    put_u32(&mut seg, 0x22, fragments_size);
    put_u32(&mut seg, 0x28, MARGIN as u32); // header_size

    let mut bytes = seg;
    bytes.extend_from_slice(&l);

    let expected = items
        .iter()
        .map(|it| Expected {
            path: it.path.clone(),
            is_dir: it.is_dir,
            size: it.size,
            md5: it.md5.clone(),
            sha1: it.sha1.clone(),
            data: it.data.clone(),
        })
        .collect();

    Built { bytes, expected }
}

/// Pseudo-random, ~incompressible bytes (so file data genuinely spans segments).
pub fn incompressible(len: usize) -> Vec<u8> {
    let mut x: u32 = 0x1234_5678;
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        })
        .collect()
}

/// A small canonical tree used across tests.
pub fn sample_tree() -> Node {
    Node::Dir(
        "root",
        vec![
            Node::File("hello.txt", b"Hello, AD1!\n".to_vec()),
            Node::Dir(
                "sub",
                vec![
                    Node::File("a.bin", incompressible(200_000)),
                    Node::File("empty.dat", Vec::new()),
                ],
            ),
        ],
    )
}

/// Split a single-segment image's logical region across `n` segments, rewriting
/// each segment header. Used by the multi-segment test.
pub fn split(single: &[u8], n: u32, fragments_size: u32) -> Vec<Vec<u8>> {
    let logical = &single[MARGIN..];
    let stride = (fragments_size as usize) * 0x1_0000 - MARGIN;
    let mut out = Vec::new();
    for i in 0..n {
        let start = (i as usize) * stride;
        if start >= logical.len() && i != 0 {
            break;
        }
        let end = ((i as usize + 1) * stride).min(logical.len());
        let mut seg = vec![0u8; MARGIN];
        seg[0..16].copy_from_slice(b"ADSEGMENTEDFILE\0");
        put_u32(&mut seg, 0x18, i + 1);
        put_u32(&mut seg, 0x1c, n);
        put_u32(&mut seg, 0x22, fragments_size);
        put_u32(&mut seg, 0x28, MARGIN as u32);
        seg.extend_from_slice(&logical[start..end]);
        out.push(seg);
    }
    out
}
