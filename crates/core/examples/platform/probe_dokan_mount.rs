//! Mount a parsed filesystem through the Dokan read-only projection and run
//! common PowerShell queries against it.
//!
//! Usage:
//!   cargo run --example probe_dokan_mount -- <image> [mount-point] [root-node-id]

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    use std::{env, process::Command, sync::Arc, thread, time::Duration};

    use lovelycarve_core::{
        filesystem::evidence_session::EvidenceSession,
        filesystem::tree::fs_tree::{children_for_image, NodeKind},
        platform::dokan_mount,
    };
    use uuid::Uuid;

    let image_path = env::args().nth(1).ok_or_else(|| {
        anyhow::anyhow!("usage: probe_dokan_mount <image> [mount-point] [root-node-id]")
    })?;
    let mount_point = env::args().nth(2).unwrap_or_else(|| "Y:\\".to_string());
    let image = Arc::new(EvidenceSession::open(&image_path)?);
    let root_node_id = match env::args().nth(3) {
        Some(id) => id,
        None => children_for_image(&image, "")?
            .into_iter()
            .find(|node| node.kind == NodeKind::Dir)
            .map(|node| node.id)
            .ok_or_else(|| anyhow::anyhow!("image has no directory-like partition to mount"))?,
    };

    println!("mounting root node {root_node_id} at {mount_point}");
    let session = dokan_mount::start_readonly_mount(
        Uuid::new_v4().to_string(),
        "probe".to_string(),
        image,
        root_node_id,
        mount_point.clone(),
    )?;
    thread::sleep(Duration::from_millis(700));

    let mount_root = if mount_point.ends_with('\\') {
        mount_point.clone()
    } else {
        format!("{mount_point}\\")
    };
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
Write-Host '## Get-Item'
Get-Item -LiteralPath '{mount_point}' | Format-List FullName,Mode,Attributes,Length
Write-Host '## Get-PSDrive'
Get-PSDrive -Name '{drive}' -PSProvider FileSystem | Format-List Name,Root,Used,Free,DisplayRoot
Write-Host '## Get-Volume'
try {{
    Get-Volume -DriveLetter '{drive}' | Format-List DriveLetter,FileSystemLabel,FileSystemType,Size,SizeRemaining,DriveType
}} catch {{
    Write-Host "Get-Volume failed: $($_.Exception.Message)"
}}
Write-Host '## Win32_LogicalDisk'
Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='{drive}:'" | Format-List DeviceID,DriveType,FileSystem,VolumeName,Size,FreeSpace
Write-Host '## Get-Acl'
try {{
    (Get-Acl -LiteralPath '{mount_point}').Access | Select-Object -First 8 IdentityReference,FileSystemRights,AccessControlType | Format-Table -AutoSize
}} catch {{
    Write-Host "Get-Acl failed: $($_.Exception.Message.Split([Environment]::NewLine)[0])"
}}
Write-Host '## Get-ChildItem'
$items = Get-ChildItem -LiteralPath '{mount_point}' -Force | Select-Object -First 20
$items | Select-Object Mode,Length,Name | Format-Table -AutoSize
Write-Host '## Test-Path'
Write-Host ("root exists: " + (Test-Path -LiteralPath '{mount_point}'))
$firstFile = $items | Where-Object {{ -not $_.PSIsContainer }} | Select-Object -First 1
if ($null -ne $firstFile) {{
    Write-Host '## Get-Content (first 32 bytes hex)'
    $bytes = [System.IO.File]::ReadAllBytes($firstFile.FullName) | Select-Object -First 32
    if ($bytes.Count -gt 0) {{
        ($bytes | ForEach-Object {{ '{{0:X2}}' -f $_ }}) -join ' ' | Write-Host
    }} else {{
        Write-Host '(empty file)'
    }}
    Write-Host '## File Get-Acl'
    try {{
        (Get-Acl -LiteralPath $firstFile.FullName).Access | Select-Object -First 4 IdentityReference,FileSystemRights | Format-Table -AutoSize
    }} catch {{
        Write-Host "File Get-Acl failed: $($_.Exception.Message.Split([Environment]::NewLine)[0])"
    }}
}}
Write-Host '## Write attempt (should fail)'
try {{
    Set-Content -LiteralPath ('{mount_point}' + 'should_fail.txt') -Value 'hello' -ErrorAction Stop
    Write-Host 'WARNING: write unexpectedly succeeded'
}} catch {{
    Write-Host "Write blocked as expected: $($_.Exception.Message.Split([Environment]::NewLine)[0])"
}}
Write-Host '## fsutil volume'
try {{
    & fsutil.exe volume diskfree '{drive}:' 2>&1 | Select-Object -First 6
    if ($LASTEXITCODE -ne 0) {{
        Write-Host "fsutil exited with code $LASTEXITCODE"
    }}
}} catch {{
    Write-Host "fsutil failed: $($_.Exception.Message.Split([Environment]::NewLine)[0])"
}}
"#,
        mount_point = mount_root,
        drive = mount_point
            .chars()
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty mount point"))?
    );

    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()?;
    println!("powershell status: {}", output.status);
    println!("--- stdout ---");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    println!("--- stderr ---");
    println!("{}", String::from_utf8_lossy(&output.stderr));
    session.stop()?;
    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!("PowerShell probe failed")
    }
}

#[cfg(not(target_os = "windows"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("probe_dokan_mount is only available on Windows")
}
