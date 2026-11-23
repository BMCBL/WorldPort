# Run engines in rotation against the same input; each attempt owns a fresh output.
param(
    [Parameter(Mandatory)][string]$World,
    [Parameter(Mandatory)][string]$Out,
    [string[]]$Engines = @('Cli'),
    [string]$Baseline,
    [string]$DllRunner,
    [string]$Dll,
    [string[]]$ExtraArgs = @(),
    [string]$ChunkerSettings,
    [int]$Runs = 5,
    [switch]$Chunker,
    [ValidateSet('JAVA_26_2', 'BEDROCK_R26_60')][string]$Format = 'JAVA_26_2',
    [string]$Java = 'C:/Program Files/Zulu/zulu-21/bin/java.exe',
    [string]$ChunkerJar,
    [int]$TimeoutSeconds = 3600
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$root = Split-Path -Parent $PSScriptRoot
$cli = Join-Path $root 'target/native-release/worldport.exe'
if (!$ChunkerJar) { $ChunkerJar = Join-Path $root '_refs/chunker/chunker-cli-1.21.0.jar' }
if ($Chunker) { $Engines = @('Chunker') }
if ($Runs -lt 1 -or $TimeoutSeconds -lt 1) { throw 'Runs and TimeoutSeconds must be positive' }
if ($ExtraArgs.Count -and ($Engines -contains 'Dll' -or $Engines -contains 'DllSingle')) {
    throw 'ExtraArgs applies only to CLI engines. Use a separate run for the fixed DLL configuration.'
}

$target = '26.2'
if ($Format -eq 'BEDROCK_R26_60') { $target = '1.26.60' }

$source = (Resolve-Path -LiteralPath $World).ProviderPath
$outRoot = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out)
$sourcePrefix = $source.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
if ($outRoot -eq $source -or $outRoot.StartsWith($sourcePrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Benchmark output must be outside the input world.'
}
foreach ($path in @($source, (Split-Path -Parent $outRoot))) {
    while ($path) {
        if ((Test-Path -LiteralPath $path) -and ((Get-Item -LiteralPath $path -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Benchmark input and output cannot use redirects: $path"
        }
        $path = Split-Path -Parent $path
    }
}

$preflight = @{
    Cli = @($cli)
    Baseline = @($Baseline)
    Chunker = @($Java, $ChunkerJar)
    Dll = @($DllRunner, $Dll)
    DllSingle = @($DllRunner, $Dll)
}
foreach ($engine in $Engines) {
    if (!$preflight.ContainsKey($engine)) { throw "Unknown engine: $engine" }
    foreach ($file in $preflight[$engine]) {
        if (!$file -or !(Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing input file for ${engine}: $file" }
    }
}
if ($Engines -contains 'Chunker') {
    $expected = '63148E587F5EE5A2CA7EBAE1079B235C82AE4D1EFF8F7A8A8A3398332513DDA6'
    if ((Get-FileHash -LiteralPath $ChunkerJar).Hash -ne $expected) { throw 'Chunker JAR differs from the pinned 1.21.0 artifact' }
}
if (!(Test-Path -LiteralPath $cli -PathType Leaf)) { throw "Missing inspector: $cli" }
New-Item -ItemType Directory -Path $outRoot -ErrorAction Stop | Out-Null
[ordered]@{
    source = $source
    format = $Format
    rounds = $Runs
    extra_args = $ExtraArgs
    chunker_settings = $ChunkerSettings
    tools = @($Engines | ForEach-Object { $preflight[$_] } | Sort-Object -Unique |
        ForEach-Object { Get-FileHash -LiteralPath $_ | Select-Object Path, Hash })
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath "$outRoot/manifest.json" -Encoding utf8

function Start-Measured([string]$File, [string[]]$Argv, [string]$Log) {
    $info = [Diagnostics.ProcessStartInfo]::new($File)
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($arg in $Argv) { $info.ArgumentList.Add($arg) }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $process = [Diagnostics.Process]::Start($info)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $peakWorkingSet = 0L
        $peakCommit = 0L
        $cpu = 0.0
        $timedOut = $false
        do {
            try {
                $process.Refresh()
                $peakWorkingSet = [math]::Max($peakWorkingSet, $process.PeakWorkingSet64)
                $peakCommit = [math]::Max($peakCommit, $process.PeakPagedMemorySize64)
                $cpu = $process.TotalProcessorTime.TotalSeconds
            } catch [InvalidOperationException] {}
            if ($watch.Elapsed.TotalSeconds -ge $TimeoutSeconds -and !$process.HasExited) {
                $process.Kill($true)
                $timedOut = $true
            }
        } until ($process.WaitForExit(25))
        $watch.Stop()
        try { $cpu = $process.TotalProcessorTime.TotalSeconds } catch [InvalidOperationException] {}
        $outText = $stdout.GetAwaiter().GetResult()
        $errText = $stderr.GetAwaiter().GetResult()
        Set-Content -LiteralPath "$Log.stdout" -Value $outText -Encoding utf8
        Set-Content -LiteralPath "$Log.stderr" -Value $errText -Encoding utf8
        [ordered]@{
            exit = $process.ExitCode
            timed_out = $timedOut
            wall_s = [math]::Round($watch.Elapsed.TotalSeconds, 4)
            cpu_s = [math]::Round($cpu, 4)
            peak_working_set_mib = [math]::Round($peakWorkingSet / 1MB, 2)
            peak_commit_mib = [math]::Round($peakCommit / 1MB, 2)
        }
    } finally {
        $process.Dispose()
    }
}

function Invoke-Engine([string]$Engine, [int]$Round) {
    $name = "$Round-$Engine"
    $dest = Join-Path $outRoot "$name-world"
    $report = Join-Path $outRoot "$name-report.json"
    $argsCli = @('convert', $source, $dest, '--target-version', $target, '--report', $report) + $ExtraArgs
    switch ($Engine) {
        Cli { $file = $cli; $argv = $argsCli }
        Baseline { $file = $Baseline; $argv = $argsCli }
        Chunker {
            $file = $Java
            $argv = @('-Xmx8G', '-jar', $ChunkerJar, '-i', $source, '-o', $dest, '-f', $Format)
            if ($ChunkerSettings) { $argv += @('-c', $ChunkerSettings) }
        }
        { $_ -in 'Dll', 'DllSingle' } {
            $jobs = if ($_ -eq 'DllSingle') { '1' } else { '0' }
            $file = $DllRunner
            $argv = @($Dll, $source, $dest, $target, $jobs, $report)
        }
    }
    $result = Start-Measured $file $argv (Join-Path $outRoot $name)
    $result.engine = $Engine
    $result.round = $Round
    $result.world = $dest
    $result.valid = $false
    if ($result.exit -eq 0 -and !$result.timed_out) {
        try {
            if (!(Test-Path -LiteralPath "$dest/level.dat" -PathType Leaf)) { throw 'Missing level.dat' }
            $inspection = & $cli inspect $dest --json 2> (Join-Path $outRoot "$name-inspect.stderr")
            if ($LASTEXITCODE -ne 0) { throw "World inspection exited with status $LASTEXITCODE" }
            $info = $inspection | Out-String | ConvertFrom-Json
            $result.dimensions = $info.dimensions
            $expectedVersion = if ($Format -eq 'JAVA_26_2') {
                $info.edition -eq 'java' -and $info.data_version -eq 4903
            } else {
                $info.edition -eq 'bedrock' -and $info.bedrock_version -match '^1\.26\.60(\.|$)'
            }
            $result.valid = $expectedVersion -and ($info.dimensions | Measure-Object chunks -Sum).Sum -gt 0
            $result.output_bytes = (Get-ChildItem -LiteralPath $dest -Recurse -File | Measure-Object Length -Sum).Sum
            Set-Content -LiteralPath (Join-Path $outRoot "$name-inspect.json") -Value $inspection -Encoding utf8
        } catch {
            $result.validation_error = $_.Exception.Message
        }
    }
    $result | ConvertTo-Json -Depth 10 -Compress | Add-Content -LiteralPath (Join-Path $outRoot 'runs.jsonl') -Encoding utf8
    Write-Host "$name (exit $($result.exit), valid $($result.valid)) $($result.wall_s)s, $($result.peak_working_set_mib) MiB working set"
    [pscustomobject]$result
}

$results = @(
    for ($round = 0; $round -le $Runs; $round++) {
        for ($index = 0; $index -lt $Engines.Count; $index++) {
            Invoke-Engine $Engines[($index + $round) % $Engines.Count] $round
        }
    }
)
foreach ($engine in $Engines) {
    $samples = @($results | Where-Object { $_.engine -eq $engine -and $_.round -gt 0 -and $_.valid })
    if (!$samples.Count) { continue }
    $walls = @($samples.wall_s | Sort-Object)
    $mid = $walls.Count -shr 1
    $median = if ($walls.Count % 2) { $walls[$mid] } else { ($walls[$mid - 1] + $walls[$mid]) / 2 }
    $peakWs = ($samples.peak_working_set_mib | Measure-Object -Maximum).Maximum
    $peakCommit = ($samples.peak_commit_mib | Measure-Object -Maximum).Maximum
    Write-Host "$engine median ${median}s, range $($walls[0])–$($walls[-1])s, peaks $peakWs MiB working set / $peakCommit MiB commit ($($samples.Count)/$Runs valid attempts)"
}
if ($results | Where-Object { !$_.valid }) { exit 1 }
