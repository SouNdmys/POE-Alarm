using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Alerts;

/// <summary>
/// A latched user alert. Once triggered it remains active until explicitly acknowledged.
/// </summary>
public interface IAlertService : IDisposable
{
    /// <summary>Raised after an active alert has been explicitly acknowledged.</summary>
    event EventHandler? Acknowledged;

    /// <summary>Gets whether an alert is currently latched.</summary>
    bool IsActive { get; }

    /// <summary>Gets whether the short pre-recognition input guard is currently requested.</summary>
    bool IsInputGuardActive { get; }

    /// <summary>
    /// Applies UI-only alert preferences outside the recognition path. A missing custom WAV
    /// falls back to the built-in sound; capture exclusion never changes input blocking.
    /// </summary>
    void Configure(string? customWavePath, bool excludeOverlayFromCapture);

    /// <summary>
    /// Prepares the reusable red alert window and, when requested, the pass-through low-level
    /// mouse guard before monitoring begins so neither is created on the latency-critical path.
    /// This method may be called from any thread.
    /// </summary>
    void Prepare(bool preparePendingInputGuard = false);

    /// <summary>
    /// Arms the hook-only mouse guard while a changed tooltip frame is being recognized. No WPF
    /// window is shown, so POE retains mouse hover. It has a fail-open timeout and never starts
    /// the audible/red alert by itself.
    /// </summary>
    void BeginInputGuard(ScreenRegion? anchorRegion = null);

    /// <summary>Releases a pending input guard after OCR decides that the frame did not match.</summary>
    void ReleaseInputGuard();

    /// <summary>
    /// Displays and sounds the alert. Calls made while the alert is active are ignored.
    /// This method may be called from any thread.
    /// </summary>
    /// <param name="detectedText">Optional OCR text to show in the alert.</param>
    /// <param name="anchorRegion">Optional screen region whose monitor hosts the alert card.</param>
    void Trigger(string? detectedText = null, ScreenRegion? anchorRegion = null);

    /// <summary>
    /// Clears the latched alert, hides and releases the reusable overlay, and stops its sound.
    /// This method may be called from any thread.
    /// </summary>
    void Acknowledge();
}
