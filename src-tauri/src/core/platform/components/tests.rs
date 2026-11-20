use std::path::{Path, PathBuf};

use super::*;

#[test]
fn components_are_searched_beside_the_executable_only() {
    let release = Path::new(r"C:\Program Files\LovelyCarve");
    assert_eq!(component_directories(release), vec![release.to_path_buf()]);
}

#[test]
fn cargo_test_and_example_binaries_also_search_their_profile_directory() {
    let profile = PathBuf::from("target").join("debug");
    for subdirectory in ["deps", "examples"] {
        assert_eq!(
            component_directories(&profile.join(subdirectory)),
            vec![profile.join(subdirectory), profile.clone()]
        );
    }
    assert_eq!(
        component_directories(&profile.join("plugins")),
        vec![profile.join("plugins")]
    );
}

#[test]
fn the_newer_cargo_build_dir_layout_also_finds_the_profile_directory() {
    let profile = PathBuf::from("target").join("debug");
    let test_directory = profile.join("build/lovelycarve/0123456789abcdef/out");
    assert_eq!(
        component_directories(&test_directory),
        vec![test_directory.clone(), profile]
    );
    let unrelated = Path::new("evidence/export/case/out");
    assert_eq!(
        component_directories(unrelated),
        vec![unrelated.to_path_buf()]
    );
}

#[test]
fn this_test_binary_searches_its_cargo_profile_directory() {
    // Whatever layout the running Cargo uses, the test binary sits below the
    // profile directory, where `just build-components` puts the libraries.
    let directories = search_directories().expect("search directories");
    assert_eq!(directories.len(), 2, "{directories:?}");
}

#[test]
fn a_missing_component_names_the_library_and_the_build_recipe() {
    let error = match Component::load("lovelycarve_missing_component") {
        Ok(component) => panic!("unexpected component at {}", component.path().display()),
        Err(error) => format!("{error:#}"),
    };
    let file_name = libloading::library_filename("lovelycarve_missing_component");
    assert!(
        error.starts_with(&format!("缺少组件 {}", file_name.to_string_lossy())),
        "{error}"
    );
    assert!(error.contains("just build-components"), "{error}");
}
