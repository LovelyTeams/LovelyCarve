//! Mounting a backup through the VFS abstraction (ADR-0010).
//!
//! These drive `IosBackupOpen` exactly as a mounting tool does — probe a
//! directory, open it with a `CredentialSource`, walk the result — so what is
//! covered is the path 4n6mount will take, not a private helper.

#![cfg(feature = "vfs")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use forensic_vfs::{
    Confidence, Credential, CredentialSource, EncryptionScheme, FileId, FileSystem, NoCredentials,
    NodeKind, TreeOpen, VfsError,
};
use ios_backup_core::vfs::IosBackupOpen;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name)
}

/// The default in `tools/mint_encrypted_backup.py`, which minted the fixture.
const FIXTURE_PASSWORD: &str = "test-password-1234";

/// Offers one password for anything that asks.
struct OnePassword(&'static str);
impl CredentialSource for OnePassword {
    fn credentials_for(&self, _s: EncryptionScheme, _t: &str) -> Vec<Credential> {
        vec![Credential::Password(self.0.to_string())]
    }
}

/// Records what it was asked for, so the scheme reaching the provider is
/// observable rather than assumed.
struct RecordingCreds(std::sync::Mutex<Vec<EncryptionScheme>>);
impl CredentialSource for RecordingCreds {
    fn credentials_for(&self, s: EncryptionScheme, _t: &str) -> Vec<Credential> {
        self.0.lock().unwrap().push(s);
        Vec::new()
    }
}

fn children(fs: &dyn FileSystem, id: FileId) -> Vec<(Vec<u8>, FileId, NodeKind)> {
    let mut out = Vec::new();
    for e in fs.read_dir(id).unwrap() {
        let e = e.unwrap();
        out.push((e.name, e.id, e.kind));
    }
    out
}

#[test]
fn a_backup_directory_is_claimed_and_an_ordinary_one_is_not() {
    assert!(matches!(
        IosBackupOpen.probe(&fixture("plain-backup")),
        Confidence::Yes { .. }
    ));

    // The probe demands both index files. A directory holding only one is not a
    // backup, and claiming it would stop resolution on evidence we cannot read.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Manifest.plist"), b"not a backup").unwrap();
    assert_eq!(IosBackupOpen.probe(tmp.path()), Confidence::No);
}

#[test]
fn a_plaintext_backup_mounts_and_its_domain_is_the_top_level_directory() {
    let fs = IosBackupOpen
        .open(&fixture("plain-backup"), &NoCredentials)
        .expect("a plaintext backup needs no credentials");

    let root = fs.root();
    let top = children(fs.as_ref(), root);
    assert!(!top.is_empty(), "the mount exposes at least one domain");
    assert!(
        top.iter().all(|(_, _, k)| *k == NodeKind::Dir),
        "every top-level node is a domain directory, got {top:?}"
    );
}

#[test]
fn a_files_bytes_read_back_through_the_mount() {
    let fs = IosBackupOpen
        .open(&fixture("plain-backup"), &NoCredentials)
        .unwrap();

    // Walk to the first file anywhere in the tree.
    let mut stack = vec![fs.root()];
    let mut found = None;
    while let Some(id) = stack.pop() {
        for (name, child, kind) in children(fs.as_ref(), id) {
            match kind {
                NodeKind::Dir => stack.push(child),
                NodeKind::File => {
                    found = Some((name, child));
                    stack.clear();
                    break;
                }
                _ => {}
            }
        }
    }
    let (name, id) = found.expect("the fixture holds at least one file");

    let meta = fs.meta(id).unwrap();
    let mut buf = vec![0u8; usize::try_from(meta.size).unwrap() + 16];
    let n = fs
        .read_at(id, forensic_vfs::StreamId::Default, 0, &mut buf)
        .unwrap();

    assert_eq!(
        n as u64,
        meta.size,
        "read_at returns exactly the size meta reported for {}",
        String::from_utf8_lossy(&name)
    );

    // Reading past the end is an empty read, never an error and never a panic.
    assert_eq!(
        fs.read_at(
            id,
            forensic_vfs::StreamId::Default,
            meta.size + 4096,
            &mut buf
        )
        .unwrap(),
        0
    );
}

#[test]
fn lookup_finds_by_name_and_misses_cleanly() {
    let fs = IosBackupOpen
        .open(&fixture("plain-backup"), &NoCredentials)
        .unwrap();
    let root = fs.root();
    let (name, id, _) = children(fs.as_ref(), root).remove(0);

    assert_eq!(fs.lookup(root, &name).unwrap(), Some(id));
    assert_eq!(fs.lookup(root, b"no-such-domain").unwrap(), None);
}

#[test]
fn an_encrypted_backup_without_a_password_demands_credentials() {
    // Not Ok(an empty mount) and not a decode error: the one condition the
    // caller can act on. Reporting locked evidence as unreadable would send an
    // examiner looking for corruption instead of a password.
    match IosBackupOpen.open(&fixture("encrypted-backup"), &NoCredentials) {
        Err(VfsError::NeedCredentials { scheme, .. }) => assert_eq!(scheme, "ios-backup"),
        other => panic!(
            "expected NeedCredentials, got {:?}",
            other.map(|_| "Ok(fs)")
        ),
    }
}

#[test]
fn an_encrypted_backup_mounts_once_the_password_is_offered() {
    let fs = IosBackupOpen
        .open(&fixture("encrypted-backup"), &OnePassword(FIXTURE_PASSWORD))
        .expect("the fixture unlocks with the generator's password");
    assert!(!children(fs.as_ref(), fs.root()).is_empty());
}

#[test]
fn a_wrong_password_is_reported_as_a_credential_problem_not_corruption() {
    match IosBackupOpen.open(&fixture("encrypted-backup"), &OnePassword("wrong")) {
        Err(VfsError::NeedCredentials { .. }) => {}
        other => panic!(
            "a wrong password must not read as damage, got {:?}",
            other.map(|_| "Ok(fs)")
        ),
    }
}

#[test]
fn the_provider_is_asked_for_the_ios_backup_scheme() {
    // The seam's credential lookup keys on EncryptionScheme. Asking under some
    // unrelated volume scheme would make a scheme-keyed provider answer for the
    // wrong thing, so which scheme is requested is worth pinning.
    let rec = RecordingCreds(std::sync::Mutex::new(Vec::new()));
    let _ = IosBackupOpen.open(&fixture("encrypted-backup"), &rec);

    assert_eq!(
        rec.0.lock().unwrap().as_slice(),
        &[EncryptionScheme::IosBackup]
    );
}

#[test]
fn a_backup_reports_no_deleted_nodes_and_no_unallocated_space() {
    // Both are empty because a capture has neither, which is different from
    // "searched and found none". Asserted so a later change cannot start
    // inventing them.
    let fs = IosBackupOpen
        .open(&fixture("plain-backup"), &NoCredentials)
        .unwrap();
    assert_eq!(fs.deleted().unwrap().count(), 0);
    assert_eq!(fs.unallocated().unwrap().count(), 0);
    assert_eq!(
        fs.extents(fs.root(), forensic_vfs::StreamId::Default)
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn the_mount_describes_itself_without_inventing_a_geometry() {
    let fs = IosBackupOpen
        .open(&fixture("plain-backup"), &NoCredentials)
        .unwrap();

    assert_eq!(fs.kind().as_str(), "ios-backup");
    assert_eq!(fs.timestamp_zone(), forensic_vfs::TimeZonePolicy::Utc);

    // A backup is a directory of files, not a block device. Reporting a sector
    // size would invent a geometry the evidence does not have, and a consumer
    // could compute offsets from it.
    let s = fs.sector_sizes();
    assert_eq!((s.logical, s.physical, s.cluster_or_block), (0, 0, 0));

    let root = fs.meta(fs.root()).unwrap();
    assert_eq!(root.kind, NodeKind::Dir);
    assert_eq!(root.allocated, forensic_vfs::Allocation::Allocated);
}

#[test]
fn an_id_this_mount_did_not_issue_is_refused_rather_than_indexed() {
    let fs = IosBackupOpen
        .open(&fixture("plain-backup"), &NoCredentials)
        .unwrap();

    // A FileId shape from some other filesystem is not a node here. Refusing is
    // the point: silently treating it as an index would read a neighbouring
    // node's metadata and report it as this one's.
    assert!(fs.meta(FileId::NtfsRef { entry: 5, seq: 2 }).is_err());

    // An Opaque id past the end of the tree.
    assert!(fs.meta(FileId::Opaque(u64::MAX)).is_err());
    assert!(fs.read_dir(FileId::Opaque(u64::MAX)).is_err());
}

#[test]
fn reading_something_that_is_not_file_content_yields_no_bytes() {
    let fs = IosBackupOpen
        .open(&fixture("plain-backup"), &NoCredentials)
        .unwrap();
    let mut buf = [0u8; 64];

    // A directory has no content blob.
    assert_eq!(
        fs.read_at(fs.root(), forensic_vfs::StreamId::Default, 0, &mut buf)
            .unwrap(),
        0
    );

    // A backup has no alternate data streams; asking for one is empty, not an
    // error, so a consumer that probes every stream does not fail the mount.
    assert_eq!(
        fs.read_at(fs.root(), forensic_vfs::StreamId::ResourceFork, 0, &mut buf)
            .unwrap(),
        0
    );

    // read_link on a non-symlink is empty rather than a guess.
    assert!(fs.read_link(fs.root(), 4096).unwrap().is_empty());
}

#[test]
fn a_credential_that_is_not_a_password_is_not_treated_as_one() {
    // A keybag has no use for volume key material. Feeding raw bytes in as a
    // passphrase would report a WRONG PASSWORD for a credential that was never
    // a password — a diagnosis that sends an examiner to re-check a password
    // they never supplied.
    struct KeyBytesOnly;
    impl CredentialSource for KeyBytesOnly {
        fn credentials_for(&self, _s: EncryptionScheme, _t: &str) -> Vec<Credential> {
            vec![
                Credential::KeyBytes(vec![0u8; 32]),
                Credential::RecoveryKey("1234-5678".to_string()),
            ]
        }
    }

    match IosBackupOpen.open(&fixture("encrypted-backup"), &KeyBytesOnly) {
        Err(VfsError::NeedCredentials { .. }) => {}
        other => panic!(
            "expected NeedCredentials with no password offered, got {:?}",
            other.map(|_| "Ok(fs)")
        ),
    }
}

#[test]
fn a_damaged_backup_is_a_decode_failure_not_a_credential_prompt() {
    // The distinction the error mapping exists to preserve: corruption must not
    // read as "locked", or an examiner goes looking for a password that would
    // not help.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Manifest.plist"), b"not a plist at all").unwrap();
    std::fs::write(tmp.path().join("Manifest.db"), b"not a database").unwrap();

    assert!(matches!(
        IosBackupOpen.probe(tmp.path()),
        Confidence::Yes { .. }
    ));
    match IosBackupOpen.open(tmp.path(), &NoCredentials) {
        Err(VfsError::Decode { layer, .. }) => assert_eq!(layer, "ios-backup"),
        other => panic!(
            "damage must not report as a credential problem, got {:?}",
            other.map(|_| "Ok(fs)")
        ),
    }
}
