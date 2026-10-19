use std::path::Path;

use super::*;
use crate::core::{
    analysis::identify::systems::SystemMount,
    filesystem::{fs_all, tree::fs_tree},
};

/// A tar evidence: a Linux root file system with relative, absolute and
/// looping links, and a separate home directory tree.
fn tar_evidence(directory: &Path) -> Arc<EvidenceSession> {
    let path = directory.join("evidence.tar");
    let file = std::fs::File::create(&path).expect("tar file");
    let mut builder = tar::Builder::new(file);
    let mut add_file = |name: &str, content: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(&mut header, name, content)
            .expect("tar entry");
    };
    add_file("rootfs/etc/passwd", b"root:x:0:0:root:/root:/bin/bash\n");
    add_file("rootfs/usr/lib/os-release", b"ID=kali\n");
    add_file("home/kali/.bash_history", b"id\n");
    add_file("home/kali/notes.txt", b"notes\n");
    let mut add_link = |name: &str, target: &str| {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_entry_type(tar::EntryType::Symlink);
        builder
            .append_link(&mut header, name, target)
            .expect("tar link");
    };
    add_link("rootfs/etc/os-release", "../usr/lib/os-release");
    add_link("rootfs/etc/absolute", "/usr/lib/os-release");
    add_link("rootfs/etc/loop", "loop");
    add_link("rootfs/home", "/srv/home");
    builder.finish().expect("finish tar");
    drop(builder);
    Arc::new(EvidenceSession::open(&path).expect("open tar evidence"))
}

fn child(image: &Arc<EvidenceSession>, parent: &str, name: &str) -> String {
    fs_tree::children_for_image(image, parent)
        .expect("children")
        .into_iter()
        .find(|node| node.name == name)
        .unwrap_or_else(|| panic!("{name} below {parent}"))
        .id
}

fn system_view(image: &Arc<EvidenceSession>) -> HostView {
    let source = fs_all::directory_root(image, "root").expect("tar root");
    let rootfs = child(image, &source, "rootfs");
    let home = child(image, &source, "home");
    let mount = |path: &str, node: &str, node_path: &str, source: &'static str| SystemMount {
        path: path.to_owned(),
        root: "root".to_owned(),
        node: node.to_owned(),
        node_path: node_path.to_owned(),
        source,
        listed: true,
    };
    let system = DetectedSystem {
        id: "linux@root/rootfs".to_owned(),
        family: "linux".to_owned(),
        product: "Kali".to_owned(),
        version: None,
        label: "Kali · root/rootfs".to_owned(),
        root: "root".to_owned(),
        mounts: vec![
            mount("", &rootfs, "rootfs", "root"),
            mount("srv/home", &home, "home", "fstab"),
        ],
    };
    HostView::system("view-1".to_owned(), &system, Arc::clone(image))
}

fn located(view: &HostView, path: &str) -> (String, String, String) {
    let resolved = view.resolve(path).expect("resolve").expect("found");
    let Located::Node {
        root, root_path, ..
    } = resolved.located
    else {
        panic!("{path} is virtual");
    };
    (resolved.view_path, root, root_path)
}

#[test]
fn system_views_follow_links_across_mounts_and_stay_inside() {
    let directory = tempfile::tempdir().expect("evidence");
    let image = tar_evidence(directory.path());
    let view = system_view(&image);

    assert_eq!(
        located(&view, "etc/passwd"),
        (
            "etc/passwd".to_owned(),
            "root".to_owned(),
            "rootfs/etc/passwd".to_owned()
        )
    );
    // Relative and absolute links land on the same file.
    for link in ["etc/os-release", "etc/absolute", "/etc/./os-release"] {
        assert_eq!(
            located(&view, link),
            (
                "usr/lib/os-release".to_owned(),
                "root".to_owned(),
                "rootfs/usr/lib/os-release".to_owned()
            ),
            "{link}"
        );
    }
    // A link into another mount.
    assert_eq!(
        located(&view, "home/kali/.bash_history"),
        (
            "srv/home/kali/.bash_history".to_owned(),
            "root".to_owned(),
            "home/kali/.bash_history".to_owned()
        )
    );
    // Loops end; `..` outside a link target is refused.
    assert!(view.resolve("etc/loop").is_ok());
    assert!(matches!(
        view.resolve("../etc/passwd"),
        Err(PluginError::Rpc { code, .. }) if code == "VIEW_PATH_INVALID"
    ));
    assert!(view.resolve("etc/missing").expect("resolve").is_none());
    assert_eq!(
        view.stat("ETC/PASSWD").expect("stat").map(|node| node.size),
        Some(32)
    );

    // `srv` exists only as the parent of a mount.
    let root = view.list("", 0, 512).expect("list root");
    let names: Vec<&str> = root.nodes.iter().map(|node| node.name.as_str()).collect();
    assert!(
        names.contains(&"etc") && names.contains(&"srv"),
        "{names:?}"
    );
    let srv = view.list("srv", 0, 512).expect("list srv");
    assert_eq!(
        srv.nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        vec!["home"]
    );
    let page = view.list("srv/home/kali", 1, 1).expect("page");
    assert_eq!((page.total, page.nodes.len()), (2, 1));
    assert!(view.list("etc/passwd", 0, 10).is_err());
}

#[test]
fn finds_walk_mounts_and_page_by_cursor() {
    let directory = tempfile::tempdir().expect("evidence");
    let image = tar_evidence(directory.path());
    let view = system_view(&image);
    let mut scan =
        FindScan::new(&["**/.bash_history".to_owned(), "etc/passwd".to_owned()]).expect("scan");
    let mut paths = Vec::new();
    while !scan.is_complete() {
        paths.extend(view.find_page(&mut scan).expect("page"));
    }
    paths.sort();
    assert_eq!(paths, vec!["etc/passwd", "srv/home/kali/.bash_history"]);
    assert!(FindScan::new(&[]).is_err());
    assert!(FindScan::new(&["../x".to_owned()]).is_err());
}

#[test]
fn evidence_views_show_every_volume_as_a_directory() {
    let directory = tempfile::tempdir().expect("evidence");
    let image = tar_evidence(directory.path());
    let view = HostView::evidence("view-2".to_owned(), "evidence.tar".to_owned(), image);
    assert_eq!(view.info().mounts.len(), 1);
    assert_eq!(view.info().mounts[0].path, "root");
    let root = view.list("", 0, 512).expect("root");
    assert_eq!(
        root.nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        vec!["root"]
    );
    let mut scan = FindScan::new(&["*/rootfs/etc/passwd".to_owned()]).expect("scan");
    let mut paths = Vec::new();
    while !scan.is_complete() {
        paths.extend(view.find_page(&mut scan).expect("page"));
    }
    assert_eq!(paths, vec!["root/rootfs/etc/passwd"]);
    let file = view
        .locate_file("root/rootfs/etc/passwd")
        .expect("open")
        .expect("file");
    assert_eq!(
        (
            file.view_path.as_str(),
            file.root.as_str(),
            file.root_path.as_str(),
            file.node.size
        ),
        ("root/rootfs/etc/passwd", "root", "rootfs/etc/passwd", 32)
    );
    assert_eq!(
        view.read("root/rootfs/etc/passwd", 0, 4).expect("read"),
        b"root".to_vec()
    );
    assert!(matches!(
        view.locate_file("root/rootfs/etc"),
        Err(PluginError::Rpc { code, .. }) if code == "VIEW_NOT_A_FILE"
    ));
}
