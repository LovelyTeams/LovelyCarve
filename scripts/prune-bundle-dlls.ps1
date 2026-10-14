# Tauri's MSI bundler adds every *.dll in the target directory to the installer,
# so DLLs left in src-tauri\target\release by QR/OCR builds (OpenCV, ONNX Runtime
# execution providers) or by the legacy oracle libraries would ship in a default
# MSI. Remove the root-level DLLs the bundle configuration does not declare
# before `tauri build` runs; symbolic links are removed, never their targets.
#
# Driven by `just build-release`, but runnable directly:
#   pwsh -NoProfile -File scripts\prune-bundle-dlls.ps1
[CmdletBinding()]
param(
    # Repo root (defaults to the parent of this script's dir).
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path,
    # Bundle configurations under src-tauri whose resources are kept.
    [string[]]$Config = @('tauri.conf.json', 'tauri.plugins.generated.conf.json')
)

$ErrorActionPreference = 'Stop'

$tauriDir = Join-Path $Root 'src-tauri'
$targetDir = Join-Path $tauriDir 'target\release'
if (-not (Test-Path -LiteralPath $targetDir)) { return }

$declared = @{}
foreach ($name in $Config) {
    $resources = (Get-Content -LiteralPath (Join-Path $tauriDir $name) -Raw | ConvertFrom-Json).bundle.resources
    if ($resources) {
        foreach ($resource in $resources.PSObject.Properties) {
            $declared[$resource.Value.ToLowerInvariant()] = $true
        }
    }
}

Get-ChildItem -LiteralPath $targetDir -Filter '*.dll' -File |
    Where-Object { -not $declared.ContainsKey($_.Name.ToLowerInvariant()) } |
    ForEach-Object {
        Write-Host "==> removing $($_.Name) from target\release (not a bundle resource)"
        Remove-Item -LiteralPath $_.FullName -Force
    }
