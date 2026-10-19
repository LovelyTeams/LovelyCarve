//! Verify maintainer signatures before handing NSIS to normal application exit.
use std::time::Duration;

use anyhow::{ensure, Context};

use super::{GithubRelease, UpdateProgress, MAX_INSTALLER_BYTES};

pub(crate) async fn download_signed(
    app: &crate::host::AppHandle,
    selected_version: &str,
    release: &GithubRelease,
    progress: impl Fn(UpdateProgress),
) -> anyhow::Result<tempfile::TempDir> {
    ensure!(
        super::supports_installation(),
        "in-app updates require a Windows x64 NSIS or portable build"
    );
    // Portable builds consume the same verified installer as NSIS builds.
    let candidate = app
        .updater_builder()?
        .target("windows-x86_64-nsis")
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()?
        .check()
        .await?
        .context("signed update is unavailable; check for updates again")?;
    ensure!(
        candidate.version == selected_version,
        "signed release changed; check for updates again"
    );
    let name = format!("LovelyCarve_{selected_version}_x64-setup.exe");
    let asset = release
        .assets
        .iter()
        .find(|asset| release.is_asset(asset, &name))
        .context("the signed release has no Windows x64 NSIS installer")?;
    ensure!(
        candidate.download_url.as_str() == asset.browser_download_url,
        "signed installer must belong to the selected GitHub release"
    );
    ensure!(
        asset.size > 0 && asset.size <= MAX_INSTALLER_BYTES,
        "invalid signed installer size"
    );
    let mut downloaded = 0_u64;
    let mut last_percent = None;
    let mut response = super::client()?
        .get(&asset.browser_download_url)
        .timeout(Duration::from_secs(10 * 60))
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        downloaded += chunk.len() as u64;
        ensure!(
            downloaded <= asset.size,
            "signed installer exceeds its expected size"
        );
        bytes.extend_from_slice(&chunk);
        let percent = downloaded.saturating_mul(100) / asset.size;
        if last_percent != Some(percent) {
            progress(UpdateProgress {
                version: selected_version.into(),
                downloaded,
                total: asset.size,
                installing: false,
            });
            last_percent = Some(percent);
        }
    }
    ensure!(
        bytes.len() as u64 == asset.size,
        "signed installer download size mismatch"
    );
    // verify() checks both the maintainer signature and its signed version.
    candidate.verify(&bytes)?;
    let directory = tempfile::Builder::new()
        .prefix("LovelyCarve-update-")
        .tempdir()?;
    tokio::fs::write(directory.path().join("setup.exe"), bytes).await?;
    progress(UpdateProgress {
        version: selected_version.into(),
        downloaded,
        total: asset.size,
        installing: true,
    });
    Ok(directory)
}
