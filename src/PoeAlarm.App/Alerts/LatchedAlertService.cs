using System.Windows;
using System.Windows.Threading;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Localization;

namespace PoeAlarm.App.Alerts;

/// <summary>
/// Shows a foreground red input shield and loops an audible alarm until acknowledged.
/// </summary>
public sealed class LatchedAlertService : IAlertService
{
    private static readonly TimeSpan InputGuardFailOpenTimeout = TimeSpan.FromMilliseconds(750);
    private static readonly TimeSpan AlertHandoffFailOpenTimeout = TimeSpan.FromMilliseconds(750);

    private readonly object stateGate = new();
    private readonly Dispatcher dispatcher;
    private LoopingAlertSound sound = new();
    private readonly PendingMouseInputGuard pendingMouseInputGuard = new();

    // Accessed only on the WPF dispatcher.
    private AffixHitOverlayWindow? overlay;

    private bool isActive;
    private bool isInputGuardActive;
    private bool isDisposed;
    private bool excludeOverlayFromCapture;
    private string? customWavePath;
    private int generation;

    /// <summary>Creates an alert service attached to the current WPF application.</summary>
    public LatchedAlertService()
        : this(Application.Current?.Dispatcher
            ?? throw new InvalidOperationException(
                "Create LatchedAlertService after the WPF Application has been initialized."))
    {
    }

    /// <summary>Creates an alert service attached to a specific UI dispatcher.</summary>
    public LatchedAlertService(Dispatcher dispatcher)
    {
        ArgumentNullException.ThrowIfNull(dispatcher);
        this.dispatcher = dispatcher;
    }

    public bool IsActive
    {
        get
        {
            lock (stateGate)
            {
                return isActive;
            }
        }
    }

    public bool IsInputGuardActive
    {
        get
        {
            lock (stateGate)
            {
                return isInputGuardActive;
            }
        }
    }

    public event EventHandler? Acknowledged;

    public void Configure(string? customWavePath, bool excludeOverlayFromCapture)
    {
        LoopingAlertSound? previousSound = null;
        lock (stateGate)
        {
            if (isDisposed)
            {
                return;
            }

            var normalizedPath = string.IsNullOrWhiteSpace(customWavePath)
                ? null
                : customWavePath.Trim();
            if (!string.Equals(this.customWavePath, normalizedPath, StringComparison.OrdinalIgnoreCase))
            {
                previousSound = sound;
                sound = new LoopingAlertSound(normalizedPath);
                this.customWavePath = sound.CustomWavePath;
            }

            this.excludeOverlayFromCapture = excludeOverlayFromCapture;
        }

        previousSound?.Dispose();
        _ = TryDispatch(() =>
        {
            overlay?.SetExcludeFromCapture(excludeOverlayFromCapture);
            overlay?.ApplyLanguage();
        });
    }

    public void Prepare(bool preparePendingInputGuard = false)
    {
        lock (stateGate)
        {
            if (isDisposed)
            {
                return;
            }

            if (preparePendingInputGuard)
            {
                // Install the pass-through hook before a Traditional Chinese monitor starts. It
                // tracks physical button pairing but consumes nothing until BeginInputGuard arms
                // it. Keep native and logical state serialized by stateGate throughout the
                // service; the hook never calls back into this service, so this lock order is safe.
                if (!pendingMouseInputGuard.Prepare())
                {
                    throw new InvalidOperationException(
                        UiText.Current.PendingGuardInstallFailed);
                }
            }
        }

        _ = TryDispatch(PrepareOverlay);
    }

    public void BeginInputGuard(ScreenRegion? anchorRegion = null)
    {
        int guardGeneration;
        lock (stateGate)
        {
            if (isDisposed || isActive)
            {
                return;
            }

            // A progressive OCR slice refreshes the generation, native hook state, and watchdog.
            // No WPF presentation is queued while the frame is pending.
            isInputGuardActive = true;
            guardGeneration = ++generation;
            // This is deliberately synchronous and happens on the monitor worker before OCR.
            // Keeping it in the same critical section as generation prevents a stale Begin from
            // arming after Stop, or a stale Release from disarming a newer progressive slice.
            if (!pendingMouseInputGuard.Arm())
            {
                isInputGuardActive = false;
                generation++;
                throw new InvalidOperationException(
                    UiText.Current.PendingGuardArmFailed);
            }
        }

        // Keep the WPF overlay natively hidden while OCR is pending. A full-desktop HWND, even
        // when visually transparent and non-activating, becomes the mouse target and makes POE
        // drop its item-tooltip hover. That tooltip disappearance changes the captured frame and
        // can self-trigger an endless guard/show/hide loop. The already-armed low-level hook is
        // synchronous and sufficient here; the overlay is promoted only after a real match.
        _ = anchorRegion;

        _ = FailOpenInputGuardAsync(guardGeneration);
    }

    public void ReleaseInputGuard()
    {
        int releaseGeneration;
        var logicalGuardWasActive = false;
        lock (stateGate)
        {
            if (isDisposed || isActive)
            {
                return;
            }

            if (isInputGuardActive)
            {
                isInputGuardActive = false;
                releaseGeneration = ++generation;
                logicalGuardWasActive = true;
            }
            else
            {
                releaseGeneration = generation;
            }

            // Also lets Stop clean up a pass-through hook that was prepared but never armed.
            pendingMouseInputGuard.Release();
        }

        if (!logicalGuardWasActive)
        {
            return;
        }

        _ = TryDispatch(() => CompleteInputGuardRelease(releaseGeneration));
    }

    public void Trigger(string? detectedText = null, ScreenRegion? anchorRegion = null)
    {
        int triggerGeneration;

        lock (stateGate)
        {
            if (isDisposed || isActive)
            {
                return;
            }

            isActive = true;
            isInputGuardActive = false;
            triggerGeneration = ++generation;

        }

        var displayText = string.IsNullOrWhiteSpace(detectedText)
            ? UiText.Current.DefaultDetectedText
            : detectedText.Trim();

        // Queue or present the input shield before starting any non-critical alert work. Sound is
        // deliberately started by ShowOverlay only after the native shield is visible.
        if (!TryDispatch(() => ShowOverlay(displayText, triggerGeneration, anchorRegion)))
        {
            CancelTrigger(triggerGeneration);
            return;
        }

        // A queued dispatcher presentation must not leave the system-wide low-level hook armed
        // forever if the UI thread stalls. The red window may still appear later, but the hook
        // independently fails open after this bounded hand-off interval.
        _ = FailOpenAlertHandoffAsync(triggerGeneration);
    }

    public void Acknowledge()
    {
        int acknowledgementGeneration;
        var transitionedToAcknowledged = false;
        lock (stateGate)
        {
            if (isDisposed)
            {
                return;
            }

            if (isActive || isInputGuardActive)
            {
                var wasAlertActive = isActive;
                isActive = false;
                isInputGuardActive = false;
                generation++;
                sound.Stop();
                transitionedToAcknowledged = wasAlertActive;
            }

            acknowledgementGeneration = generation;
            if (transitionedToAcknowledged)
            {
                // A normal acknowledgement normally has a blocking red HWND. If it raced the
                // presentation, explicit acknowledgement is still the requested fail-open path.
                pendingMouseInputGuard.TransferToBlockingOverlay();
            }
            else
            {
                // A pending guard has no visible owner yet. Drain any consumed Down/Up pair before
                // uninstalling so an unmatched Up is never sent to the game.
                pendingMouseInputGuard.Release();
            }
        }

        // Hiding is harmless when no window exists, so an acknowledgement also repairs any stale
        // overlay left behind after an unexpected presentation error.
        _ = TryDispatch(() => CompleteAcknowledgement(
            acknowledgementGeneration,
            transitionedToAcknowledged));
    }

    public void Dispose()
    {
        lock (stateGate)
        {
            if (isDisposed)
            {
                return;
            }

            isDisposed = true;
            isActive = false;
            isInputGuardActive = false;
            generation++;
            sound.Stop();
            sound.Dispose();
            pendingMouseInputGuard.Dispose();
        }

        _ = TryDispatch(CloseOverlay);
        GC.SuppressFinalize(this);
    }

    private void ShowOverlay(
        string detectedText,
        int triggerGeneration,
        ScreenRegion? anchorRegion)
    {
        lock (stateGate)
        {
            if (isDisposed || !isActive || generation != triggerGeneration)
            {
                return;
            }
        }

        try
        {
            overlay ??= CreateOverlay();
            overlay.Arm(detectedText, triggerGeneration, anchorRegion);
            overlay.Present();
        }
        catch (InvalidOperationException)
        {
            // A WPF window can no longer be shown after it has closed. Recreate it once before
            // treating presentation as failed.
            try
            {
                overlay = CreateOverlay();
                overlay.Arm(detectedText, triggerGeneration, anchorRegion);
                overlay.Present();
            }
            catch (InvalidOperationException)
            {
                CloseOverlay();
                CancelTrigger(triggerGeneration);
            }
        }
    }

    private void PrepareOverlay()
    {
        lock (stateGate)
        {
            if (isDisposed || isActive || isInputGuardActive)
            {
                return;
            }
        }

        try
        {
            overlay ??= CreateOverlay();
            overlay.PrepareForReuse();
        }
        catch (InvalidOperationException)
        {
            CloseOverlay();
            overlay = CreateOverlay();
            overlay.PrepareForReuse();
        }
    }

    private void CancelTrigger(int triggerGeneration)
    {
        lock (stateGate)
        {
            if (!isDisposed && isActive && generation == triggerGeneration)
            {
                isActive = false;
                generation++;
                sound.Stop();
                pendingMouseInputGuard.Release();
            }
        }
    }

    private async Task FailOpenInputGuardAsync(int guardGeneration)
    {
        await Task.Delay(InputGuardFailOpenTimeout).ConfigureAwait(false);
        CancelInputGuard(guardGeneration);
    }

    private async Task FailOpenAlertHandoffAsync(int triggerGeneration)
    {
        await Task.Delay(AlertHandoffFailOpenTimeout).ConfigureAwait(false);
        lock (stateGate)
        {
            if (isDisposed || !isActive || generation != triggerGeneration)
            {
                return;
            }

            // This is harmless if AlertPresented already transferred ownership. If presentation
            // is still queued or stuck, it prevents an indefinite system-wide mouse lock.
            pendingMouseInputGuard.Release();
        }
    }

    private void CancelInputGuard(int guardGeneration)
    {
        int releaseGeneration;
        lock (stateGate)
        {
            if (isDisposed || isActive || !isInputGuardActive || generation != guardGeneration)
            {
                return;
            }

            isInputGuardActive = false;
            releaseGeneration = ++generation;
            pendingMouseInputGuard.Release();
        }

        _ = TryDispatch(() => CompleteInputGuardRelease(releaseGeneration));
    }

    private void CompleteInputGuardRelease(int releaseGeneration)
    {
        lock (stateGate)
        {
            if (isDisposed || isActive || isInputGuardActive || generation != releaseGeneration)
            {
                return;
            }
        }

        HideOverlay();
    }

    private void CloseOverlay()
    {
        if (overlay is null)
        {
            return;
        }

        var window = overlay;
        try
        {
            window.CloseFromService();
        }
        finally
        {
            if (ReferenceEquals(overlay, window))
            {
                overlay = null;
            }
        }
    }

    private void HideOverlay()
    {
        overlay?.ReleaseForReuse();
    }

    private void CompleteAcknowledgement(
        int acknowledgementGeneration,
        bool raiseAcknowledged)
    {
        lock (stateGate)
        {
            if (isDisposed || isActive || generation != acknowledgementGeneration)
            {
                return;
            }
        }

        HideOverlay();
        if (raiseAcknowledged)
        {
            Acknowledged?.Invoke(this, EventArgs.Empty);
        }
    }

    private AffixHitOverlayWindow CreateOverlay()
    {
        var window = new AffixHitOverlayWindow(excludeOverlayFromCapture);
        window.AcknowledgeRequested += OnOverlayAcknowledgementRequested;
        window.AlertPresented += OnOverlayAlertPresented;
        return window;
    }

    private void OnOverlayAlertPresented(object? sender, OverlayPresentedEventArgs e)
    {
        lock (stateGate)
        {
            if (!ReferenceEquals(sender, overlay) || isDisposed || !isActive ||
                generation != e.Generation)
            {
                return;
            }

            // AffixHitOverlayWindow raises this only after its native topmost HWND is visible and
            // hit-testable. Hand input ownership to that window before doing audible work.
            pendingMouseInputGuard.TransferToBlockingOverlay();
            sound.Start();
        }
    }

    private void OnOverlayAcknowledgementRequested(
        object? sender,
        OverlayAcknowledgementRequestedEventArgs e)
    {
        if (!ReferenceEquals(sender, overlay))
        {
            return;
        }

        Acknowledge(e.Generation);
    }

    private void Acknowledge(int expectedGeneration)
    {
        int acknowledgementGeneration;
        lock (stateGate)
        {
            if (isDisposed || generation != expectedGeneration ||
                (!isActive && !isInputGuardActive))
            {
                return;
            }

            var wasAlertActive = isActive;
            isActive = false;
            isInputGuardActive = false;
            generation++;
            sound.Stop();
            acknowledgementGeneration = generation;

            if (!wasAlertActive)
            {
                pendingMouseInputGuard.Release();
                _ = TryDispatch(() => CompleteInputGuardRelease(acknowledgementGeneration));
                return;
            }

            pendingMouseInputGuard.TransferToBlockingOverlay();
        }

        _ = TryDispatch(() => CompleteAcknowledgement(
            acknowledgementGeneration,
            raiseAcknowledged: true));
    }

    private bool TryDispatch(Action action)
    {
        if (dispatcher.HasShutdownStarted || dispatcher.HasShutdownFinished)
        {
            return false;
        }

        try
        {
            if (dispatcher.CheckAccess())
            {
                action();
            }
            else
            {
                _ = dispatcher.BeginInvoke(DispatcherPriority.Send, action);
            }

            return true;
        }
        catch (InvalidOperationException)
        {
            return false;
        }
        catch (TaskCanceledException)
        {
            return false;
        }
    }
}
