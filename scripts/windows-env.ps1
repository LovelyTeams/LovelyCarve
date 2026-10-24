# Dot-source from just recipes so the environment stays in the command's shell.
[CmdletBinding()]
param(
    [string]$OpenSSLDir = 'C:\Program Files\OpenSSL-Win64',
    [string]$OpenSSLLib = '',
    [string]$PerlBin = '',
    [switch]$VisualStudio,
    [string]$VsPath = '',
    [switch]$OpenCV,
    [string]$OpenCVDir = '',
    [string]$OpenCVLib = 'opencv_world4130',
    [string]$OpenCVMsvc = 'vc18'
)
$ErrorActionPreference = 'Stop'

if ($VisualStudio) {
    if (-not $VsPath) {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        if (-not (Test-Path -LiteralPath $vswhere)) {
            throw 'Cannot find Visual Studio Installer. Set VS_PATH to your C++ tools installation.'
        }
        $VsPath = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($LASTEXITCODE -ne 0 -or -not $VsPath) {
            throw 'Cannot find Visual Studio C++ x64 tools. Install them or set VS_PATH.'
        }
    }
    Import-Module (Join-Path $VsPath 'Common7/Tools/Microsoft.VisualStudio.DevShell.dll')
    Enter-VsDevShell -VsInstallPath $VsPath -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
    if (-not $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER) {
        $lldLinker = Join-Path $VsPath 'VC/Tools/Llvm/x64/bin/lld-link.exe'
        if (-not (Test-Path -LiteralPath $lldLinker -PathType Leaf)) {
            throw 'Cannot find lld-link.exe. Install the Visual Studio C++ Clang tools for Windows, or set CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER.'
        }
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = (Get-Item -LiteralPath $lldLinker).FullName
    }
    # Keep caller-supplied flags intact, and never pass LLD options to another linker.
    $linkerName = Split-Path $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER -Leaf
    if ($linkerName -in @('lld-link', 'lld-link.exe') -and -not $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS) {
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = '-C link-arg=/threads:1'
    }
    if ($OpenCV) {
        $llvmBin = Join-Path $VsPath 'VC/Tools/Llvm/x64/bin'
        $env:PATH = $llvmBin + ';' + $env:PATH
        $env:LIBCLANG_PATH = $llvmBin
    }
}

if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = '1' }
if (-not $env:CARGO_PROFILE_RELEASE_LTO) { $env:CARGO_PROFILE_RELEASE_LTO = 'thin' }

if (-not $OpenSSLLib) { $OpenSSLLib = Join-Path $OpenSSLDir 'lib/VC/x64/MT' }
$env:OPENSSL_NO_VENDOR = '1'
$env:OPENSSL_STATIC = '1'
$env:OPENSSL_DIR = $OpenSSLDir
$env:OPENSSL_LIB_DIR = $OpenSSLLib
$env:OPENSSL_INCLUDE_DIR = Join-Path $OpenSSLDir 'include'
if ($PerlBin) { $env:PATH = $PerlBin + ';' + $env:PATH }

if ($OpenCV) {
    if (-not $OpenCVDir) { throw 'Set OPENCV_DIR to your contrib-enabled OpenCV installation.' }
    $env:OPENCV_LINK_LIBS = $OpenCVLib
    $env:OPENCV_LINK_PATHS = Join-Path $OpenCVDir "x64/$OpenCVMsvc/lib"
    $env:OPENCV_INCLUDE_PATHS = Join-Path $OpenCVDir 'include'
    $env:PATH = (Join-Path $OpenCVDir "x64/$OpenCVMsvc/bin") + ';' + $env:PATH
}
