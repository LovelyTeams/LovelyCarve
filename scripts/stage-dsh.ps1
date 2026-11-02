# Refresh the DSH bundle from maintained sources and the staged CLI runtime.
[CmdletBinding()]
param(
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
)
$ErrorActionPreference = 'Stop'
$runtime = Join-Path $Root 'skills/lovelycarve-cli/bin'
$executable = Join-Path $runtime 'lovelycarve-cli.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Run just stage-cli-skill first.' }
$source = Join-Path $Root 'integrations/dsh-lovelycarve'
$distRoot = Join-Path $Root 'dist-pack'
$stage = Join-Path $distRoot "dsh-stage-$([Guid]::NewGuid().ToString('N'))"
$target = Join-Path $distRoot 'dsh-lovelycarve'
$backup = Join-Path $distRoot "dsh-previous-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Get-ChildItem -LiteralPath $source | Copy-Item -Destination $stage -Recurse
$skillTarget = Join-Path $stage 'skills/lovelycarve-cli'
New-Item -ItemType Directory -Force -Path $skillTarget | Out-Null
Get-ChildItem -LiteralPath (Join-Path $Root 'skills/lovelycarve-cli') -Force |
    Where-Object Name -ne 'bin' | Copy-Item -Destination $skillTarget -Recurse
Copy-Item -LiteralPath $runtime -Destination (Join-Path $skillTarget 'bin') -Recurse
foreach ($name in @('LICENSE', 'THIRD_PARTY_NOTICES.md')) {
    Copy-Item -LiteralPath (Join-Path $Root $name) -Destination $stage
}
# All moves stay inside the repository's distribution directory. Preserve the
# previous complete bundle, including its runtime, for a recoverable refresh.
$distPrefix = [IO.Path]::GetFullPath($distRoot).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
foreach ($movePath in @($stage, $target, $backup)) {
    if (-not [IO.Path]::GetFullPath($movePath).StartsWith($distPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "DSH staging path is outside dist-pack: $movePath"
    }
}
$hadPrevious = Test-Path -LiteralPath $target
if ($hadPrevious) { Move-Item -LiteralPath $target -Destination $backup }
try {
    Move-Item -LiteralPath $stage -Destination $target
} catch {
    if ($hadPrevious) { Move-Item -LiteralPath $backup -Destination $target }
    throw
}
Write-Output $target
