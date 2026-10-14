# Package discovered plugins and their isolation host; generated output stays in dist-pack.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$tauriDirectory = Join-Path $repositoryRoot 'src-tauri'
$bundle = Get-Content -LiteralPath (Join-Path $tauriDirectory 'tauri.plugins.generated.conf.json') -Raw | ConvertFrom-Json
$resources = @($bundle.bundle.resources.PSObject.Properties)
$plugins = @($resources | Where-Object { $_.Value -match '^plugins/[^/]+/plugin\.toml$' } | ForEach-Object {
    $_.Value.Split('/')[1]
})
$sources = @($resources | ForEach-Object { Join-Path $tauriDirectory $_.Name })
$sources += @((Join-Path $repositoryRoot 'LICENSE'), (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md'), (Join-Path $repositoryRoot 'docs/PLUGIN_MANIFEST.md'), (Join-Path $repositoryRoot 'docs/PLUGIN_PROTOCOL.md'))
foreach ($source in $sources) {
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing build output: $source. Run just build-plugins --release." }
}
$commit = (& git -C $repositoryRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve source commit.' }
$sourceStatus = & git -C $repositoryRoot status --porcelain --untracked-files=normal
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve source worktree status.' }
$name = 'lovelycarve-plugins-windows-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
$outputRoot = Join-Path $repositoryRoot 'dist-pack'
$stage = Join-Path $outputRoot $name
$archive = "$stage.zip"
if ((Test-Path -LiteralPath $stage) -or (Test-Path -LiteralPath $archive)) { throw "Output already exists: $stage" }
New-Item -ItemType Directory -Path (Join-Path $stage 'plugins') | Out-Null
foreach ($resource in $resources) {
    $destination = Join-Path $stage $resource.Value
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
    Copy-Item -LiteralPath (Join-Path $tauriDirectory $resource.Name) -Destination $destination
}
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'LICENSE') -Destination $stage
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md') -Destination $stage
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'docs/PLUGIN_MANIFEST.md') -Destination (Join-Path $stage 'README.md')
Copy-Item -LiteralPath (Join-Path $repositoryRoot 'docs/PLUGIN_PROTOCOL.md') -Destination $stage
@{
    sourceCommit = $commit
    sourceDirty = @($sourceStatus).Count -gt 0
    builtAt = [DateTimeOffset]::Now.ToString('o')
    plugins = $plugins
    protocol = 1
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $stage 'build.json') -Encoding utf8
$hashes = Get-ChildItem -LiteralPath $stage -Recurse -File | Sort-Object FullName | ForEach-Object {
    $relative = [IO.Path]::GetRelativePath($stage, $_.FullName).Replace('\', '/')
    (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + $relative
}
$hashes | Set-Content -LiteralPath (Join-Path $stage 'SHA256SUMS.txt') -Encoding utf8
$contents = Get-ChildItem -LiteralPath $stage | ForEach-Object FullName
Compress-Archive -LiteralPath $contents -DestinationPath $archive -CompressionLevel Optimal
Write-Output $archive
