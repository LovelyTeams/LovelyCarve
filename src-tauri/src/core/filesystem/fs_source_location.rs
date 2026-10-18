//! Resolve plugin provenance against live evidence or verified offline indexes.
use std::{collections::VecDeque, path::PathBuf, sync::Arc};

use rusqlite::OptionalExtension;

use {
    crate::core::catalog::filedb::{filedb_file, filedb_query},
    crate::core::filesystem::evidence_session::EvidenceSession,
    crate::core::filesystem::fs_all::FsAll,
    crate::core::filesystem::tree::fs_tree::{FsNode, FsSession, NodeKind},
};

#[derive(Clone, Copy)]
enum PathRole {
    Ancestor,
    Target,
}

/// Prefer an active name. A unique deleted file may be the final target, but
/// deleted directories never become traversal ancestors.
fn source_node(nodes: Vec<FsNode>, name: &str, role: PathRole) -> anyhow::Result<Option<FsNode>> {
    if let Some(node) = nodes.iter().find(|node| node.name == name && !node.deleted) {
        return Ok(Some(node.clone()));
    }
    if matches!(role, PathRole::Ancestor) {
        return Ok(None);
    }
    let mut deleted = nodes.into_iter().filter(|node| {
        node.name == name && node.deleted && matches!(node.kind, NodeKind::File | NodeKind::Symlink)
    });
    let node = deleted.next();
    anyhow::ensure!(deleted.next().is_none(), "同名删除来源不唯一：{name}");
    Ok(node)
}

/// Navigation ancestors followed by the target (a final symlink stays visible;
/// a unique deleted final file can be selected for forensic recovery).
pub fn source_chain(
    image: &Arc<EvidenceSession>,
    root: &str,
    path: &str,
) -> anyhow::Result<Vec<FsNode>> {
    anyhow::ensure!(path.len() <= 32_768, "来源路径过长");
    anyhow::ensure!(
        FsAll::for_image(image).vol_id_of(root).is_some(),
        "来源卷不存在：{root}"
    );
    let mut session = FsSession::new(image);
    let mut ancestors = Vec::new();
    if root != "root" {
        let parent = if root.starts_with("host-docker:") {
            Some("host-docker:".to_owned())
        } else if let Some(rest) = root.strip_prefix("lvm:") {
            rest.split(':').next().map(|part| format!("part:{part}"))
        } else {
            None
        };
        let mut parent_id = "root".to_owned();
        for id in parent.into_iter().chain(std::iter::once(root.to_owned())) {
            let node = session
                .children(&parent_id)?
                .into_iter()
                .find(|node| node.id == id && !node.deleted)
                .ok_or_else(|| anyhow::anyhow!("来源卷入口不可用：{id}"))?;
            parent_id = node.id.clone();
            ancestors.push(node);
        }
    }
    resolve_source_path(root, path, ancestors, |parent| session.children(parent))
}

/// Resolve against verified, published offline indexes. A logical volume may
/// be a subtree of a partition publication rather than its own index root.
pub fn indexed_source_chain(
    image: &EvidenceSession,
    indexes: &[PathBuf],
    root: &str,
    path: &str,
) -> anyhow::Result<Vec<FsNode>> {
    let roots = image
        .indexed_roots()
        .ok_or_else(|| anyhow::anyhow!("当前检材不是离线索引会话"))?;
    for index in indexes {
        let connection = filedb_file::open_reader(index)?;
        let metadata = filedb_file::read_meta(&connection)?;
        let Some(boundary) = roots.iter().find(|node| node.id == metadata.volume_key) else {
            continue;
        };
        let mut ancestors = Vec::new();
        let mut parent = root.to_owned();
        while parent != boundary.id {
            anyhow::ensure!(ancestors.len() < 256, "来源卷祖先链过深或包含循环");
            let row = connection
                .query_row(
                    "SELECT node_json, pid FROM filedb WHERE nid = ?1",
                    [&parent],
                    |row| {
                        Ok((
                            filedb_query::deserialize_node(row, 0)?,
                            row.get::<_, String>(1)?,
                        ))
                    },
                )
                .optional()?;
            let Some((node, parent_id)) = row else {
                break;
            };
            anyhow::ensure!(
                node.kind == NodeKind::Dir && !node.deleted,
                "来源卷入口不可用：{}",
                node.id
            );
            parent = parent_id;
            ancestors.push(node);
        }
        if parent != boundary.id {
            continue;
        }
        if boundary.id != "root" {
            ancestors.push(boundary.clone());
        }
        ancestors.reverse();
        return resolve_source_path(root, path, ancestors, |parent| {
            filedb_query::children(index, parent)?.ok_or_else(|| {
                anyhow::anyhow!("此目录未收录在已有索引中，请恢复源镜像后重试：{parent}")
            })
        });
    }
    anyhow::bail!("来源卷未收录在已有索引中，请恢复源镜像后重试：{root}")
}

fn resolve_source_path(
    root: &str,
    path: &str,
    mut ancestors: Vec<FsNode>,
    mut children: impl FnMut(&str) -> anyhow::Result<Vec<FsNode>>,
) -> anyhow::Result<Vec<FsNode>> {
    anyhow::ensure!(path.len() <= 32_768, "来源路径过长");
    let mut stack: Vec<FsNode> = Vec::new();
    let mut components: VecDeque<String> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    anyhow::ensure!(components.len() <= 256, "来源路径层级过深");
    let mut hops = 0;
    let mut steps = 0;
    while let Some(component) = components.pop_front() {
        steps += 1;
        anyhow::ensure!(steps <= 1024, "来源路径解析超出限制");
        if component == "." {
            continue;
        }
        if component == ".." {
            anyhow::ensure!(stack.pop().is_some(), "来源路径越过所选卷根目录");
            continue;
        }
        let parent = stack.last().map_or(root, |node| node.id.as_str());
        let role = if components.is_empty() {
            PathRole::Target
        } else {
            PathRole::Ancestor
        };
        let node = source_node(children(parent)?, &component, role)?
            .ok_or_else(|| anyhow::anyhow!("源文件不存在：{path}（卷 {root}）"))?;
        if node.kind == NodeKind::Symlink && !components.is_empty() {
            hops += 1;
            anyhow::ensure!(hops <= 16, "来源路径包含循环或过多符号链接");
            let target = node
                .link_target
                .as_deref()
                .filter(|target| !target.is_empty())
                .ok_or_else(|| anyhow::anyhow!("无法解析来源目录链接"))?;
            if target.starts_with('/') {
                stack.clear();
            }
            let mut linked: VecDeque<String> = target
                .split('/')
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect();
            linked.append(&mut components);
            components = linked;
        } else {
            anyhow::ensure!(
                components.is_empty() || node.kind == NodeKind::Dir,
                "来源路径中间节点不是目录"
            );
            stack.push(node);
        }
    }
    ancestors.extend(stack);
    Ok(ancestors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::catalog::filedb::filedb_store;
    use crate::core::evidence::image::{DiskImage, ImageFormat};

    #[test]
    fn offline_lvm_sources_use_partition_index_and_keep_volume_boundaries() -> anyhow::Result<()> {
        use filedb_store::test_support::{entry, source};
        let directory = tempfile::tempdir()?;
        let index = directory.path().join("index.db");
        let volume = entry("part:1", "lvm:1:1", "root", NodeKind::Dir, "/root");
        let etc = entry("lvm:1:1", "xfs:etc", "etc", NodeKind::Dir, "/root/etc");
        let passwd = entry(
            "xfs:etc",
            "xfs:passwd",
            "passwd",
            NodeKind::File,
            "/root/etc/passwd",
        );
        let mut alias = entry(
            "lvm:1:1",
            "xfs:alias",
            "alias",
            NodeKind::Symlink,
            "/root/alias",
        );
        alias.node.link_target = Some("/etc".into());
        let mut cycle = entry(
            "lvm:1:1",
            "xfs:cycle",
            "cycle",
            NodeKind::Symlink,
            "/root/cycle",
        );
        cycle.node.link_target = Some("cycle".into());
        let empty = entry(
            "lvm:1:1",
            "xfs:empty",
            "empty",
            NodeKind::Dir,
            "/root/empty",
        );
        filedb_store::build_index_from_entries(
            &[
                volume.clone(),
                etc.clone(),
                passwd.clone(),
                alias.clone(),
                cycle,
                empty,
            ],
            &index,
            &source("part:1"),
        )?;
        let boundary = FsNode {
            id: "part:1".into(),
            name: "分区1".into(),
            parent_id: None,
            ..volume.node.clone()
        };
        let image = EvidenceSession::from_indexed_image(
            DiskImage::from_index_metadata(
                "saved".into(),
                directory.path().join("missing.E01"),
                ImageFormat::Ewf,
                4096,
            )?,
            vec![boundary.clone()],
        );
        let indexes = vec![index];
        for path in ["/etc/passwd", "alias/passwd", "etc/../etc/passwd"] {
            assert_eq!(
                serde_json::to_value(indexed_source_chain(&image, &indexes, "lvm:1:1", path)?)?,
                serde_json::to_value(vec![
                    boundary.clone(),
                    volume.node.clone(),
                    etc.node.clone(),
                    passwd.node.clone()
                ])?
            );
        }
        assert_eq!(
            serde_json::to_value(
                indexed_source_chain(&image, &indexes, "lvm:1:1", "alias")?.last()
            )?,
            serde_json::to_value(Some(&alias.node))?
        );
        assert_eq!(
            serde_json::to_value(indexed_source_chain(&image, &indexes, "lvm:1:1", "/")?)?,
            serde_json::to_value(vec![boundary, volume.node])?
        );
        for path in ["../root/etc/passwd", "etc/Passwd", "cycle/passwd"] {
            assert!(indexed_source_chain(&image, &indexes, "lvm:1:1", path).is_err());
        }
        assert!(indexed_source_chain(&image, &indexes, "lvm:1:99", "etc/passwd").is_err());
        // A truncated walk may include the directory entry without its children.
        let connection = rusqlite::Connection::open(&indexes[0])?;
        connection.execute("DELETE FROM filedb_walked_dirs WHERE pid = 'xfs:empty'", [])?;
        assert!(
            indexed_source_chain(&image, &indexes, "lvm:1:1", "empty/passwd")
                .expect_err("unindexed directory")
                .to_string()
                .contains("此目录未收录")
        );
        Ok(())
    }

    #[test]
    #[ignore = "set LOVELYCARVE_LOCATE_INDEX_TEST to the reported partition index"]
    fn reported_offline_server_index_locates_passwd() -> anyhow::Result<()> {
        let index =
            PathBuf::from(std::env::var_os("LOVELYCARVE_LOCATE_INDEX_TEST").expect("index path"));
        let connection = filedb_file::open_reader(&index)?;
        let metadata = filedb_file::read_meta(&connection)?;
        let boundary = FsNode {
            id: metadata.volume_key.clone(),
            name: "分区1".into(),
            parent_id: None,
            kind: NodeKind::Dir,
            size: 0,
            deleted: false,
            inode: None,
            timestamps: Default::default(),
            link_target: None,
            has_snapshot: false,
        };
        let image = EvidenceSession::from_indexed_image(
            DiskImage::from_index_metadata(
                metadata.image_id,
                PathBuf::from("missing.E01"),
                ImageFormat::Ewf,
                42949672960,
            )?,
            vec![boundary],
        );
        let nodes = indexed_source_chain(&image, &[index], "lvm:1:1", "/etc/passwd")?;
        assert_eq!(
            nodes
                .iter()
                .map(|node| (node.id.as_str(), node.name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("part:1", "分区1"),
                ("lvm:1:1", "root"),
                ("xfs:lvm:1:1:2f657463", "etc"),
                ("xfs:lvm:1:1:2f6574632f706173737764", "passwd"),
            ]
        );
        assert_eq!(
            nodes.last().map(|node| (node.kind, node.size)),
            Some((NodeKind::File, 968))
        );
        Ok(())
    }
    #[test]
    fn active_names_win_and_deleted_targets_must_be_unique() {
        let deleted = FsNode {
            id: "deleted:1".into(),
            name: "cache".into(),
            parent_id: None,
            kind: NodeKind::File,
            size: 42,
            deleted: true,
            inode: None,
            timestamps: Default::default(),
            link_target: None,
            has_snapshot: false,
        };
        let active = FsNode {
            id: "active:1".into(),
            deleted: false,
            ..deleted.clone()
        };
        assert_eq!(
            source_node(
                vec![deleted.clone(), active.clone()],
                "cache",
                PathRole::Target
            )
            .unwrap()
            .map(|node| node.id),
            Some(active.id)
        );
        assert_eq!(
            source_node(vec![deleted.clone()], "cache", PathRole::Target)
                .unwrap()
                .map(|node| node.id),
            Some(deleted.id.clone())
        );
        assert!(
            source_node(vec![deleted.clone()], "cache", PathRole::Ancestor)
                .unwrap()
                .is_none()
        );
        assert!(source_node(
            vec![
                deleted.clone(),
                FsNode {
                    id: "deleted:2".into(),
                    ..deleted.clone()
                }
            ],
            "cache",
            PathRole::Target
        )
        .is_err());
        assert!(source_node(
            vec![FsNode {
                kind: NodeKind::Dir,
                ..deleted
            }],
            "cache",
            PathRole::Target
        )
        .unwrap()
        .is_none());
    }
    #[test]
    fn archive_directory_links_are_bounded_and_final_links_stay_visible() {
        let temporary = tempfile::tempdir().expect("fixture");
        let path = temporary.path().join("source.tar");
        let mut archive = tar::Builder::new(std::fs::File::create(&path).expect("archive"));
        for (name, target, contents) in [
            ("etc/passwd", None, b"root".as_slice()),
            ("alias", Some("etc"), b"".as_slice()),
            ("final", Some("etc/passwd"), b"".as_slice()),
            ("cycle-a", Some("cycle-b"), b"".as_slice()),
            ("cycle-b", Some("cycle-a"), b"".as_slice()),
            ("escape", Some("../../etc"), b"".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(contents.len() as u64);
            if let Some(target) = target {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name(target).expect("link");
            }
            header.set_cksum();
            archive
                .append_data(&mut header, name, contents)
                .expect("entry");
        }
        archive.finish().expect("finish archive");
        drop(archive);
        let image = Arc::new(EvidenceSession::open(&path).expect("tar image"));
        let root = FsSession::new(&image)
            .children("root")
            .expect("root")
            .remove(0);
        let prefix = root.name;
        let nodes = source_chain(&image, "root", &format!("{prefix}/alias/passwd"))
            .expect("directory link");
        assert_eq!(nodes.last().expect("target").name, "passwd");
        assert_eq!(
            source_chain(&image, "root", &format!("{prefix}/final"))
                .expect("final link")
                .last()
                .expect("link")
                .kind,
            NodeKind::Symlink
        );
        assert!(source_chain(&image, "root", &format!("{prefix}/cycle-a/passwd")).is_err());
        assert!(source_chain(&image, "root", &format!("{prefix}/escape/passwd")).is_err());
    }

    #[test]
    fn folder_sources_need_no_index_and_preserve_case() {
        let directory = tempfile::tempdir().expect("fixture");
        std::fs::create_dir(directory.path().join("etc")).expect("etc");
        std::fs::write(directory.path().join("etc/passwd"), b"root").expect("file");
        let image = Arc::new(EvidenceSession::open(directory.path()).expect("folder image"));
        let root = FsSession::new(&image)
            .children("root")
            .expect("folder roots")
            .into_iter()
            .next()
            .expect("folder root");
        let prefix = &root.name;
        let nodes = source_chain(&image, "root", &format!("/{prefix}/etc/passwd")).expect("source");
        assert_eq!(
            nodes
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            [prefix.as_str(), "etc", "passwd"]
        );
        assert!(source_chain(&image, "root", "../etc/passwd").is_err());
        assert!(source_chain(&image, "root", &format!("{prefix}/etc/Passwd")).is_err());
        assert!(source_chain(&image, "root", &format!("{prefix}/etc/missing")).is_err());
        assert!(source_chain(&image, "part:99", "etc/passwd").is_err());
    }

    #[test]
    #[ignore = "set LOVELYCARVE_LOCATE_VMDK_TEST to the Kali VMDK"]
    fn real_plugin_sources_locate_without_index() {
        let path = std::env::var_os("LOVELYCARVE_LOCATE_VMDK_TEST").expect("image path");
        let image = Arc::new(EvidenceSession::open(&path).expect("VMDK"));
        for (root, path) in [
            ("part:0", "var/lib/dpkg/status"),
            ("part:0", "etc/os-release"),
            ("part:0", "lib/systemd/system/ssh.service"),
            (
                "host-docker:14d2197f1eeea329b444226f95809830f5f342debad13057078fd0f21cf701e7:",
                "etc/passwd",
            ),
        ] {
            let nodes = source_chain(&image, root, path).expect("source chain");
            let last = nodes.last().expect("target");
            assert_eq!(last.name, path.rsplit('/').next().expect("filename"));
            let parent = nodes
                .iter()
                .rev()
                .nth(1)
                .map_or("root", |node| node.id.as_str());
            assert!(FsSession::new(&image)
                .children(parent)
                .expect("navigate")
                .iter()
                .any(|node| node.id == last.id));
        }
    }
}
