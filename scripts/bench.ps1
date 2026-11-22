# Measures wall time and peak working set for a conversion.
#
# Speed alone hides regressions: a change that trades memory for time (or the
# reverse) only shows up when both are reported, so every run prints both.
#
# Usage:
#   ./scripts/bench.ps1 -World <dir> -Out <dir> [-Arguments "<extra cli args>"] [-Runs 3]
#   ./scripts/bench.ps1 -Chunker -World <dir> -Out <dir> -Format JAVA_26_2 [-Runs 3]
param(
    [Parameter(Mandatory = $true)][string]$World,
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$Arguments = "",
    [int]$Runs = 3,
    [switch]$Chunker,
    [string]$Format = "JAVA_26_2",
    [string]$Java = "C:/Program Files/Zulu/zulu-21/bin/java.exe",
    [string]$ChunkerJar = "",
    [int]$PollMs = 40
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $ChunkerJar) {
    $ChunkerJar = Join-Path $root "_refs/chunker/chunker-cli-1.21.0.jar"
}

function Invoke-Measured {
    param([string]$File, [string]$ArgLine)
    if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process -FilePath $File -ArgumentList $ArgLine -PassThru -NoNewWindow
    $peak = 0
    while (-not $process.HasExited) {
        try {
            $current = Get-Process -Id $process.Id -ErrorAction Stop
            if ($current.PeakWorkingSet64 -gt $peak) { $peak = $current.PeakWorkingSet64 }
        } catch {}
        Start-Sleep -Milliseconds $PollMs
    }
    $watch.Stop()
    [pscustomobject]@{
        exit   = $process.ExitCode
        wall_s = [math]::Round($watch.Elapsed.TotalSeconds, 2)
        peak_mib = [math]::Round($peak / 1MB, 1)
    }
}

if ($Chunker) {
    if (-not (Test-Path $Java)) { throw "java not found: $Java" }
    if (-not (Test-Path $ChunkerJar)) { throw "chunker jar not found: $ChunkerJar" }
    $argLine = "-Xmx8G -jar `"$ChunkerJar`" -i `"$World`" -o `"$Out`" -f $Format"
    $file = $Java
} else {
    $exe = Join-Path $root "target/native-release/worldport.exe"
    if (-not (Test-Path $exe)) { throw "build it first: cargo build --profile native-release -p worldport-cli" }
    $argLine = "convert `"$World`" `"$Out`" $Arguments"
    $file = $exe
}

Write-Host "command: $file $argLine"
$results = @()
for ($run = 1; $run -le $Runs; $run++) {
    $result = Invoke-Measured -File $file -ArgLine $argLine
    $results += $result
    Write-Host ("run {0}: exit={1} wall={2}s peak={3} MiB" -f $run, $result.exit, $result.wall_s, $result.peak_mib)
}

# Report per-run medians rather than "best of each metric": the fastest run and
# the hungriest run are usually different runs, so mixing them invents a
# combination that never happened. Peak memory is monotonic per run, so its
# maximum is the only meaningful summary; wall time uses the median because a
# single scheduling hiccup should not become the headline.
function Get-Median([double[]]$values) {
    $sorted = $values | Sort-Object
    $count = $sorted.Count
    if ($count -eq 0) { return 0 }
    if ($count % 2 -eq 1) { return $sorted[[int](($count - 1) / 2)] }
    return ($sorted[$count / 2 - 1] + $sorted[$count / 2]) / 2
}

$size = if (Test-Path $Out) {
    [math]::Round(((Get-ChildItem $Out -Recurse -File | Measure-Object Length -Sum).Sum) / 1MB, 1)
} else { 0 }
$walls = $results | ForEach-Object { $_.wall_s }
$peaks = $results | ForEach-Object { $_.peak_mib }
Write-Host ("median wall={0}s  max peak_working_set={1} MiB  output={2} MiB  runs={3}" -f `
    [math]::Round((Get-Median $walls), 2), ($peaks | Measure-Object -Maximum).Maximum, $size, $Runs)
Write-Host "note: peak_working_set is Windows' PeakWorkingSet64; it is not a portable RSS figure."
