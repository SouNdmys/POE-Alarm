using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Text.Json;

namespace PoeAlarm.App.Alerts;

/// <summary>
/// Command-line audio diagnostic used by packaging checks and support. It verifies the selected
/// file using the exact production validator, submits the exact production playback request, and
/// leaves it audible briefly. A successful WinMM request cannot prove the user's speakers are
/// powered on, but it does catch malformed assets, unsupported formats, and playback API failure.
/// </summary>
internal static class AlertSoundSelfTest
{
    public static int Run(string reportPath, string? customWavePath = null)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(reportPath);

        var requestedCustomWave = !string.IsNullOrWhiteSpace(customWavePath);
        var customWaveValidated = true;
        string? validationError = null;
        if (requestedCustomWave &&
            !LoopingAlertSound.TryValidateCustomWaveFile(customWavePath, out var error))
        {
            customWaveValidated = false;
            validationError = error;
        }

        var stopwatch = Stopwatch.StartNew();
        bool playbackRequestAccepted;
        bool usingCustomWave;
        using (var sound = new LoopingAlertSound(customWavePath))
        {
            usingCustomWave = sound.IsUsingCustomWave;
            playbackRequestAccepted = sound.Start();
            Thread.Sleep(TimeSpan.FromMilliseconds(1_350));
            sound.Stop();
        }

        stopwatch.Stop();
        var passed = customWaveValidated &&
                     (!requestedCustomWave || usingCustomWave) &&
                     playbackRequestAccepted;
        var result = new
        {
            SchemaVersion = 1,
            AppVersion = Assembly.GetExecutingAssembly().GetName().Version?.ToString(),
            Passed = passed,
            RequestedCustomWave = requestedCustomWave,
            CustomWaveValidated = customWaveValidated,
            UsingCustomWave = usingCustomWave,
            PlaybackRequestAccepted = playbackRequestAccepted,
            PlaybackRoute = "application-multimedia-session",
            SystemSoundsRoutingDisabled = true,
            ValidationError = validationError,
            ElapsedMilliseconds = stopwatch.Elapsed.TotalMilliseconds,
        };

        var resolvedReportPath = Path.GetFullPath(reportPath);
        Directory.CreateDirectory(
            Path.GetDirectoryName(resolvedReportPath)
            ?? throw new InvalidOperationException("The audio report path has no parent directory."));
        File.WriteAllText(
            resolvedReportPath,
            JsonSerializer.Serialize(result, new JsonSerializerOptions { WriteIndented = true }));
        return passed ? 0 : 1;
    }
}
