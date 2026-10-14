use super::*;

fn install(root: &Path, directory: &str, id: &str, version: &str) -> PathBuf {
    let directory = root.join(directory);
    fs::create_dir_all(&directory).expect("directory");
    fs::write(
        directory.join("plugin.toml"),
        format!(
            r#"id = "{id}"
name = "Test"
version = "{version}"
protocol_min = 1
protocol_max = 1
[library]
path = "test.dll"
[[capabilities]]
type = "evidence_analyzer"
analyzer = "test"
"#
        ),
    )
    .expect("manifest");
    fs::write(
        directory.join("test.dll"),
        b"test bytes, deliberately not executable",
    )
    .expect("library");
    directory
}

#[test]
fn manual_scan_adds_without_loading_and_preserves_old_snapshot() {
    let temporary = tempfile::tempdir().expect("temp");
    let catalog = PluginCatalog::new(&[temporary.path().to_owned()]);
    catalog.rescan().expect("empty scan");
    let previous = catalog.snapshot();
    install(temporary.path(), "one", "org.test.one", "1");
    let report = catalog.rescan().expect("scan");
    assert_eq!(report.generation, 2);
    assert!(report
        .entries
        .iter()
        .any(|entry| entry.status == PluginScanStatus::Added));
    assert_eq!(catalog.snapshot().definitions.len(), 1);
    assert!(previous.definitions.is_empty());
}

#[test]
fn updates_keep_definition_and_cannot_load_new_bytes_with_old_permissions() {
    let temporary = tempfile::tempdir().expect("temp");
    let directory = install(temporary.path(), "one", "org.test.one", "1");
    let catalog = PluginCatalog::new(&[temporary.path().to_owned()]);
    catalog.rescan().expect("initial");
    let original = catalog.snapshot();
    fs::write(
        directory.join("test.dll"),
        b"replacement with different length",
    )
    .expect("update");
    let report = catalog.rescan().expect("scan");
    assert_eq!(report.entries[0].status, PluginScanStatus::UpdatePending);
    assert_eq!(
        catalog.snapshot().definitions[0].manifest,
        original.definitions[0].manifest
    );
    assert!(catalog.validate_for_load(&original.definitions[0]).is_err());
}

#[test]
fn changed_manifest_displays_new_version_but_preserves_running_version() {
    let temporary = tempfile::tempdir().expect("temp");
    install(temporary.path(), "one", "org.test.one", "1");
    let catalog = PluginCatalog::new(&[temporary.path().to_owned()]);
    catalog.rescan().expect("initial");
    install(temporary.path(), "one", "org.test.one", "2");
    let report = catalog.rescan().expect("update");
    assert_eq!(report.entries[0].version.as_deref(), Some("2"));
    assert_eq!(report.entries[0].status, PluginScanStatus::UpdatePending);
    assert_eq!(catalog.snapshot().definitions[0].manifest.version, "1");
}

#[test]
fn corrupted_manifest_is_reported_and_keeps_existing_definition() {
    let temporary = tempfile::tempdir().expect("temp");
    let directory = install(temporary.path(), "one", "org.test.one", "1");
    let catalog = PluginCatalog::new(&[temporary.path().to_owned()]);
    catalog.rescan().expect("initial");
    fs::write(directory.join("plugin.toml"), "not TOML").expect("corrupt");
    let report = catalog.rescan().expect("scan");
    assert_eq!(report.entries[0].status, PluginScanStatus::Error);
    assert_eq!(report.entries[0].plugin_id.as_deref(), Some("org.test.one"));
    assert_eq!(catalog.snapshot().definitions.len(), 1);
}

#[test]
fn missing_libraries_and_unknown_fields_do_not_publish_new_plugins() {
    let temporary = tempfile::tempdir().expect("temp");
    let first = install(temporary.path(), "one", "org.test.one", "1");
    fs::remove_file(first.join("test.dll")).expect("remove library");
    let second = install(temporary.path(), "two", "org.test.two", "1");
    let path = second.join("plugin.toml");
    let manifest = fs::read_to_string(&path).expect("read");
    fs::write(path, manifest.replace("path =", "patth =")).expect("typo");
    let catalog = PluginCatalog::new(&[temporary.path().to_owned()]);
    let report = catalog.rescan().expect("scan");
    assert!(catalog.snapshot().definitions.is_empty());
    assert_eq!(
        report
            .entries
            .iter()
            .filter(|entry| entry.status == PluginScanStatus::Error)
            .count(),
        2
    );
}

#[test]
fn duplicate_ids_keep_root_priority_and_report_losing_path() {
    let temporary = tempfile::tempdir().expect("temp");
    let first = temporary.path().join("first");
    let second = temporary.path().join("second");
    install(&first, "one", "org.test.one", "1");
    install(&second, "one", "org.test.one", "2");
    let catalog = PluginCatalog::new(&[first, second]);
    let report = catalog.rescan().expect("scan");
    assert_eq!(catalog.snapshot().definitions.len(), 1);
    assert_eq!(catalog.snapshot().definitions[0].manifest.version, "1");
    assert_eq!(report.entries[1].status, PluginScanStatus::Duplicate);
}

#[test]
fn root_inspection_failure_does_not_remove_existing_plugins() {
    let temporary = tempfile::tempdir().expect("temp");
    let root = temporary.path().join("root");
    install(&root, "one", "org.test.one", "1");
    let catalog = PluginCatalog::new(std::slice::from_ref(&root));
    catalog.rescan().expect("initial");
    fs::rename(&root, temporary.path().join("saved")).expect("move root");
    fs::write(&root, b"root is now a file").expect("replace root");
    let report = catalog.rescan().expect("scan");
    assert!(report
        .entries
        .iter()
        .any(|entry| entry.status == PluginScanStatus::Error));
    assert_eq!(catalog.snapshot().definitions.len(), 1);
}

#[test]
fn removal_revokes_catalog_but_keeps_captured_definition_and_history() {
    let temporary = tempfile::tempdir().expect("temp");
    let directory = install(temporary.path(), "one", "org.test.one", "1");
    let catalog = PluginCatalog::new(&[temporary.path().to_owned()]);
    catalog.rescan().expect("initial");
    let captured = catalog.snapshot();
    fs::rename(directory, temporary.path().join("saved")).expect("move");
    fs::remove_file(temporary.path().join("saved/plugin.toml")).expect("hide manifest");
    let report = catalog.rescan().expect("remove");
    assert!(catalog.snapshot().definitions.is_empty());
    assert_eq!(captured.definitions[0].manifest.id, "org.test.one");
    assert!(report
        .entries
        .iter()
        .any(|entry| entry.status == PluginScanStatus::Removed));
    install(temporary.path(), "one", "org.test.one", "2");
    let report = catalog.rescan().expect("reinstall");
    assert!(report
        .entries
        .iter()
        .any(|entry| entry.status == PluginScanStatus::UpdatePending));
    assert_eq!(catalog.snapshot().definitions[0].manifest.version, "1");
}

#[test]
fn missing_root_can_be_created_and_close_blocks_late_scans() {
    let temporary = tempfile::tempdir().expect("temp");
    let root = temporary.path().join("new");
    let catalog = PluginCatalog::new(std::slice::from_ref(&root));
    assert_eq!(
        catalog.rescan().expect("missing").entries[0].status,
        PluginScanStatus::MissingRoot
    );
    install(&root, "one", "org.test.one", "1");
    catalog.rescan().expect("created");
    catalog.close();
    let generation = catalog.snapshot().report.generation;
    assert!(catalog.rescan().is_err());
    assert_eq!(catalog.snapshot().report.generation, generation);
}

#[test]
fn concurrent_scans_publish_unique_ordered_generations() {
    let temporary = tempfile::tempdir().expect("temp");
    install(temporary.path(), "one", "org.test.one", "1");
    let catalog = Arc::new(PluginCatalog::new(&[temporary.path().to_owned()]));
    let handles = (0..4)
        .map(|_| {
            let catalog = catalog.clone();
            std::thread::spawn(move || catalog.rescan().expect("scan").generation)
        })
        .collect::<Vec<_>>();
    let mut generations = handles
        .into_iter()
        .map(|handle| handle.join().expect("join"))
        .collect::<Vec<_>>();
    generations.sort();
    assert_eq!(generations, [1, 2, 3, 4]);
    assert_eq!(catalog.snapshot().definitions.len(), 1);
}
