param(
    [Parameter(Mandatory)][string]$BmcblRoot
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$vendor = Join-Path $repo 'vendor'

foreach ($crate in @('bedrock-nbt', 'bedrock-leveldb', 'bedrock-world')) {
    $source = Join-Path $BmcblRoot "crates/$crate"
    $target = Join-Path $vendor $crate
    if (!(Test-Path -LiteralPath $source)) { throw "missing $source" }
    if (Test-Path -LiteralPath $target) { Remove-Item -Recurse -Force -LiteralPath $target }
    Copy-Item -Recurse -LiteralPath $source -Destination $target
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $target 'target')

    $manifest = Join-Path $target 'Cargo.toml'
    $text = Get-Content -LiteralPath $manifest -Raw
    # The workspace root is not copied, so pin the shared dependency locally.
    $text = $text -replace 'fearless_simd = \{ workspace = true, optional = true \}', 'fearless_simd = { version = "=1.1.0", optional = true }'
    Set-Content -LiteralPath $manifest -Value $text -NoNewline
    Write-Output "synced $crate"
}
