# Stage a CLI distribution using its actual capabilities.
[CmdletBinding()]
param(
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path,
    [switch]$RequireQrOcr,
    [switch]$StageSkill
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib/release-bundle.ps1')
$executable = Join-Path $Root 'target/release/lovelycarve-cli.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Run just build-cli first.' }
$capabilities = & $executable system get | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cannot read CLI capabilities.' }
if ($RequireQrOcr -and (-not $capabilities.qrcode -or -not $capabilities.ocr)) {
    throw 'The binary does not enable QR/OCR. Run just build-cli-full before packaging.'
}
$version = Get-WorkspaceVersion -Root $Root
$distRoot = Join-Path $Root 'dist-pack'
$packageName = "lovelycarve-cli-v$version-win-x64-$(Get-Date -Format yyyyMMdd-HHmmss)-$([Guid]::NewGuid().ToString('N').Substring(0, 6))"
$stage = Join-Path $distRoot $packageName
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item -LiteralPath $executable -Destination $stage
Copy-RuntimeLibraries -Root $Root -Stage $stage -OpenCv:([bool]$capabilities.qrcode)
foreach ($feature in @('ocr', 'qrcode')) {
    if ($capabilities.$feature) {
        $resourcesTarget = Join-Path $stage 'resources'
        New-Item -ItemType Directory -Force -Path $resourcesTarget | Out-Null
        Copy-Item -LiteralPath (Join-Path $Root "resources/$feature") -Destination $resourcesTarget -Recurse
    }
}
# lc-plugin-host and the bundled plugins.
Copy-PluginBundle -Root $Root -Stage $stage
Copy-Item -LiteralPath (Join-Path $Root 'docs/CLI.md') -Destination (Join-Path $stage 'README.md')
$skillsTarget = Join-Path $stage 'skills'
New-Item -ItemType Directory -Path $skillsTarget | Out-Null
$skillSource = Join-Path $Root 'skills/lovelycarve-cli'
$stagedSkill = Join-Path $skillsTarget 'lovelycarve-cli'
New-Item -ItemType Directory -Path $stagedSkill | Out-Null
# The ordinary CLI package already carries the runtime at its root. Do not
# duplicate a locally staged skill bin/ (or its previous runtime) inside it.
Get-ChildItem -LiteralPath $skillSource -Force | Where-Object Name -ne 'bin' |
    Copy-Item -Destination $stagedSkill -Recurse
Copy-Item -LiteralPath (Join-Path $Root 'LICENSE') -Destination $stage
Copy-Item -LiteralPath (Join-Path $Root 'THIRD_PARTY_NOTICES.md') -Destination $stage
$stagedCapabilities = & (Join-Path $stage 'lovelycarve-cli.exe') system get | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $stagedCapabilities.qrcode -ne $capabilities.qrcode -or $stagedCapabilities.ocr -ne $capabilities.ocr) {
    throw 'The staged CLI cannot report matching capabilities; check its runtime dependencies.'
}
if ($StageSkill) {
    $runtimeStage = Join-Path $stage 'skill-bin'
    New-Item -ItemType Directory -Path $runtimeStage | Out-Null
    Get-ChildItem -LiteralPath $stage -Force | Where-Object Name -notin @('skills', 'skill-bin') |
        Copy-Item -Destination $runtimeStage -Recurse
    $skillBin = [IO.Path]::GetFullPath((Join-Path $skillSource 'bin'))
    $ignoreFile = Join-Path $skillBin '.gitignore'
    if (Test-Path -LiteralPath $ignoreFile) {
        Copy-Item -LiteralPath $ignoreFile -Destination $runtimeStage
    }
    $runtimeCapabilities = & (Join-Path $runtimeStage 'lovelycarve-cli.exe') system get | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $runtimeCapabilities.qrcode -ne $capabilities.qrcode -or $runtimeCapabilities.ocr -ne $capabilities.ocr) {
        throw 'The skill runtime cannot report matching capabilities.'
    }
    # Validate every directory move against the requested repository root.
    # Keep the previous bin intact in dist-pack so refreshes are recoverable.
    $rootPrefix = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $previousBin = [IO.Path]::GetFullPath((Join-Path $stage 'previous-skill-bin'))
    foreach ($movePath in @($skillBin, $runtimeStage, $previousBin)) {
        if (-not ([IO.Path]::GetFullPath($movePath)).StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Skill staging path is outside the repository: $movePath"
        }
    }
    $hadPreviousBin = Test-Path -LiteralPath $skillBin
    if ($hadPreviousBin) {
        Move-Item -LiteralPath $skillBin -Destination $previousBin
    }
    try {
        Move-Item -LiteralPath $runtimeStage -Destination $skillBin
    } catch {
        if ($hadPreviousBin) {
            Move-Item -LiteralPath $previousBin -Destination $skillBin
        }
        throw
    }
    Write-Output $skillBin
    return
}
$archive = Join-Path $distRoot "$packageName.zip"
Compress-Archive -LiteralPath $stage -DestinationPath $archive -CompressionLevel Optimal
Write-Output $archive
