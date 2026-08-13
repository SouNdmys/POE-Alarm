[CmdletBinding()]
param(
    [string] $PackageDirectory = 'artifacts\rust-preview\POE-Alarm-Rust-Preview-0.1.0-win-x64',
    [string] $EvidenceDirectory = 'artifacts\rust-validation\package-isolation',
    [int] $StartupTimeoutSeconds = 15,
    [int] $ShutdownTimeoutSeconds = 10
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not ('PoeAlarmIsolatedSmokeNative' -as [type])) {
    Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class PoeAlarmIsolatedSmokeNative
{
    private delegate bool EnumWindowsProc(IntPtr window, IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowTextW(IntPtr window, StringBuilder text, int capacity);

    [DllImport("user32.dll")]
    private static extern bool IsWindowVisible(IntPtr window);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool PostMessageW(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);

    public sealed class WindowInfo
    {
        public long Handle { get; set; }
        public string Title { get; set; } = String.Empty;
    }

    public static WindowInfo[] VisibleWindowsForProcess(uint expectedProcessId)
    {
        var windows = new List<WindowInfo>();
        EnumWindows((window, _) =>
        {
            GetWindowThreadProcessId(window, out uint processId);
            if (processId != expectedProcessId || !IsWindowVisible(window))
                return true;

            var title = new StringBuilder(512);
            GetWindowTextW(window, title, title.Capacity);
            windows.Add(new WindowInfo { Handle = window.ToInt64(), Title = title.ToString() });
            return true;
        }, IntPtr.Zero);
        return windows.ToArray();
    }

    public static bool CloseWindow(long handle) =>
        PostMessageW(new IntPtr(handle), 0x0010, IntPtr.Zero, IntPtr.Zero);
}
'@
}

function Resolve-WorkspacePath([string] $Path) {
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Path))
}

function Get-OptionalHash([string] $Path) {
    if (Test-Path -LiteralPath $Path -PathType Leaf) {
        return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
    }
    return $null
}

$package = Resolve-WorkspacePath $PackageDirectory
$evidence = Resolve-WorkspacePath $EvidenceDirectory
$executable = Join-Path $package 'PoeAlarm.exe'
$manifest = Join-Path $package 'PACKAGE-MANIFEST.json'
$checksums = Join-Path $package 'SHA256SUMS.txt'
foreach ($required in @($executable, $manifest, $checksums)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "required packaged file is missing: $required"
    }
}

New-Item -ItemType Directory -Force -Path $evidence | Out-Null
$sandbox = Join-Path $evidence ("sandbox-{0}-{1}" -f $PID, [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())
$localAppData = Join-Path $sandbox 'LocalAppData'
$roamingAppData = Join-Path $sandbox 'Roaming'
$profile = Join-Path $sandbox 'Profile'
$temporary = Join-Path $sandbox 'Temp'
New-Item -ItemType Directory -Force -Path $localAppData, $roamingAppData, $profile, $temporary | Out-Null

$releasedSettings = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'PoeAlarm\settings.json'
$releasedHashBefore = Get-OptionalHash $releasedSettings
$startedAt = [DateTimeOffset]::UtcNow
$process = $null
$mainWindow = $null
$visibleWindows = @()

try {
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $executable
    $start.WorkingDirectory = $package
    $start.UseShellExecute = $false
    $start.Environment['LOCALAPPDATA'] = $localAppData
    $start.Environment['APPDATA'] = $roamingAppData
    $start.Environment['USERPROFILE'] = $profile
    $start.Environment['TEMP'] = $temporary
    $start.Environment['TMP'] = $temporary
    $start.Environment['PATH'] = "$package;$env:SystemRoot\System32;$env:SystemRoot"
    $process = [System.Diagnostics.Process]::Start($start)
    if ($null -eq $process) { throw 'packaged process did not start' }

    $startupDeadline = [DateTimeOffset]::UtcNow.AddSeconds($StartupTimeoutSeconds)
    do {
        if ($process.HasExited) {
            throw "packaged process exited before its main window appeared (exit $($process.ExitCode))"
        }
        $visibleWindows = @([PoeAlarmIsolatedSmokeNative]::VisibleWindowsForProcess([uint32]$process.Id))
        $mainWindow = $visibleWindows | Where-Object {
            $_.Title -match '流放之路词缀提醒|POE Alarm.*Rust Preview'
        } | Select-Object -First 1
        if ($null -eq $mainWindow) { Start-Sleep -Milliseconds 25 }
    } while ($null -eq $mainWindow -and [DateTimeOffset]::UtcNow -lt $startupDeadline)

    if ($null -eq $mainWindow) {
        $titles = ($visibleWindows | ForEach-Object Title) -join ' | '
        throw "main window did not appear within $StartupTimeoutSeconds seconds; visible titles: $titles"
    }
    $mainWindowAppearedAt = [DateTimeOffset]::UtcNow
    if (-not [PoeAlarmIsolatedSmokeNative]::CloseWindow([long]$mainWindow.Handle)) {
        throw 'WM_CLOSE could not be posted to the packaged main window'
    }
    if (-not $process.WaitForExit($ShutdownTimeoutSeconds * 1000)) {
        throw "packaged process did not close within $ShutdownTimeoutSeconds seconds"
    }
    if ($process.ExitCode -ne 0) {
        throw "packaged process exited with code $($process.ExitCode)"
    }
    $exitedAt = [DateTimeOffset]::UtcNow

    $releasedHashAfter = Get-OptionalHash $releasedSettings
    if ($releasedHashBefore -ne $releasedHashAfter) {
        throw 'the released .NET settings file changed during the isolated Rust smoke test'
    }

    $previewSettings = Join-Path $localAppData 'PoeAlarm-RustPreview\settings.json'
    $report = [ordered]@{
        package = $package
        executableSha256 = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash
        processId = $process.Id
        mainWindowTitle = $mainWindow.Title
        startupMilliseconds = [Math]::Round(($mainWindowAppearedAt - $startedAt).TotalMilliseconds, 3)
        shutdownMilliseconds = [Math]::Round(($exitedAt - $mainWindowAppearedAt).TotalMilliseconds, 3)
        exitCode = $process.ExitCode
        isolatedLocalAppData = $localAppData
        previewSettingsCreated = Test-Path -LiteralPath $previewSettings -PathType Leaf
        releasedSettingsPath = $releasedSettings
        releasedSettingsSha256Before = $releasedHashBefore
        releasedSettingsSha256After = $releasedHashAfter
        minimalPath = $start.Environment['PATH']
        result = 'pass'
        limitation = 'This is an isolated loader/settings smoke test on the development Windows installation, not a clean-OS field test.'
    }
    $reportPath = Join-Path $evidence 'isolated-package-smoke.json'
    Set-Content -LiteralPath $reportPath -Value ($report | ConvertTo-Json -Depth 4) -Encoding utf8NoBOM
    $report | Format-List
    "Evidence: $reportPath"
}
finally {
    if ($null -ne $process) {
        if (-not $process.HasExited) {
            $process.Kill($true)
            $process.WaitForExit()
        }
        $process.Dispose()
    }
    $resolvedEvidence = [System.IO.Path]::GetFullPath($evidence).TrimEnd('\') + '\'
    $resolvedSandbox = [System.IO.Path]::GetFullPath($sandbox)
    if ($resolvedSandbox.StartsWith($resolvedEvidence, [StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $resolvedSandbox)) {
        Remove-Item -LiteralPath $resolvedSandbox -Recurse -Force
    }
}
