# Builds one plugin package and the selected native modules.
param(
    [ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc', 'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu')]
    [string[]]$Targets
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$root = Split-Path -Parent $PSScriptRoot
$supported = @{
    'x86_64-pc-windows-msvc' = @('windows-x86_64-msvc', 'worldport_native.dll')
    'aarch64-pc-windows-msvc' = @('windows-aarch64-msvc', 'worldport_native.dll')
    'x86_64-unknown-linux-gnu' = @('linux-x86_64-gnu', 'libworldport_native.so')
    'aarch64-unknown-linux-gnu' = @('linux-aarch64-gnu', 'libworldport_native.so')
}
Push-Location $root
try {
    $hostLine = rustc -vV | Where-Object { $_ -like 'host: *' }
    if (!$hostLine) { throw 'Could not determine the Rust host target' }
    $hostTarget = $hostLine.Substring(6)
    if (!$Targets) {
        if (!$supported.ContainsKey($hostTarget)) {
            throw "Unsupported native build host: $hostTarget"
        }
        $Targets = @($hostTarget)
    }
    $components = @(
        foreach ($target in ($Targets | Sort-Object -Unique)) {
            $platform, $library = $supported[$target]
            $targetDir = Join-Path $root 'target'
            $cargoArgs = @('build', '--locked', '--profile', 'native-release', '--target-dir', $targetDir, '-p', 'worldport-native')
            if ($target -ne $hostTarget) {
                $cargoArgs += @('--target', $target)
                $targetDir = Join-Path $targetDir $target
            }
            cargo @cargoArgs | Out-Host
            $path = Join-Path $targetDir "native-release/$library"
            $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
            [pscustomobject]@{ Platform = $platform; Path = $path; Library = $library; Digest = $hash }
        }
    )

    $previousComponents = $env:WORLDPORT_NATIVE_COMPONENTS
    $previousOut = $env:BMCBL_PLUGIN_PACKAGE_OUT_DIR
    Push-Location (Join-Path $root 'plugin')
    try {
        $env:WORLDPORT_NATIVE_COMPONENTS = ($components | ForEach-Object { "$($_.Platform)=$($_.Digest)" }) -join [Environment]::NewLine
        $env:BMCBL_PLUGIN_PACKAGE_OUT_DIR = Join-Path $root 'dist'
        cargo build --locked --release --target wasm32-wasip2
    } finally {
        $env:WORLDPORT_NATIVE_COMPONENTS = $previousComponents
        $env:BMCBL_PLUGIN_PACKAGE_OUT_DIR = $previousOut
        Pop-Location
    }

    $packageVersion = Select-String -LiteralPath 'plugin/Cargo.toml' -Pattern '^version = "([^"]+)"$' | Select-Object -First 1
    if (!$packageVersion) { throw 'Missing WorldPort plugin version' }
    $package = Get-Item -LiteralPath "dist/worldport-$($packageVersion.Matches[0].Groups[1].Value).bmcblx"
    @(
        foreach ($component in $components) {
            $relative = "native/$($component.Platform)/$($component.Library)"
            $artifact = Join-Path "$root/dist" $relative
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $artifact) | Out-Null
            Copy-Item -LiteralPath $component.Path -Destination $artifact -Force
            "$($component.Digest) *$relative"
        }
        "$((Get-FileHash -LiteralPath $package.FullName).Hash.ToLowerInvariant()) *$($package.Name)"
    ) | Set-Content -LiteralPath "$root/dist/SHA256SUMS" -Encoding ascii
    Write-Host "package: $($package.FullName) ($($package.Length) bytes)"
} finally {
    Pop-Location
}
