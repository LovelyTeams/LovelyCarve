//! The market end to end on a local market directory: an index generated
//! from packages, listed against the installed plugins, downloaded with its
//! digest checked, inspected and installed.

use std::{io::Write as _, path::Path};

use lovelycarve_plugin::{
    abi::{HostApiV1, PluginApiV1},
    runtime, Plugin,
};

use super::*;
use crate::core::extensions::plugin::{digest_file, PackageChange};

const PLUGIN_ID: &str = "org.test.market";

unsafe extern "C" fn market_entry(host: *const HostApiV1, plugin: *mut PluginApiV1) -> i32 {
    unsafe { runtime::entry(host, plugin, || Plugin::new(PLUGIN_ID)) }
}

/// A library header of this build's platform, so the package offers it.
fn native_library() -> (String, Vec<u8>) {
    let x86_64 = std::env::consts::ARCH == "x86_64";
    match std::env::consts::OS {
        "windows" => {
            let mut bytes = vec![0_u8; 0x100];
            bytes[..2].copy_from_slice(b"MZ");
            bytes[0x3C..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
            bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
            let machine: u16 = if x86_64 { 0x8664 } else { 0xAA64 };
            bytes[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
            ("market.dll".to_owned(), bytes)
        }
        "macos" => {
            let mut bytes = vec![0_u8; 32];
            bytes[..4].copy_from_slice(&0xFEED_FACF_u32.to_le_bytes());
            let cpu: u32 = if x86_64 { 0x0100_0007 } else { 0x0100_000C };
            bytes[4..8].copy_from_slice(&cpu.to_le_bytes());
            ("libmarket.dylib".to_owned(), bytes)
        }
        _ => {
            let mut bytes = vec![0_u8; 64];
            bytes[..4].copy_from_slice(b"\x7fELF");
            bytes[5] = 1;
            let machine: u16 = if x86_64 { 0x3E } else { 0xB7 };
            bytes[18..20].copy_from_slice(&machine.to_le_bytes());
            ("libmarket.so".to_owned(), bytes)
        }
    }
}

fn write_package(path: &Path, version: &str) {
    let (library, bytes) = native_library();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).expect("package"));
    let options = zip::write::SimpleFileOptions::default();
    let manifest = format!(
        "id = \"{PLUGIN_ID}\"\nname = \"市场测试\"\nversion = \"{version}\"\ndescription = \"测试\"\nprotocol = 2\nlibrary = \"market\"\n"
    );
    for (name, content) in [
        ("plugin.toml".to_owned(), manifest.into_bytes()),
        (library, bytes),
        (
            "locales/en.toml".to_owned(),
            b"\"plugin.name\" = \"Market test\"\n\"analyzer.none\" = \"x\"\n".to_vec(),
        ),
    ] {
        writer.start_file(name, options).expect("entry");
        writer.write_all(&content).expect("content");
    }
    writer.finish().expect("finish");
}

struct Market {
    _data: tempfile::TempDir,
    _install: tempfile::TempDir,
    directory: tempfile::TempDir,
    market: PluginMarket,
    manager: PluginManager,
    source: String,
}

async fn market() -> Market {
    let data = tempfile::tempdir().expect("data");
    let install = tempfile::tempdir().expect("install");
    let directory = tempfile::tempdir().expect("market directory");
    write_package(&directory.path().join("market-1.0.0.lcpkg"), "1.0.0");
    generate_index(directory.path(), "实验室", None).expect("index");
    let market = PluginMarket::new(data.path());
    let source = market
        .set_sources(&[directory.path().to_string_lossy().into_owned()])
        .await
        .expect("sources")
        .remove(0);
    let manager = PluginManager::discover(&[], Some(install.path().to_owned())).expect("manager");
    manager.serve_in_process(PLUGIN_ID, market_entry);
    Market {
        _data: data,
        _install: install,
        directory,
        market,
        manager,
        source,
    }
}

#[tokio::test]
async fn sources_are_stored_in_their_stable_spelling() {
    let data = tempfile::tempdir().expect("data");
    let market = PluginMarket::new(data.path());
    assert_eq!(market.sources().expect("none yet"), Vec::<String>::new());
    let stored = market
        .set_sources(&["https://plugins.example.com/lab/".to_owned()])
        .await
        .expect("sources");
    assert_eq!(stored, vec!["https://plugins.example.com/lab/index.json"]);
    assert_eq!(market.sources().expect("stored"), stored);
    for bad in [
        vec!["http://plugins.example.com/index.json".to_owned()],
        vec![
            "https://plugins.example.com/lab/".to_owned(),
            "https://plugins.example.com/lab/index.json".to_owned(),
        ],
    ] {
        assert!(market.set_sources(&bad).await.is_err(), "{bad:?}");
    }
    assert_eq!(market.sources().expect("unchanged"), stored);
    assert!(matches!(
        market
            .index("https://elsewhere.example.com/index.json", false)
            .await,
        Err(MarketError::UnknownSource(_))
    ));
}

#[tokio::test]
async fn a_local_market_lists_downloads_and_installs() {
    let fixture = market().await;
    let listing = fixture
        .market
        .catalog(&fixture.manager, &fixture.source, false)
        .await
        .expect("catalog");
    assert_eq!(listing.title, "实验室");
    let package = &listing.packages[0];
    assert_eq!(
        (
            package.id.as_str(),
            package.latest.as_deref(),
            package.status,
            package.description.as_deref()
        ),
        (
            PLUGIN_ID,
            Some("1.0.0"),
            MarketStatus::NotInstalled,
            Some("测试")
        )
    );
    // Only the name and description translations reach the index.
    assert_eq!(
        package.locales["en"].keys().collect::<Vec<_>>(),
        vec!["plugin.name"]
    );

    let download = fixture
        .market
        .download(&fixture.manager, &fixture.source, PLUGIN_ID, "1.0.0")
        .await
        .expect("download");
    assert_eq!(download.inspection.change, PackageChange::New);
    assert_eq!(
        digest_file(Path::new(&download.archive))
            .expect("digest")
            .sha256,
        download.sha256
    );
    fixture
        .manager
        .install(download.archive.clone().into(), &download.sha256)
        .await
        .expect("install");
    fixture.market.release(&download.sha256).expect("release");
    assert!(!Path::new(&download.archive).exists());
    let listing = fixture
        .market
        .catalog(&fixture.manager, &fixture.source, false)
        .await
        .expect("catalog");
    assert_eq!(listing.packages[0].status, MarketStatus::Installed);

    // An unchanged directory gives the same index.
    let index = fixture.directory.path().join(INDEX_FILE);
    let before = std::fs::read(&index).expect("index");
    generate_index(fixture.directory.path(), "实验室", None).expect("again");
    assert_eq!(std::fs::read(&index).expect("index"), before);
}

#[tokio::test]
async fn downloads_must_match_what_the_index_lists() {
    let fixture = market().await;
    fixture
        .market
        .index(&fixture.source, false)
        .await
        .expect("first read");
    let index_path = fixture.directory.path().join(INDEX_FILE);
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&index_path).expect("index")).expect("json");

    // The listed digest is not the file's.
    let listed = index["packages"][0]["versions"][0]["packages"][0]["sha256"].clone();
    index["packages"][0]["versions"][0]["packages"][0]["sha256"] = "c".repeat(64).into();
    std::fs::write(&index_path, serde_json::to_vec(&index).expect("json")).expect("write");
    // The cached index still lists the right digest until a refresh.
    let cached = fixture
        .market
        .download(&fixture.manager, &fixture.source, PLUGIN_ID, "1.0.0")
        .await
        .expect("cached index");
    fixture.market.release(&cached.sha256).expect("release");
    fixture
        .market
        .index(&fixture.source, true)
        .await
        .expect("refresh");
    let error = fixture
        .market
        .download(&fixture.manager, &fixture.source, PLUGIN_ID, "1.0.0")
        .await
        .expect_err("digest mismatch");
    assert!(matches!(error, MarketError::Integrity { .. }), "{error}");

    // The right bytes, but the package is another version.
    index["packages"][0]["versions"][0]["packages"][0]["sha256"] = listed;
    index["packages"][0]["versions"][0]["version"] = "1.0.1".into();
    std::fs::write(&index_path, serde_json::to_vec(&index).expect("json")).expect("write");
    fixture
        .market
        .index(&fixture.source, true)
        .await
        .expect("refresh");
    let error = fixture
        .market
        .download(&fixture.manager, &fixture.source, PLUGIN_ID, "1.0.1")
        .await
        .expect_err("version mismatch");
    assert!(matches!(error, MarketError::Mismatch { .. }), "{error}");
    assert_eq!(
        std::fs::read_dir(fixture.market.downloads.as_path())
            .map(|entries| entries.count())
            .unwrap_or_default(),
        0,
        "rejected downloads are removed"
    );
}
