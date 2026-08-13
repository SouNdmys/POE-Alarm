using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Alerts;

/// <summary>
/// A latched user alert. Once triggered it remains active until explicitly acknowledged.
/// </summary>
public interface IAlertService : IDisposable
{
    /// <summary>Raised after an active alert has been explicitly acknowledged.</summary>
    event EventHandler? Acknowledged;

    /// <summary>
    /// Raised only after the yellow Guarded overlay has absorbed the complete confirmation click
    /// and released its full-desktop input shield.
    /// </summary>
    event EventHandler? GuardedContinueRequested;

    /// <summary>
    /// Raised only after the yellow Guarded overlay has absorbed the complete stop click and
    /// released its full-desktop input shield.
    /// </summary>
    event EventHandler? GuardedStopRequested;

    /// <summary>
    /// Raised when Guarded input protection cannot be installed, times out, or cannot be handed
    /// to a visible blocking overlay. Subscribers must immediately stop/fault Guarded monitoring.
    /// </summary>
    event EventHandler<GuardedAlertFailOpenEventArgs>? GuardedFailOpen;

    /// <summary>
    /// Raised after the one allowed Guarded crafting click has delivered its final button-up and
    /// the native gate has atomically returned to blocking subsequent clicks.
    /// </summary>
    event EventHandler? GuardedCausativeClickCompleted;

    /// <summary>Raised after the one allowed Guarded button-down has reached the game.</summary>
    event EventHandler? GuardedCausativeClickStarted;

    /// <summary>Gets whether an alert is currently latched.</summary>
    bool IsActive { get; }

    /// <summary>Gets whether the short pre-recognition input guard is currently requested.</summary>
    bool IsInputGuardActive { get; }

    /// <summary>Gets whether Guarded owns either the pending native gate or yellow overlay.</summary>
    bool IsGuardedAlertActive { get; }

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

    /// <summary>
    /// Arms the Guarded native mouse gate for the supplied decision lifetime. Unlike the legacy
    /// 750 ms input guard, its lifetime is explicit and intended to cover a complete Guarded
    /// capture/OCR/decision cycle. Failure raises <see cref="GuardedFailOpen"/>.
    /// </summary>
    void BeginGuardedInputGuard(TimeSpan watchdogTimeout, ScreenRegion? anchorRegion = null);

    /// <summary>Releases a pending input guard after OCR decides that the frame did not match.</summary>
    void ReleaseInputGuard();

    /// <summary>Releases a Guarded pending gate after a conclusive safe decision.</summary>
    void ReleaseGuardedInputGuard();

    /// <summary>
    /// Promotes the already-armed Guarded gate to a yellow full-desktop blocking overlay. All UI
    /// strings are supplied by the caller so localization remains outside the alert layer.
    /// </summary>
    void ShowGuardedUncertain(
        string title,
        string detail,
        string instruction,
        string continueButtonText,
        string stopButtonText,
        ScreenRegion? anchorRegion = null);

    /// <summary>
    /// While the yellow overlay still owns input, atomically prepares the native gate to pass
    /// exactly one complete causative click and guard all subsequent clicks. The decision
    /// watchdog starts when that click completes (or when a changed frame explicitly begins its
    /// Guarded decision). Returns false and raises <see cref="GuardedFailOpen"/> if the native
    /// gate cannot be prepared.
    /// </summary>
    bool AwaitNextGuardedClick(
        TimeSpan decisionWatchdogTimeout,
        ScreenRegion? anchorRegion = null);

    /// <summary>
    /// Displays and sounds the alert. Calls made while the alert is active are ignored.
    /// This method may be called from any thread.
    /// </summary>
    /// <param name="detectedText">Optional OCR text to show in the alert.</param>
    /// <param name="anchorRegion">Optional screen region whose monitor hosts the alert card.</param>
    void Trigger(string? detectedText = null, ScreenRegion? anchorRegion = null);

    /// <summary>
    /// Atomically promotes the currently owned Guarded native gate to a red match alert. Returns
    /// false when Guarded no longer owns input (for example, after its watchdog has begun a
    /// fail-open), so the caller can fault the monitoring session instead of assuming the red
    /// overlay took over.
    /// </summary>
    bool TriggerGuardedMatch(string? detectedText = null, ScreenRegion? anchorRegion = null);

    /// <summary>
    /// Clears the latched alert, hides and releases the reusable overlay, and stops its sound.
    /// This method may be called from any thread.
    /// </summary>
    void Acknowledge();
}

/// <summary>Reason a Guarded input owner was forced to fail open.</summary>
public enum GuardedAlertFailOpenReason
{
    InputGuardUnavailable,
    InputGuardTimedOut,
    DispatcherUnavailable,
    PresentationTimedOut,
    PresentationFailed,
}

public sealed class GuardedAlertFailOpenEventArgs(
    GuardedAlertFailOpenReason reason,
    string detail) : EventArgs
{
    public GuardedAlertFailOpenReason Reason { get; } = reason;

    public string Detail { get; } = detail;
}
