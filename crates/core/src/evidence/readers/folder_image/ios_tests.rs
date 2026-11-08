use super::*;
use crate::{
    evidence::image::{DerivedImage, DiskImage},
    filesystem::{
        evidence_session::EvidenceSession,
        tree::fs_tree::{self, FsSession},
    },
};
use ios_backup_core::Password;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/ios-backup/tests/data")
        .join(name)
}

#[test]
fn ios_tree_reconstructs_domains_parents_and_prefix_reads() {
    let folder = FolderImage::open(fixture("plain-backup")).unwrap();
    assert_eq!(
        folder
            .children("")
            .iter()
            .map(|entry| entry.rel_path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "AppDomain-com.example.app",
            "CameraRollDomain",
            "HomeDomain",
            "__backup"
        ]
    );
    assert_eq!(
        folder
            .read(
                "HomeDomain/Library/Preferences/com.apple.example.plist",
                usize::MAX
            )
            .unwrap(),
        b"tiny"
    );
    assert_eq!(
        folder.read("HomeDomain/Library/SMS/sms.db", 16).unwrap(),
        b"SQLite format 3\0"
    );
    assert!(folder.lookup("HomeDomain/Library").is_some());
    assert!(folder.lookup("__backup/Manifest.db").is_some());
    assert!(!folder.ios_info().unwrap().locked);
}

#[test]
fn encrypted_backup_keeps_raw_tree_and_unlock_creates_child_session() {
    let source = DiskImage::open(fixture("encrypted-backup")).unwrap();
    assert!(source.info().ios_backup.unwrap().locked);
    assert!(source
        .folder
        .as_ref()
        .unwrap()
        .lookup("Manifest.db")
        .is_some());
    assert!(source
        .unlock_ios(Password::new("wrong"), &CancellationToken::new())
        .is_err());
    let child = source
        .unlock_ios(
            Password::new("test-password-1234"),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(
        child.derived.as_ref().map(DerivedImage::parent),
        Some(source.id.as_str())
    );
    assert!(child.path.to_string_lossy().starts_with("decrypted://ios/"));
    assert!(!child.info().ios_backup.unwrap().locked);
    assert_eq!(
        child
            .folder
            .unwrap()
            .read("AppDomain-com.example.app/Documents/exact.bin", usize::MAX)
            .unwrap(),
        vec![b'B'; 32]
    );
}

#[test]
fn ios_files_are_available_through_normal_session_reads_and_ranges() {
    let image = Arc::new(EvidenceSession::open(fixture("plain-backup")).unwrap());
    let node = format!("folder:{}", hex::encode("HomeDomain/Library/SMS/sms.db"));
    let mut session = FsSession::new(&image);
    let expected = fs_tree::read_file(&image, &node).unwrap();
    assert_eq!(
        session.read_file_range(&node, 17, 25).unwrap(),
        expected[17..42]
    );
}

#[test]
fn ios_audit_preserves_codes_and_unrated_domain_findings() {
    let folder = FolderImage::open(fixture("plain-backup")).unwrap();
    let findings = folder.audit_ios(&CancellationToken::new()).unwrap();
    let findings = findings.as_array().unwrap();
    assert!(findings
        .iter()
        .any(|finding| finding["code"] == "IOS-BACKUP-UNENCRYPTED"));
    assert!(findings
        .iter()
        .any(|finding| finding["code"] == "IOS-BACKUP-DOMAIN-ABSENT"
            && finding["severity"].is_null()));
    let token = CancellationToken::new();
    token.cancel();
    assert!(folder.audit_ios(&token).is_err());
}

#[test]
fn ios_missing_size_uses_true_plaintext_length() {
    let folder = FolderImage::open_ios(
        &fixture("no-size-encrypted"),
        Password::new("test-password-1234"),
        &CancellationToken::new(),
    )
    .unwrap();
    let path = "HomeDomain/Library/Preferences/com.apple.example.plist";
    assert_eq!(folder.lookup(path).unwrap().size, 4);
    assert_eq!(folder.read(path, usize::MAX).unwrap(), b"tiny");
}

#[test]
fn ios_short_blob_never_succeeds_as_complete_file() {
    let temporary = tempfile::tempdir().unwrap();
    for name in [
        "Manifest.db",
        "Manifest.plist",
        "Info.plist",
        "Status.plist",
    ] {
        fs::copy(
            fixture("plain-backup").join(name),
            temporary.path().join(name),
        )
        .unwrap();
    }
    let id = "3d0d7e5fb2ce288813306e4d4636395e047a3d28";
    fs::create_dir(temporary.path().join("3d")).unwrap();
    fs::write(temporary.path().join("3d").join(id), b"short").unwrap();
    let folder = FolderImage::open(temporary.path()).unwrap();
    assert!(folder
        .stream_file("HomeDomain/Library/SMS/sms.db", 64 * 1024, |_| Ok(()))
        .is_err());
    assert!(folder.read("HomeDomain/Library/SMS/sms.db", 16).is_err());
}

#[test]
fn ios_unsafe_paths_and_conflicting_tree_entries_are_refused() {
    for (domain, relative) in [
        ("../bad", "a"),
        ("HomeDomain", "../a"),
        ("HomeDomain", "/a"),
        ("HomeDomain", "C:\\secret"),
        ("__backup", "a"),
    ] {
        assert!(super::ios::ensure_safe_path(domain, relative).is_err());
    }
}
