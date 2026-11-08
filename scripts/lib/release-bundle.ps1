# Shared staging steps for the portable desktop package (pack.ps1) and the CLI
# package (pack-cli.ps1). Dot-source this file; it defines functions only.

# The workspace version from [workspace.package] in the root Cargo.toml.
function Get-WorkspaceVersion([string]$Root) {
    $manifest = Get-Content -LiteralPath (Join-Path $Root 'Cargo.toml') -Raw
    $section = [regex]::Match($manifest, '(?ms)^\[workspace\.package\]\s*$(.*?)(?=^\[|\z)')
    if (-not $section.Success) { throw 'Cargo.toml has no [workspace.package] table' }
    $version = [regex]::Match($section.Groups[1].Value, '(?m)^version\s*=\s*"([^"]+)"')
    if (-not $version.Success) { throw 'Cargo.toml [workspace.package] has no version' }
    $version.Groups[1].Value
}

# The load-time libraries from binaries/ placed flat beside an executable.
# Legacy oracle DLLs never ship; OpenCV only with QR support.
function Copy-RuntimeLibraries([string]$Root, [string]$Stage, [switch]$OpenCv) {
    Get-ChildItem -LiteralPath (Join-Path $Root 'binaries') -Filter '*.dll' -File | Where-Object {
        $_.Name -notin @('libewf.dll', 'libvmdk.dll', 'libbde.dll', 'libvhdi.dll', 'librefs-13.dll', 'bzip2.dll', 'zlib.dll') -and
        ($OpenCv -or $_.Name -ne 'opencv_world4130.dll')
    } | Copy-Item -Destination $Stage -Force
}

# lc-plugin-host beside the executable and every bundled plugin package under
# plugins/<name>/: plugin.toml, the library, schema.json and locales/. Run
# `just build-plugins --release` first.
function Copy-PluginBundle([string]$Root, [string]$Stage) {
    $workspace = Join-Path $Root 'plugins'
    $pluginHost = Join-Path $workspace 'target/release/lc-plugin-host.exe'
    if (-not (Test-Path -LiteralPath $pluginHost -PathType Leaf)) { throw "Missing $pluginHost. Run just build-plugins --release." }
    Copy-Item -LiteralPath $pluginHost -Destination $Stage -Force
    foreach ($manifest in Get-ChildItem -LiteralPath $workspace -Directory | ForEach-Object { Join-Path $_.FullName 'plugin.toml' } | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }) {
        $directory = Split-Path -Parent $manifest
        $name = Split-Path -Leaf $directory
        # Top-level keys come before the first table.
        $header = ((Get-Content -LiteralPath $manifest -Raw) -split '(?m)^\s*\[')[0]
        $library = [regex]::Match($header, '(?m)^\s*library\s*=\s*"([^"]+)"')
        if (-not $library.Success) { throw "$manifest has no top-level library" }
        $target = Join-Path (Join-Path $Stage 'plugins') $name
        New-Item -ItemType Directory -Force -Path $target | Out-Null
        foreach ($file in @('plugin.toml', "$($library.Groups[1].Value).dll", 'schema.json')) {
            $source = Join-Path $directory $file
            if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing build output: $source. Run just build-plugins --release." }
            Copy-Item -LiteralPath $source -Destination $target -Force
        }
        $locales = Join-Path $directory 'locales'
        if (Test-Path -LiteralPath $locales -PathType Container) {
            Copy-Item -LiteralPath $locales -Destination $target -Recurse -Force
        }
    }
}
