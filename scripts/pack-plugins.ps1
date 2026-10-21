# Package every workspace plugin as an installable .lcpkg: a ZIP of its package
# directory (plugin.toml, library, schema.json, locales/) plus its guide,
# license and build provenance. Generated output stays in dist-pack.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$workspace = Join-Path $repositoryRoot 'plugins'
# Package directory => the guide shipped as its README.md.
$packages = [ordered]@{
    linuxinfo = Join-Path $repositoryRoot 'docs/LINUXINFO.md'
}
$commit = (& git -C $repositoryRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve source commit.' }
$sourceStatus = & git -C $repositoryRoot status --porcelain --untracked-files=normal
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve source worktree status.' }
$outputRoot = Join-Path $repositoryRoot 'dist-pack'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'

foreach ($package in $packages.Keys) {
    $directory = Join-Path $workspace $package
    $manifestPath = Join-Path $directory 'plugin.toml'
    $manifest = Get-Content -LiteralPath $manifestPath -Raw
    # Top-level keys come before the first [[analyzer]] table.
    $header = ($manifest -split '(?m)^\s*\[')[0]
    $field = {
        param($name)
        $match = [regex]::Match($header, "(?m)^\s*$name\s*=\s*""([^""]+)""")
        if (-not $match.Success) { throw "$manifestPath has no top-level $name" }
        $match.Groups[1].Value
    }
    $id = & $field 'id'
    $version = & $field 'version'
    $library = & $field 'library'
    $files = @(
        $manifestPath,
        (Join-Path $directory "$library.dll"),
        (Join-Path $directory 'schema.json'),
        (Join-Path $repositoryRoot 'LICENSE'),
        (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md')
    )
    foreach ($source in $files + $packages[$package]) {
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing build output: $source. Run just build-plugins --release." }
    }
    $name = "$id-$version-windows-x64"
    $stage = Join-Path $outputRoot "$name-$stamp"
    $archive = Join-Path $outputRoot "$name-$stamp.lcpkg"
    if ((Test-Path -LiteralPath $stage) -or (Test-Path -LiteralPath $archive)) { throw "Output already exists: $archive" }
    New-Item -ItemType Directory -Path $stage | Out-Null
    foreach ($file in $files) {
        Copy-Item -LiteralPath $file -Destination $stage
    }
    $locales = Join-Path $directory 'locales'
    if (Test-Path -LiteralPath $locales -PathType Container) {
        Copy-Item -LiteralPath $locales -Destination $stage -Recurse
    }
    Copy-Item -LiteralPath $packages[$package] -Destination (Join-Path $stage 'README.md')
    @{
        plugin = $id
        version = $version
        protocol = 2
        sourceCommit = $commit
        sourceDirty = @($sourceStatus).Count -gt 0
        builtAt = [DateTimeOffset]::Now.ToString('o')
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $stage 'build.json') -Encoding utf8
    $hashes = Get-ChildItem -LiteralPath $stage -Recurse -File | Sort-Object FullName | ForEach-Object {
        $relative = [IO.Path]::GetRelativePath($stage, $_.FullName).Replace('\', '/')
        (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + $relative
    }
    $hashes | Set-Content -LiteralPath (Join-Path $stage 'SHA256SUMS.txt') -Encoding utf8
    # plugin.toml at the archive root; entry names use forward slashes.
    [IO.Compression.ZipFile]::CreateFromDirectory($stage, $archive, [IO.Compression.CompressionLevel]::Optimal, $false)
    Remove-Item -LiteralPath $stage -Recurse
    Write-Output $archive
}
