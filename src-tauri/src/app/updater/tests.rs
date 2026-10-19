use super::*;

const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn github_client_initializes_tls_without_plugin_startup() {
    client().expect("the startup GitHub check must be able to create an HTTPS client");
}

#[tokio::test]
async fn cli_host_rejects_installation_before_network_or_shutdown() {
    let host = crate::host::AppHandle::new(None, |_, _| panic!("no update progress expected"));
    assert!(!host.is_desktop());
    let error = install(&host, "invalid-selected-version")
        .await
        .unwrap_err();
    assert_eq!(error.code, crate::app::error::ErrorCode::InvalidInput);
    assert!(error.message.contains("desktop application"));
}

fn release(tag: &str) -> GithubRelease {
    let name = format!("LovelyCarve_{}_x64-setup.exe", tag.trim_start_matches('v'));
    serde_json::from_value(serde_json::json!({
        "tag_name": tag,
        "draft": false,
        "prerelease": false,
        "assets": [{
            "name": name,
            "browser_download_url": format!("{DOWNLOAD_PREFIX}{tag}/{name}"),
            "digest": format!("sha256:{ABC_SHA256}"),
            "size": 3
        }]
    }))
    .unwrap()
}

#[test]
fn legacy_release_at_current_version_does_not_require_a_manifest() {
    assert_eq!(
        release("v1.1.1")
            .info(&Version::parse("1.1.1").unwrap())
            .unwrap(),
        ReleaseUpdateInfo {
            available: false,
            version: "1.1.1".into(),
            signed: false,
            can_install: supports_installation(),
        }
    );
}

#[test]
fn compares_semantic_versions_and_never_downgrades() {
    assert!(
        release("v1.10.0")
            .info(&Version::parse("1.9.0").unwrap())
            .unwrap()
            .available
    );
    assert!(
        !release("v1.9.0")
            .info(&Version::parse("1.10.0").unwrap())
            .unwrap()
            .available
    );
    assert!(
        release("v1.1.1")
            .info(&Version::parse("1.1.1-beta.1").unwrap())
            .unwrap()
            .available
    );
    assert!(version("vv1.1.1").is_err());
}

#[test]
fn a_new_release_without_an_installer_is_still_an_update() {
    let mut release = release("v1.2.0");
    release.assets.clear();
    let info = release.info(&Version::parse("1.1.1").unwrap()).unwrap();
    assert!(info.available);
    assert!(!info.can_install);
}

#[test]
fn rejects_other_repositories_cli_packages_and_missing_or_malformed_digests() {
    for url in [
        "https://github.com/attacker/LovelyCarve/releases/download/v1.2.0/LovelyCarve_1.2.0_x64-setup.exe",
        "https://github.com/LovelyTeams/LovelyCarve/releases/download/v1.1.0/LovelyCarve_1.2.0_x64-setup.exe",
        "http://github.com/LovelyTeams/LovelyCarve/releases/download/v1.2.0/LovelyCarve_1.2.0_x64-setup.exe",
    ] {
        let mut release = release("v1.2.0");
        release.assets[0].browser_download_url = url.into();
        assert!(release.installer().is_err());
    }
    for digest in [
        None,
        Some("sha256:1234"),
        Some("sha256:gg"),
        Some("sha1:1234"),
    ] {
        let mut release = release("v1.2.0");
        release.assets[0].digest = digest.map(str::to_owned);
        assert!(release.installer().is_err());
    }
    let mut release = release("v1.2.0");
    release.assets[0].name = "lovelycarve-cli-v1.2.0-win-x64.zip".into();
    assert!(release.installer().is_err());
}

#[test]
fn recognizes_signed_metadata_only_from_the_same_release() {
    let mut release = release("v1.2.0");
    release.assets.push(GithubAsset {
        name: "latest.json".into(),
        browser_download_url: format!("{DOWNLOAD_PREFIX}v1.2.0/latest.json"),
        digest: None,
        size: 123,
    });
    assert!(
        release
            .info(&Version::parse("1.1.1").unwrap())
            .unwrap()
            .signed
    );
    release.assets[1].browser_download_url = "https://example.org/latest.json".into();
    assert!(!release.has_manifest());
}

#[test]
fn incomplete_or_corrupt_downloads_are_rejected_before_launch() {
    let expected = hex::decode(ABC_SHA256).unwrap();
    let actual = Sha256::digest(b"abc");
    assert!(verify_download(3, 3, actual.as_slice(), &expected).is_ok());
    assert!(verify_download(2, 3, actual.as_slice(), &expected).is_err());
    assert!(verify_download(3, 3, &[0; 32], &expected).is_err());
}

#[test]
fn nsis_uses_quiet_update_and_restart_flags() {
    let command = installer_command(
        Path::new("C:/example directory"),
        Path::new("C:/installed app"),
    );
    assert_eq!(
        command.get_program(),
        Path::new("C:/example directory")
            .join("setup.exe")
            .as_os_str()
    );
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        ["/S", "/UPDATE", "/R", "/D=C:/installed app"]
    );
}

#[tokio::test]
#[ignore = "requires direct access to api.github.com"]
async fn live_github_check() {
    let release = latest_release(&client().unwrap()).await.unwrap();
    let info = release
        .info(&Version::parse(env!("CARGO_PKG_VERSION")).unwrap())
        .unwrap();
    println!("{}", serde_json::to_string(&info).unwrap());
}
