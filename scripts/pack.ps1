# Portable packaging for LovelyCarve.
#
# Stages the release exe + the runtime DLLs (the same set bundle.resources ships)
# into a `LovelyCarve/` folder, UPX-compresses the exe AND the runtime DLLs with
# -9, then bundles the whole
# folder into one .7z under dist-pack/. The exe is always copied fresh before UPX
# — never pack the linker output in place (a re-run would then feed UPX an
# already-packed exe and fail); the DLLs are likewise copied fresh each run.
#
# Driven by the `just pack` (build first) / `just repack` (skip build) recipes,
# but runnable directly:  pwsh -NoProfile -File scripts\pack.ps1
[CmdletBinding()]
param(
    # Repo root (defaults to the parent of this script's dir).
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path,
    # Tool executables — override if not on PATH.
    [string]$Upx = 'upx',
    [string]$SevenZip = '7z',
    # Match the build flavor; enabled by pack-full / repack-full.
    [switch]$QrOcr
)

$ErrorActionPreference = 'Stop'
# Only our explicit $LASTEXITCODE checks decide native-command failure (avoid PS
# 7.3+ turning benign upx/7z stderr or exit codes into terminating errors).
$PSNativeCommandUseErrorActionPreference = $false

$exe = Join-Path $Root 'src-tauri\target\release\LovelyCarve.exe'
$binaries = Join-Path $Root 'src-tauri\binaries'
$resources = Join-Path $Root 'src-tauri\resources'
$distRoot = Join-Path $Root 'dist-pack'
$stage = Join-Path $distRoot 'LovelyCarve'

if (-not (Test-Path $exe)) {
    throw "release exe not found: $exe`nRun ``bun run tauri build`` first (or use ``just pack``)."
}

# Fresh staging dir.
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

# Copy a fresh, unpacked exe, then UPX it in the staging dir.
$stagedExe = Join-Path $stage 'LovelyCarve.exe'
Copy-Item $exe $stagedExe -Force

Write-Host '==> UPX -9 LovelyCarve.exe'
& $Upx -9 $stagedExe
if ($LASTEXITCODE -ne 0) { throw "upx failed (exit $LASTEXITCODE)" }

# Bundle the runtime DLLs flat next to the exe (same set as bundle.resources).
Get-ChildItem -LiteralPath $binaries -Filter '*.dll' -File | Where-Object {
    $_.Name -notin @('libewf.dll', 'libvmdk.dll', 'libbde.dll', 'libvhdi.dll', 'librefs-13.dll', 'bzip2.dll', 'zlib.dll') -and
    ($QrOcr -or $_.Name -ne 'opencv_world4130.dll')
} | Copy-Item -Destination $stage -Force
Write-Host "==> staged $((Get-ChildItem (Join-Path $stage '*.dll')).Count) runtime DLLs next to the exe"
# The app-local Visual C++ runtime (vcruntime140*, msvcp140*) stays exactly as
# Microsoft signed it.
$dlls = Get-ChildItem (Join-Path $stage '*.dll') | Where-Object { $_.Name -notlike 'vcruntime*.dll' -and $_.Name -notlike 'msvcp*.dll' }

# UPX -9 the staged DLLs too (the freshly-copied ones, never the originals under
# src-tauri\binaries). One upx invocation packs them all in a single table; the
# big win is opencv_world4130.dll (~31 MB). Unlike the exe, DLL packing is
# best-effort: upx exits non-zero if ANY file is un-packable (CantPack) but still
# packs the rest and leaves the un-packable ones byte-for-byte intact, so we do
# NOT throw — the package is valid either way (uncompressed DLLs still load).
$dllBefore = ($dlls | Measure-Object Length -Sum).Sum
Write-Host "==> UPX -9 $($dlls.Count) runtime DLLs (un-packable ones shipped uncompressed)"
& $Upx -9 @($dlls.FullName)
$upxDllExit = $LASTEXITCODE
$dllAfter = ($dlls | ForEach-Object { (Get-Item -LiteralPath $_.FullName).Length } | Measure-Object -Sum).Sum
if ($upxDllExit -ne 0) {
    Write-Host "    note: upx exit $upxDllExit — one or more DLLs left uncompressed (see table above)"
}
Write-Host ("==> runtime DLLs: {0:N1} MB -> {1:N1} MB" -f ($dllBefore / 1MB), ($dllAfter / 1MB))

# Externalized model resources (resources/ocr + resources/qrcode) — copied as a
# `resources/` folder next to the exe; the app resolves them via resource_path().
if ($QrOcr) {
    Copy-Item $resources (Join-Path $stage 'resources') -Recurse -Force
    $resCount = (Get-ChildItem (Join-Path $stage 'resources') -Recurse -File).Count
    Write-Host "==> staged resources/ ($resCount externalized model files)"
}

# lc-plugin-host and the bundled plugins: the same set tauri.plugins.generated.conf.json
# adds to the installers (built by `just build-release`).
$pluginBundle = Get-Content -LiteralPath (Join-Path $Root 'src-tauri\tauri.plugins.generated.conf.json') -Raw | ConvertFrom-Json
foreach ($resource in $pluginBundle.bundle.resources.PSObject.Properties) {
    $target = Join-Path $stage $resource.Value
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
    Copy-Item -LiteralPath (Join-Path (Join-Path $Root 'src-tauri') $resource.Name) -Destination $target -Force
}
Write-Host "==> staged lc-plugin-host and plugins/"

Copy-Item -LiteralPath (Join-Path $Root 'LICENSE') -Destination $stage -Force
Copy-Item -LiteralPath (Join-Path $Root 'THIRD_PARTY_NOTICES.md') -Destination $stage -Force

# Archive name carries the app version from tauri.conf.json.
$version = (Get-Content (Join-Path $Root 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json).version
$archive = Join-Path $distRoot "LovelyCarve-v$version-win-x64.7z"
if (Test-Path $archive) { Remove-Item $archive -Force }

Write-Host "==> 7z packing -> $archive"
Push-Location $distRoot
try {
    & $SevenZip a -t7z -mx=9 -y $archive 'LovelyCarve' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "7z failed (exit $LASTEXITCODE)" }
}
finally {
    Pop-Location
}

$size = '{0:N1} MB' -f ((Get-Item $archive).Length / 1MB)
Write-Host "==> done: $archive ($size)"
