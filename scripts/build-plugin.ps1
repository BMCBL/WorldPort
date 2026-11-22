# Builds the WorldPort plugin package and its native module.
#
# The plugin bakes the native DLL's SHA-256 at compile time (WORLDPORT_NATIVE_SHA256)
# so end users only approve the DLL once; rebuilding the DLL therefore requires
# rebuilding the package.
#
# Usage:  ./scripts/build-plugin.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    Write-Host "building native module (native-release)…"
    cargo build --profile native-release -p worldport-native

    $dll = Join-Path $root "target/native-release/worldport_native.dll"
    if (-not (Test-Path $dll)) {
        throw "native module not found: $dll"
    }
    $digest = (Get-FileHash -Algorithm SHA256 $dll).Hash.ToLowerInvariant()
    Write-Host "native sha256: $digest"

    $nativeOut = Join-Path $root "dist/native/windows-x86_64-msvc"
    New-Item -ItemType Directory -Force -Path $nativeOut | Out-Null
    Copy-Item $dll (Join-Path $nativeOut "worldport_native.dll") -Force

    Write-Host "building plugin package (wasm32-wasip2)…"
    Push-Location (Join-Path $root "plugin")
    try {
        $env:WORLDPORT_NATIVE_SHA256 = $digest
        $env:BMCBL_PLUGIN_PACKAGE_OUT_DIR = Join-Path $root "dist"
        cargo build --release --target wasm32-wasip2
    } finally {
        Pop-Location
    }

    Get-ChildItem (Join-Path $root "dist/*.bmcblx") |
        ForEach-Object { Write-Host "package: $($_.FullName) ($($_.Length) bytes)" }
} finally {
    Pop-Location
}
