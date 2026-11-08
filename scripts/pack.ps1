# Portable packaging for the LovelyCarve desktop app.
#
# Stages the release exe, the runtime DLLs, lc-plugin-host and the bundled
# plugins into a `LovelyCarve/` folder, UPX-compresses the exe AND the
# runtime DLLs with -9, then bundles the whole folder into one .7z under
# dist-pack/. The exe is always copied fresh before UPX — never pack the
# linker output in place (a re-run would then feed UPX an already-packed exe
# and fail); the DLLs are likewise copied fresh each run.
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
    # Skip UPX (CI runners have 7-Zip but no UPX); the archive is larger.
    [switch]$NoUpx
)

$ErrorActionPreference = 'Stop'
# Only our explicit $LASTEXITCODE checks decide native-command failure (avoid PS
# 7.3+ turning benign upx/7z stderr or exit codes into terminating errors).
$PSNativeCommandUseErrorActionPreference = $false
. (Join-Path $PSScriptRoot 'lib/release-bundle.ps1')

$exe = Join-Path $Root 'target\release\LovelyCarve.exe'
$distRoot = Join-Path $Root 'dist-pack'
$stage = Join-Path $distRoot 'LovelyCarve'

if (-not (Test-Path $exe)) {
    throw "release exe not found: $exe`nRun ``just build-release`` first (or use ``just pack``)."
}

# Fresh staging dir.
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

# Copy a fresh, unpacked exe, then UPX it in the staging dir.
$stagedExe = Join-Path $stage 'LovelyCarve.exe'
Copy-Item $exe $stagedExe -Force

if (-not $NoUpx) {
    Write-Host '==> UPX -9 LovelyCarve.exe'
    & $Upx -9 $stagedExe
    if ($LASTEXITCODE -ne 0) { throw "upx failed (exit $LASTEXITCODE)" }
}

# Bundle the runtime DLLs flat next to the exe.
Copy-RuntimeLibraries -Root $Root -Stage $stage
Write-Host "==> staged $((Get-ChildItem (Join-Path $stage '*.dll')).Count) runtime DLLs next to the exe"
# The app-local Visual C++ runtime (vcruntime140*, msvcp140*) stays exactly as
# Microsoft signed it.
$dlls = Get-ChildItem (Join-Path $stage '*.dll') | Where-Object { $_.Name -notlike 'vcruntime*.dll' -and $_.Name -notlike 'msvcp*.dll' }

# UPX -9 the staged DLLs too (the freshly-copied ones, never the originals under
# binaries\). One upx invocation packs them all in a single table. Unlike the
# exe, DLL packing is best-effort: upx exits non-zero if ANY file is
# un-packable (CantPack) but still packs the rest and leaves the un-packable
# ones byte-for-byte intact, so we do NOT throw — the package is valid either
# way (uncompressed DLLs still load).
if (-not $NoUpx) {
    $dllBefore = ($dlls | Measure-Object Length -Sum).Sum
    Write-Host "==> UPX -9 $($dlls.Count) runtime DLLs (un-packable ones shipped uncompressed)"
    & $Upx -9 @($dlls.FullName)
    $upxDllExit = $LASTEXITCODE
    $dllAfter = ($dlls | ForEach-Object { (Get-Item -LiteralPath $_.FullName).Length } | Measure-Object -Sum).Sum
    if ($upxDllExit -ne 0) {
        Write-Host "    note: upx exit $upxDllExit — one or more DLLs left uncompressed (see table above)"
    }
    Write-Host ("==> runtime DLLs: {0:N1} MB -> {1:N1} MB" -f ($dllBefore / 1MB), ($dllAfter / 1MB))
}

# lc-plugin-host and the bundled plugins (built by `just build-release`).
Copy-PluginBundle -Root $Root -Stage $stage
Write-Host "==> staged lc-plugin-host and plugins/"

Copy-Item -LiteralPath (Join-Path $Root 'LICENSE') -Destination $stage -Force
Copy-Item -LiteralPath (Join-Path $Root 'THIRD_PARTY_NOTICES.md') -Destination $stage -Force

# Archive name carries the workspace version from Cargo.toml.
$version = Get-WorkspaceVersion -Root $Root
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
