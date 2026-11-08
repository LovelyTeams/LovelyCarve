//! Build-time command discovery must follow declarations rather than scan files.
#[path = "../build/cli_catalog/source.rs"]
mod source;

use std::{fs, path::Path};

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn resolves_flat_nested_legacy_and_explicit_module_paths() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "commands.rs", "pub mod flat; pub mod domain; #[path = \"custom.rs\"] pub mod explicit; pub mod inline { pub mod leaf; }");
    write(root.path(), "commands/flat.rs", "pub fn flat_command() {}");
    write(root.path(), "commands/domain.rs", "pub mod legacy;");
    write(
        root.path(),
        "commands/domain/legacy/mod.rs",
        "pub fn nested_command() {}",
    );
    write(
        root.path(),
        "custom.rs",
        "pub mod child; pub fn explicit_command() {}",
    );
    write(
        root.path(),
        "child.rs",
        "pub fn explicit_child_command() {}",
    );
    write(
        root.path(),
        "commands/inline/leaf.rs",
        "pub fn inline_command() {}",
    );
    let mut sources = source::Sources::new(root.path().join("commands.rs"));
    for (modules, name) in [
        (vec!["flat"], "flat_command"),
        (vec!["domain", "legacy"], "nested_command"),
        (vec!["explicit"], "explicit_command"),
        (vec!["explicit", "child"], "explicit_child_command"),
        (vec!["inline", "leaf"], "inline_command"),
    ] {
        let modules = modules.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            sources
                .function(&modules, name)
                .unwrap()
                .sig
                .ident
                .to_string(),
            name
        );
    }
    assert_eq!(sources.files().count(), 7);
}

#[test]
fn rejects_undeclared_ambiguous_and_private_functions() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        "commands.rs",
        "pub mod ambiguous; pub mod plain;",
    );
    write(root.path(), "commands/ambiguous.rs", "");
    write(root.path(), "commands/ambiguous/mod.rs", "");
    write(root.path(), "commands/plain.rs", "fn helper() {}");
    let mut sources = source::Sources::new(root.path().join("commands.rs"));
    for (module, name, expected) in [
        ("missing", "command", "undeclared"),
        ("ambiguous", "command", "ambiguous"),
        ("plain", "missing", "function missing"),
        ("plain", "helper", "not public"),
    ] {
        let error = sources.function(&[module.to_owned()], name).err().unwrap();
        assert!(error.to_string().contains(expected), "{error}");
    }
}
