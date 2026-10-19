//! GitHub release discovery and verified Windows installer update paths.
#[cfg(any(feature = "desktop", test))]
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, ensure, Context};
use reqwest::{redirect::Policy, Client};
use semver::Version;
use serde::{Deserialize, Serialize};
#[cfg(any(feature = "desktop", test))]
use sha2::{Digest, Sha256};
#[cfg(feature = "desktop")]
use tokio::io::AsyncWriteExt;

const RELEASE_API: &str = "https://api.github.com/repos/LovelyTeams/LovelyCarve/releases/latest";
const DOWNLOAD_PREFIX: &str = "https://github.com/LovelyTeams/LovelyCarve/releases/download/";
const MAX_INSTALLER_BYTES: u64 = 512 * 1024 * 1024;
#[cfg(any(feature = "desktop", test))]
const NSIS_UPDATE_ARGS: [&str; 3] = ["/S", "/UPDATE", "/R"];

#[path = "updater/signed.rs"]
#[cfg(feature = "desktop")]
mod signed;
#[cfg(feature = "desktop")]
pub(crate) use signed::download_signed;

fn supports_installation() -> bool {
    #[cfg(feature = "desktop")]
    return crate::host::desktop_supports_update();
    #[cfg(not(feature = "desktop"))]
    false
}

pub(crate) fn current_version() -> anyhow::Result<Version> {
    Ok(Version::parse(env!("CARGO_PKG_VERSION"))?)
}

pub(crate) async fn install(
    app: &crate::host::AppHandle,
    selected_version: &str,
) -> Result<(), crate::app::error::AppError> {
    use crate::app::error::AppError;
    if !app.is_desktop() {
        return Err(AppError::invalid_input(
            "in-app installation requires the desktop application",
        ));
    }
    #[cfg(feature = "desktop")]
    {
        let current = current_version().map_err(AppError::from)?;
        let client = client().map_err(AppError::from)?;
        let release = latest_release(&client).await.map_err(AppError::from)?;
        let info = release.info(&current).map_err(AppError::from)?;
        if !info.available || info.version != selected_version {
            return Err(AppError::source_changed(
                "release changed; check for updates again",
            ));
        }
        if !info.can_install {
            return Err(AppError::invalid_input(
                "this package does not support in-app installation",
            ));
        }
        let progress = |progress| {
            if let Err(error) = app.emit("app-update-progress", progress) {
                tracing::debug!("update progress unavailable: {error}");
            }
        };
        let directory = if info.signed {
            download_signed(app, selected_version, &release, progress).await
        } else {
            download(&client, &current, selected_version, progress).await
        }
        .map_err(AppError::from)?;
        launch_installer(directory).map_err(AppError::from)?;
        app.exit_after_update();
        Ok(())
    }
    #[cfg(not(feature = "desktop"))]
    {
        let _ = selected_version;
        Err(AppError::invalid_input(
            "in-app installation requires the desktop application",
        ))
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
    size: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseUpdateInfo {
    pub available: bool,
    pub version: String,
    pub signed: bool,
    pub can_install: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg(feature = "desktop")]
pub(crate) struct UpdateProgress {
    pub version: String,
    pub downloaded: u64,
    pub total: u64,
    pub installing: bool,
}

pub(crate) fn client() -> anyhow::Result<Client> {
    // reqwest's rustls-no-provider feature requires explicit initialization.
    // Another caller may already have initialized it; keep that provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    Ok(Client::builder()
        .user_agent(concat!("LovelyCarve/", env!("CARGO_PKG_VERSION")))
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(15))
        .redirect(Policy::custom(|attempt| {
            let url = attempt.url();
            if attempt.previous().len() < 5
                && url.scheme() == "https"
                && matches!(
                    url.host_str(),
                    Some(
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                )
            {
                attempt.follow()
            } else {
                attempt.error("unexpected GitHub download redirect")
            }
        }))
        .build()?)
}

pub(crate) async fn latest_release(client: &Client) -> anyhow::Result<GithubRelease> {
    let response = client
        .get(RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await?
        .error_for_status()?;
    let bytes = bounded_body(response, 2 * 1024 * 1024).await?;
    serde_json::from_slice(&bytes).context("invalid GitHub release metadata")
}

async fn bounded_body(mut response: reqwest::Response, limit: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            chunk.len() <= limit.saturating_sub(bytes.len()),
            "GitHub release metadata too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn version(tag: &str) -> anyhow::Result<Version> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).context("invalid GitHub release version")
}

impl GithubRelease {
    fn is_asset(&self, asset: &GithubAsset, name: &str) -> bool {
        asset.name == name
            && asset.browser_download_url == format!("{DOWNLOAD_PREFIX}{}/{name}", self.tag_name)
    }

    fn has_manifest(&self) -> bool {
        self.assets
            .iter()
            .any(|asset| self.is_asset(asset, "latest.json"))
    }

    fn installer(&self) -> anyhow::Result<&GithubAsset> {
        let name = format!("LovelyCarve_{}_x64-setup.exe", version(&self.tag_name)?);
        let asset = self
            .assets
            .iter()
            .find(|asset| self.is_asset(asset, &name))
            .context("the release has no Windows x64 NSIS installer")?;
        ensure!(
            asset.size > 0 && asset.size <= MAX_INSTALLER_BYTES,
            "invalid update installer size"
        );
        asset.sha256()?;
        Ok(asset)
    }

    pub(crate) fn info(&self, current: &Version) -> anyhow::Result<ReleaseUpdateInfo> {
        ensure!(
            !self.draft && !self.prerelease,
            "GitHub latest release must be stable and published"
        );
        let latest = version(&self.tag_name)?;
        Ok(ReleaseUpdateInfo {
            available: latest > *current,
            version: latest.to_string(),
            signed: self.has_manifest(),
            can_install: supports_installation()
                && (self.has_manifest() || self.installer().is_ok()),
        })
    }
}

impl GithubAsset {
    fn sha256(&self) -> anyhow::Result<Vec<u8>> {
        let digest = self
            .digest
            .as_deref()
            .and_then(|value| value.strip_prefix("sha256:"))
            .context("the GitHub asset has no SHA-256 digest")?;
        let decoded = hex::decode(digest).context("invalid GitHub asset SHA-256 digest")?;
        ensure!(decoded.len() == 32, "invalid GitHub asset SHA-256 digest");
        Ok(decoded)
    }
}

/// Download and verify only the version the user selected. A release with a
/// signed manifest must use Tauri; a signature failure never falls back here.
#[cfg(feature = "desktop")]
pub(crate) async fn download(
    client: &Client,
    current: &Version,
    selected_version: &str,
    progress: impl Fn(UpdateProgress),
) -> anyhow::Result<tempfile::TempDir> {
    ensure!(
        supports_installation(),
        "in-app updates require a Windows x64 NSIS or portable build"
    );
    let release = latest_release(client).await?;
    let info = release.info(current)?;
    ensure!(
        info.available && info.version == selected_version,
        "release changed; check for updates again"
    );
    ensure!(!info.signed, "signed releases must use the signed updater");
    let asset = release.installer()?;
    let expected = asset.sha256()?;
    let directory = tempfile::Builder::new()
        .prefix("LovelyCarve-update-")
        .tempdir()?;
    let path = directory.path().join("setup.exe");
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await?;
    let mut response = client
        .get(&asset.browser_download_url)
        .timeout(Duration::from_secs(10 * 60))
        .send()
        .await?
        .error_for_status()?;
    let mut hash = Sha256::new();
    let mut downloaded = 0_u64;
    let mut last_percent = None;
    while let Some(chunk) = response.chunk().await? {
        downloaded += chunk.len() as u64;
        ensure!(
            downloaded <= asset.size,
            "installer download exceeds its expected size"
        );
        hash.update(&chunk);
        file.write_all(&chunk).await?;
        let percent = downloaded * 100 / asset.size;
        if last_percent != Some(percent) {
            progress(UpdateProgress {
                version: info.version.clone(),
                downloaded,
                total: asset.size,
                installing: false,
            });
            last_percent = Some(percent);
        }
    }
    file.flush().await?;
    drop(file);
    verify_download(
        downloaded,
        asset.size,
        hash.finalize().as_slice(),
        &expected,
    )?;
    progress(UpdateProgress {
        version: info.version,
        downloaded,
        total: asset.size,
        installing: true,
    });
    Ok(directory)
}

#[cfg(any(feature = "desktop", test))]
fn verify_download(
    actual_size: u64,
    expected_size: u64,
    actual_hash: &[u8],
    expected_hash: &[u8],
) -> anyhow::Result<()> {
    ensure!(
        actual_size == expected_size,
        "installer download ended early"
    );
    ensure!(
        actual_hash == expected_hash,
        "installer SHA-256 verification failed"
    );
    Ok(())
}

#[cfg(feature = "desktop")]
pub(crate) fn launch_installer(directory: tempfile::TempDir) -> anyhow::Result<()> {
    if !supports_installation() {
        bail!("in-app updates require a Windows x64 NSIS or portable build");
    }
    let executable = std::env::current_exe()?;
    let install_directory = executable
        .parent()
        .context("application directory unavailable")?;
    installer_command(directory.path(), install_directory)
        .spawn()
        .context("could not launch update installer")?;
    // NSIS continues after this process exits; keep only a verified installer.
    let _ = directory.keep();
    Ok(())
}

#[cfg(any(feature = "desktop", test))]
fn installer_command(directory: &Path, install_directory: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(directory.join("setup.exe"));
    command.args(NSIS_UPDATE_ARGS);
    let mut destination = std::ffi::OsString::from("/D=");
    destination.push(install_directory);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
        // NSIS requires /D last, unquoted even when the path contains spaces.
        // Command launches the EXE directly; this text never goes through a shell.
        command.raw_arg(destination);
    }
    #[cfg(not(target_os = "windows"))]
    command.arg(destination);
    command
}

#[cfg(test)]
#[path = "updater/tests.rs"]
mod tests;
