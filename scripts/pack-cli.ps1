# Stage a CLI distribution using its actual capabilities.
[CmdletBinding()]
param(
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path,
    [switch]$RequireQrOcr
)
$ErrorActionPreference = 'Stop'
$executable = Join-Path $Root 'src-tauri/target/release/lovelycarve-cli.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Run just build-cli first.' }
$capabilities = & $executable system get | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cannot read CLI capabilities.' }
if ($RequireQrOcr -and (-not $capabilities.qrcode -or -not $capabilities.ocr)) {
    throw 'The binary does not enable QR/OCR. Run just build-cli-full before packaging.'
}
$version = (Get-Content -LiteralPath (Join-Path $Root 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version
$distRoot = Join-Path $Root 'dist-pack'
$packageName = "lovelycarve-cli-v$version-win-x64-$(Get-Date -Format yyyyMMdd-HHmmss)-$([Guid]::NewGuid().ToString('N').Substring(0, 6))"
$stage = Join-Path $distRoot $packageName
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item -LiteralPath $executable -Destination $stage
Get-ChildItem -LiteralPath (Join-Path $Root 'src-tauri/binaries') -Filter '*.dll' -File | Where-Object {
    $_.Name -notin @('libewf.dll', 'libvmdk.dll', 'libbde.dll', 'libvhdi.dll', 'librefs-13.dll', 'bzip2.dll', 'zlib.dll') -and
    ($capabilities.qrcode -or $_.Name -ne 'opencv_world4130.dll')
} | Copy-Item -Destination $stage
foreach ($feature in @('ocr', 'qrcode')) {
    if ($capabilities.$feature) {
        $resourcesTarget = Join-Path $stage 'resources'
        New-Item -ItemType Directory -Force -Path $resourcesTarget | Out-Null
        Copy-Item -LiteralPath (Join-Path $Root "src-tauri/resources/$feature") -Destination $resourcesTarget -Recurse
    }
}
# lc-plugin-host and the bundled plugins (the set tauri.plugins.conf.json ships).
$pluginBundle = Get-Content -LiteralPath (Join-Path $Root 'src-tauri/tauri.plugins.conf.json') -Raw | ConvertFrom-Json
foreach ($resource in $pluginBundle.bundle.resources.PSObject.Properties) {
    $target = Join-Path $stage $resource.Value
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
    Copy-Item -LiteralPath (Join-Path (Join-Path $Root 'src-tauri') $resource.Name) -Destination $target
}
Copy-Item -LiteralPath (Join-Path $Root 'docs/CLI.md') -Destination (Join-Path $stage 'README.md')
$skillsTarget = Join-Path $stage 'skills'
New-Item -ItemType Directory -Path $skillsTarget | Out-Null
Copy-Item -LiteralPath (Join-Path $Root 'skills/lovelycarve-cli') -Destination $skillsTarget -Recurse
Copy-Item -LiteralPath (Join-Path $Root 'LICENSE') -Destination $stage
Copy-Item -LiteralPath (Join-Path $Root 'THIRD_PARTY_NOTICES.md') -Destination $stage
$stagedCapabilities = & (Join-Path $stage 'lovelycarve-cli.exe') system get | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $stagedCapabilities.qrcode -ne $capabilities.qrcode -or $stagedCapabilities.ocr -ne $capabilities.ocr) {
    throw 'The staged CLI cannot report matching capabilities; check its runtime dependencies.'
}
$archive = Join-Path $distRoot "$packageName.zip"
Compress-Archive -LiteralPath $stage -DestinationPath $archive -CompressionLevel Optimal
Write-Output $archive
