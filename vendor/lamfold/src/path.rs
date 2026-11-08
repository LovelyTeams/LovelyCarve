//! Path-based access over a [`FoldFrontend`] — the host-consumable surface.
//!
//! A frontend is node-based (`root` → `lookup` → `read_at`), but a bootloader
//! asks for files **by path** ("read `/boot/vmlinuz`"). This module turns a path
//! into a node by walking components through `lookup`, following symlinks
//! POSIX-style (so `/vmlinuz` → `vmlinuz-6.12` resolves), and offers
//! `read_path` / `read_dir_path` / `metadata_path` on top.
//!
//! It is frontend-agnostic — the same code drives iso, udf, squashfs, erofs,
//! cramfs, romfs, and cpio — so it is the seam a host (LamBoot's `FsBackend`)
//! adapts to: build the frontend over a `BlockSource`, then read by path here.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::error::{FoldError, Result};
use crate::frontend::{DirEntry, FileKind, FoldFrontend, Metadata, NodeId, SubstrateCtx};
use crate::read_cap::checked_full_read_len;
use crate::source::BlockSource;

/// Maximum symlinks followed while resolving one path (loop/`ELOOP` guard).
pub const MAX_SYMLINKS: u32 = 16;

/// Resolve an absolute-or-relative `path` to a node, following symlinks. Leading
/// `/`, empty components, and `.` are ignored; `..` is rejected (a read-only
/// boot reader has no use for parent traversal and it invites escapes).
pub fn resolve<S, F>(fs: &mut F, cx: &mut SubstrateCtx<'_>, path: &str) -> Result<NodeId>
where
    S: BlockSource,
    F: FoldFrontend<S>,
{
    let mut node = fs.root();
    let mut pending: Vec<String> = Vec::new();
    push_components(path, &mut pending);
    let mut links = 0u32;

    while let Some(comp) = pending.pop() {
        if comp == ".." {
            return Err(FoldError::InvalidPath("'..' not supported"));
        }
        let child = fs.lookup(node, &comp, cx)?.ok_or(FoldError::NotFound)?;

        if fs.metadata(child, cx)?.kind == FileKind::Symlink {
            links += 1;
            if links > MAX_SYMLINKS {
                return Err(FoldError::InvalidPath("too many symlinks"));
            }
            let target = fs
                .read_link(child, cx)?
                .ok_or(FoldError::Corrupt("symlink without a target"))?;
            let target = core::str::from_utf8(&target)
                .map_err(|_| FoldError::InvalidPath("non-UTF-8 symlink"))?;
            // An absolute target restarts from the root; a relative one resolves
            // against the directory the link lives in (node stays put). The rest
            // of the original path stays queued after the target's components.
            if target.starts_with('/') {
                node = fs.root();
            }
            push_components(target, &mut pending);
        } else {
            node = child;
        }
    }
    Ok(node)
}

/// Read a whole file by path (symlinks followed), bounded by the substrate read
/// cap. Errors with [`FoldError::IsDirectory`] if the path is a directory.
pub fn read_path<S, F>(fs: &mut F, cx: &mut SubstrateCtx<'_>, path: &str) -> Result<Vec<u8>>
where
    S: BlockSource,
    F: FoldFrontend<S>,
{
    let node = resolve(fs, cx, path)?;
    let md = fs.metadata(node, cx)?;
    if md.kind == FileKind::Directory {
        return Err(FoldError::IsDirectory);
    }
    let total = checked_full_read_len(md.size)?;
    let mut out = vec![0u8; total];
    let mut done = 0;
    while done < total {
        let n = fs.read_at(node, done as u64, &mut out[done..], cx)?;
        if n == 0 {
            break;
        }
        done += n;
    }
    out.truncate(done);
    Ok(out)
}

/// Directory listing by path (symlinks followed).
pub fn read_dir_path<S, F>(
    fs: &mut F,
    cx: &mut SubstrateCtx<'_>,
    path: &str,
) -> Result<Vec<DirEntry>>
where
    S: BlockSource,
    F: FoldFrontend<S>,
{
    let node = resolve(fs, cx, path)?;
    fs.read_dir(node, cx)
}

/// Metadata by path (symlinks followed — so this reports the *target's* kind).
pub fn metadata_path<S, F>(fs: &mut F, cx: &mut SubstrateCtx<'_>, path: &str) -> Result<Metadata>
where
    S: BlockSource,
    F: FoldFrontend<S>,
{
    let node = resolve(fs, cx, path)?;
    fs.metadata(node, cx)
}

/// Push `path`'s components onto `stack` reversed, so `stack.pop()` yields them
/// left-to-right (and a symlink's target is processed before the queued rest).
fn push_components(path: &str, stack: &mut Vec<String>) {
    for c in path.split('/').filter(|c| !c.is_empty() && *c != ".").rev() {
        stack.push(c.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SliceSource;
    use alloc::collections::BTreeMap;

    // A minimal in-memory frontend: a fixed tree with files and symlinks, to
    // exercise path resolution independently of any on-disk format.
    struct MockNode {
        kind: FileKind,
        data: Vec<u8>,
        children: BTreeMap<String, NodeId>,
    }
    struct MockFs {
        nodes: Vec<MockNode>,
    }
    impl MockFs {
        fn build() -> Self {
            // 0=/ ; 1=/a (dir) ; 2=/a/file "hello" ; 3=/rel -> a/file ;
            // 4=/abs -> /a/file ; 5=/loop -> loop ; 6=/a/up -> ../a/file
            let mut nodes = vec![
                MockNode {
                    kind: FileKind::Directory,
                    data: vec![],
                    children: BTreeMap::new(),
                },
                MockNode {
                    kind: FileKind::Directory,
                    data: vec![],
                    children: BTreeMap::new(),
                },
                MockNode {
                    kind: FileKind::Regular,
                    data: b"hello".to_vec(),
                    children: BTreeMap::new(),
                },
                MockNode {
                    kind: FileKind::Symlink,
                    data: b"a/file".to_vec(),
                    children: BTreeMap::new(),
                },
                MockNode {
                    kind: FileKind::Symlink,
                    data: b"/a/file".to_vec(),
                    children: BTreeMap::new(),
                },
                MockNode {
                    kind: FileKind::Symlink,
                    data: b"loop".to_vec(),
                    children: BTreeMap::new(),
                },
            ];
            nodes[0].children.insert("a".into(), 1);
            nodes[0].children.insert("rel".into(), 3);
            nodes[0].children.insert("abs".into(), 4);
            nodes[0].children.insert("loop".into(), 5);
            nodes[1].children.insert("file".into(), 2);
            MockFs { nodes }
        }
    }
    impl FoldFrontend<SliceSource<'_>> for MockFs {
        const TAG: &'static str = "mock";
        fn probe(_s: &mut SliceSource<'_>) -> Result<bool> {
            Ok(true)
        }
        fn open(_s: SliceSource<'_>, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
            Ok(MockFs::build())
        }
        fn root(&self) -> NodeId {
            0
        }
        fn lookup(
            &mut self,
            dir: NodeId,
            name: &str,
            _cx: &mut SubstrateCtx<'_>,
        ) -> Result<Option<NodeId>> {
            Ok(self.nodes[dir as usize].children.get(name).copied())
        }
        fn read_dir(&mut self, dir: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
            Ok(self.nodes[dir as usize]
                .children
                .iter()
                .map(|(n, &c)| DirEntry {
                    name: n.clone(),
                    node: c,
                    kind: self.nodes[c as usize].kind,
                })
                .collect())
        }
        fn metadata(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Metadata> {
            let n = &self.nodes[node as usize];
            Ok(Metadata {
                kind: n.kind,
                size: n.data.len() as u64,
                mode: 0,
            })
        }
        fn read_at(
            &mut self,
            node: NodeId,
            off: u64,
            buf: &mut [u8],
            _cx: &mut SubstrateCtx<'_>,
        ) -> Result<usize> {
            let d = &self.nodes[node as usize].data;
            let start = off as usize;
            let n = core::cmp::min(buf.len(), d.len().saturating_sub(start));
            buf[..n].copy_from_slice(&d[start..start + n]);
            Ok(n)
        }
        fn read_link(
            &mut self,
            node: NodeId,
            _cx: &mut SubstrateCtx<'_>,
        ) -> Result<Option<Vec<u8>>> {
            let n = &self.nodes[node as usize];
            Ok((n.kind == FileKind::Symlink).then(|| n.data.clone()))
        }
    }

    fn ctx() -> (crate::BlockCache, crate::NoVerifier) {
        (crate::BlockCache::new(0), crate::NoVerifier)
    }

    #[test]
    fn resolves_plain_and_symlinks() {
        let (mut cache, nov) = ctx();
        let mut cx = SubstrateCtx {
            cache: &mut cache,
            verifier: &nov,
        };
        let mut fs = MockFs::build();
        assert_eq!(read_path(&mut fs, &mut cx, "/a/file").unwrap(), b"hello");
        assert_eq!(read_path(&mut fs, &mut cx, "a/file").unwrap(), b"hello"); // leading slash optional
        assert_eq!(
            read_path(&mut fs, &mut cx, "/./a/./file").unwrap(),
            b"hello"
        ); // '.' ignored
           // relative symlink resolves against its directory
        assert_eq!(read_path(&mut fs, &mut cx, "/rel").unwrap(), b"hello");
        // absolute symlink restarts at root
        assert_eq!(read_path(&mut fs, &mut cx, "/abs").unwrap(), b"hello");
    }

    #[test]
    fn rejects_loops_dotdot_and_missing() {
        let (mut cache, nov) = ctx();
        let mut cx = SubstrateCtx {
            cache: &mut cache,
            verifier: &nov,
        };
        let mut fs = MockFs::build();
        assert!(matches!(
            read_path(&mut fs, &mut cx, "/loop"),
            Err(FoldError::InvalidPath(_))
        ));
        assert!(matches!(
            resolve(&mut fs, &mut cx, "/a/../a"),
            Err(FoldError::InvalidPath(_))
        ));
        assert!(matches!(
            read_path(&mut fs, &mut cx, "/missing"),
            Err(FoldError::NotFound)
        ));
        assert!(matches!(
            read_path(&mut fs, &mut cx, "/a"),
            Err(FoldError::IsDirectory)
        ));
        // read_dir by path
        let names: Vec<String> = read_dir_path(&mut fs, &mut cx, "/a")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["file".to_string()]);
    }
}
