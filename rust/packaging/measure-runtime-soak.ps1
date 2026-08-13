[CmdletBinding()]
param(
    [ValidateSet('live', 'screenshot')]
    [string] $Mode = 'live',
    [double] $DurationSeconds = 60,
    [string] $ExecutablePath = 'rust\target-soak\release\poe-alarm-runtime-soak.exe',
    [string] $ImagePath,
    [ValidateSet('poe1', 'poe2')]
    [string] $Game = 'poe2',
    [ValidateSet('en', 'zh-TW')]
    [string] $Language = 'zh-TW',
    [string] $Template = 'POE Alarm resource soak target that must never match 9F4A7B2C',
    [string] $Region = '0,0,1433,1117',
    [string] $OnnxRuntimePath = 'rust\target\release\onnxruntime.dll',
    [string] $ModelPath = 'rust\target\release\PP-OCRv5_mobile_rec.onnx',
    [string] $DictionaryPath = 'rust\target\release\ppocrv5_dict.txt',
    [string] $WavePath,
    [Nullable[long]] $MaximumCycles,
    [double] $SampleIntervalSeconds = 1,
    [string] $EvidenceDirectory = 'artifacts\rust-validation\resource-soak',
    [double] $MaximumPrivateGrowthMiB = 5,
    [double] $MaximumHandleGrowthPercent = 2,
    [int] $MaximumThreadGrowth = 2,
    [double] $MaximumAverageCpuPercent = 25,
    [switch] $SkipThresholds
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Resolve-WorkspacePath([string] $Path) {
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Path))
}

function Require-File([string] $Path, [string] $Label) {
    $resolved = Resolve-WorkspacePath $Path
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw "$Label is missing: $resolved"
    }
    return $resolved
}

function New-SilentWave([string] $Path) {
    $sampleRate = 8000
    $samples = 800
    $dataBytes = $samples * 2
    $bytes = [byte[]]::new(44 + $dataBytes)
    [Text.Encoding]::ASCII.GetBytes('RIFF').CopyTo($bytes, 0)
    [BitConverter]::GetBytes([uint32](36 + $dataBytes)).CopyTo($bytes, 4)
    [Text.Encoding]::ASCII.GetBytes('WAVEfmt ').CopyTo($bytes, 8)
    [BitConverter]::GetBytes([uint32]16).CopyTo($bytes, 16)
    [BitConverter]::GetBytes([uint16]1).CopyTo($bytes, 20)
    [BitConverter]::GetBytes([uint16]1).CopyTo($bytes, 22)
    [BitConverter]::GetBytes([uint32]$sampleRate).CopyTo($bytes, 24)
    [BitConverter]::GetBytes([uint32]($sampleRate * 2)).CopyTo($bytes, 28)
    [BitConverter]::GetBytes([uint16]2).CopyTo($bytes, 32)
    [BitConverter]::GetBytes([uint16]16).CopyTo($bytes, 34)
    [Text.Encoding]::ASCII.GetBytes('data').CopyTo($bytes, 36)
    [BitConverter]::GetBytes([uint32]$dataBytes).CopyTo($bytes, 40)
    [IO.File]::WriteAllBytes($Path, $bytes)
}

if (-not [double]::IsFinite($DurationSeconds) -or $DurationSeconds -le 0) {
    throw 'DurationSeconds must be finite and positive.'
}
if (-not [double]::IsFinite($SampleIntervalSeconds) -or $SampleIntervalSeconds -le 0) {
    throw 'SampleIntervalSeconds must be finite and positive.'
}

$executable = Require-File $ExecutablePath 'soak executable'
$onnxRuntime = Require-File $OnnxRuntimePath 'ONNX Runtime'
$model = Require-File $ModelPath 'Paddle model'
$dictionary = Require-File $DictionaryPath 'Paddle dictionary'
$image = $null
if ($Mode -eq 'screenshot') {
    if ([string]::IsNullOrWhiteSpace($ImagePath)) {
        throw 'ImagePath is required in screenshot mode.'
    }
    $image = Require-File $ImagePath 'fixed real screenshot'
}

$evidence = Resolve-WorkspacePath $EvidenceDirectory
New-Item -ItemType Directory -Force -Path $evidence | Out-Null
$stamp = [DateTimeOffset]::UtcNow.ToString('yyyyMMdd-HHmmss')
$runName = "$stamp-$Mode"
$csvPath = Join-Path $evidence "$runName-samples.csv"
$jsonPath = Join-Path $evidence "$runName-summary.json"
$stdoutPath = Join-Path $evidence "$runName-stdout.log"
$stderrPath = Join-Path $evidence "$runName-stderr.log"
$generatedWave = $false
if ([string]::IsNullOrWhiteSpace($WavePath)) {
    $wave = Join-Path $evidence "$runName-silent.wav"
    $generatedWave = $true
} else {
    $wave = Require-File $WavePath 'diagnostic WAV'
}

$arguments = [Collections.Generic.List[string]]::new()
foreach ($pair in @(
    @('--mode', $Mode),
    @('--duration-seconds', $DurationSeconds.ToString('R', [Globalization.CultureInfo]::InvariantCulture)),
    @('--game', $Game),
    @('--language', $Language),
    @('--template', $Template),
    @('--region', $Region),
    @('--onnx-runtime', $onnxRuntime),
    @('--model', $model),
    @('--dictionary', $dictionary),
    @('--wave', $wave)
)) {
    $arguments.Add([string]$pair[0])
    $arguments.Add([string]$pair[1])
}
if ($null -ne $image) {
    $arguments.Add('--image')
    $arguments.Add($image)
}
if ($null -ne $MaximumCycles) {
    $arguments.Add('--maximum-cycles')
    $arguments.Add($MaximumCycles.ToString([Globalization.CultureInfo]::InvariantCulture))
}

$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = $executable
$start.WorkingDirectory = Split-Path -Parent $executable
$start.UseShellExecute = $false
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
foreach ($argument in $arguments) { $start.ArgumentList.Add($argument) }

$process = $null
$samples = [Collections.Generic.List[object]]::new()
$startedAt = [DateTimeOffset]::UtcNow
$processorCount = [Environment]::ProcessorCount
$ready = $false
try {
    if ($generatedWave) {
        New-SilentWave $wave
    }
    $process = [Diagnostics.Process]::Start($start)
    if ($null -eq $process) { throw 'soak process did not start' }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()

    $previousCpu = [TimeSpan]::Zero
    $previousElapsed = 0.0
    while (-not $process.HasExited) {
        $process.Refresh()
        $elapsed = ([DateTimeOffset]::UtcNow - $startedAt).TotalSeconds
        $cpu = $process.TotalProcessorTime
        $interval = $elapsed - $previousElapsed
        $cpuPercent = if ($interval -gt 0) {
            100.0 * ($cpu - $previousCpu).TotalSeconds / ($interval * $processorCount)
        } else { 0.0 }
        $samples.Add([pscustomobject][ordered]@{
            elapsed_seconds = [Math]::Round($elapsed, 3)
            cpu_percent = [Math]::Round($cpuPercent, 4)
            total_cpu_seconds = [Math]::Round($cpu.TotalSeconds, 6)
            private_bytes = $process.PrivateMemorySize64
            working_set_bytes = $process.WorkingSet64
            handles = $process.HandleCount
            threads = $process.Threads.Count
        })
        $previousCpu = $cpu
        $previousElapsed = $elapsed
        Start-Sleep -Milliseconds ([Math]::Max(10, [int]($SampleIntervalSeconds * 1000)))
    }
    $process.WaitForExit()
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    Set-Content -LiteralPath $stdoutPath -Value $stdout -Encoding utf8NoBOM
    Set-Content -LiteralPath $stderrPath -Value $stderr -Encoding utf8NoBOM
    $ready = $stdout.Contains('SOAK_READY')
    if ($process.ExitCode -ne 0) {
        throw "soak process exited with $($process.ExitCode): $stderr"
    }
    if (-not $ready -or -not $stdout.Contains('SOAK_COMPLETE') -or -not $stdout.Contains('SOAK_SHUTDOWN')) {
        throw 'soak process did not emit complete ready/run/shutdown markers'
    }
    if ($samples.Count -lt 2) { throw 'fewer than two process samples were captured' }

    $samples | Export-Csv -LiteralPath $csvPath -NoTypeInformation -Encoding utf8NoBOM
    $warmSamples = @($samples | Where-Object elapsed_seconds -ge ([Math]::Min(10.0, $DurationSeconds / 4.0)))
    if ($warmSamples.Count -lt 2) { $warmSamples = @($samples) }
    $first = $warmSamples[0]
    $last = $warmSamples[-1]
    $privateGrowth = [long]$last.private_bytes - [long]$first.private_bytes
    $handleGrowthPercent = if ([long]$first.handles -eq 0) { 0.0 } else {
        100.0 * ([long]$last.handles - [long]$first.handles) / [long]$first.handles
    }
    $threadGrowth = [int]$last.threads - [int]$first.threads
    $averageCpu = ($warmSamples | Measure-Object -Property cpu_percent -Average).Average
    $thresholdFailures = [Collections.Generic.List[string]]::new()
    if ($privateGrowth -gt $MaximumPrivateGrowthMiB * 1MB) {
        $thresholdFailures.Add("private byte growth $privateGrowth exceeds $MaximumPrivateGrowthMiB MiB")
    }
    if ($handleGrowthPercent -gt $MaximumHandleGrowthPercent) {
        $thresholdFailures.Add("handle growth $handleGrowthPercent% exceeds $MaximumHandleGrowthPercent%")
    }
    if ($threadGrowth -gt $MaximumThreadGrowth) {
        $thresholdFailures.Add("thread growth $threadGrowth exceeds $MaximumThreadGrowth")
    }
    if ($averageCpu -gt $MaximumAverageCpuPercent) {
        $thresholdFailures.Add("average CPU $averageCpu% exceeds $MaximumAverageCpuPercent%")
    }

    $wallClockTwoHour = $DurationSeconds -ge 7200 -and $null -eq $MaximumCycles
    $report = [ordered]@{
        schemaVersion = 1
        result = if ($thresholdFailures.Count -eq 0) { 'pass' } else { 'fail' }
        mode = $Mode
        processId = $process.Id
        executable = $executable
        executableSha256 = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash
        startedUtc = $startedAt.ToString('O')
        elapsedSeconds = [Math]::Round(([DateTimeOffset]::UtcNow - $startedAt).TotalSeconds, 3)
        configuredDurationSeconds = $DurationSeconds
        samples = $samples.Count
        warmSampleStartSeconds = $first.elapsed_seconds
        averageCpuPercent = [Math]::Round($averageCpu, 4)
        privateBytesStart = $first.private_bytes
        privateBytesEnd = $last.private_bytes
        privateBytesPeak = ($samples | Measure-Object -Property private_bytes -Maximum).Maximum
        privateBytesGrowth = $privateGrowth
        workingSetPeak = ($samples | Measure-Object -Property working_set_bytes -Maximum).Maximum
        handlesStart = $first.handles
        handlesEnd = $last.handles
        handlesPeak = ($samples | Measure-Object -Property handles -Maximum).Maximum
        handleGrowthPercent = [Math]::Round($handleGrowthPercent, 4)
        threadsStart = $first.threads
        threadsEnd = $last.threads
        threadsPeak = ($samples | Measure-Object -Property threads -Maximum).Maximum
        threadGrowth = $threadGrowth
        exitCode = $process.ExitCode
        frameSource = if ($Mode -eq 'live') { 'live GDI desktop region' } else { 'fixed real screenshot via WIC; not live gameplay' }
        productionComponents = @('RuntimeHandle/actor', 'NativeProtection/alert HWND owner', 'WinRT OCR', 'Paddle/ONNX fallback', $(if ($Mode -eq 'live') { 'GDI capture' } else { 'WIC decoder' }))
        wallClockTwoHourGate = $wallClockTwoHour
        classification = if ($wallClockTwoHour) { 'two-hour-wall-clock-gate' } elseif ($null -ne $MaximumCycles) { 'accelerated-workload-precheck-not-two-hour-wall-clock' } else { 'bounded-wall-clock-resource-gate' }
        thresholds = [ordered]@{
            maximumPrivateGrowthMiB = $MaximumPrivateGrowthMiB
            maximumHandleGrowthPercent = $MaximumHandleGrowthPercent
            maximumThreadGrowth = $MaximumThreadGrowth
            maximumAverageCpuPercent = $MaximumAverageCpuPercent
        }
        thresholdFailures = @($thresholdFailures)
        stdout = $stdoutPath
        stderr = $stderrPath
        samplesCsv = $csvPath
        limitation = if ($Mode -eq 'live') {
            'Captures the current desktop through production GDI. It is a real unchanged-frame runtime gate only when the selected region remains visually unchanged; it is not a human crafting session.'
        } else {
            'Uses a fixed real screenshot through production WIC/OCR/runtime. This is not live game capture and cannot satisfy the two-hour real-game field gate.'
        }
    }
    Set-Content -LiteralPath $jsonPath -Value ($report | ConvertTo-Json -Depth 8) -Encoding utf8NoBOM
    $report | Format-List
    "Evidence: $jsonPath"
    if (-not $SkipThresholds -and $thresholdFailures.Count -gt 0) {
        throw "resource thresholds failed: $($thresholdFailures -join '; ')"
    }
}
finally {
    if ($null -ne $process) {
        if (-not $process.HasExited) {
            $process.Kill($true)
            $process.WaitForExit()
        }
        $process.Dispose()
    }
    if ($generatedWave -and (Test-Path -LiteralPath $wave -PathType Leaf)) {
        Remove-Item -LiteralPath $wave -Force
    }
}
