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
    private bool isGuardedAlertActive;
    private bool isGuardedOverlayPresented;
    private bool isGuardedRedHandoff;
    private bool isGuardedFailurePending;
    private bool guardedPassCycleReady;
    private TimeSpan guardedDecisionWatchdogTimeout;
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
        pendingMouseInputGuard.CausativeClickCompleted += OnCausativeClickCompleted;
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

    public bool IsGuardedAlertActive
    {
        get
        {
            lock (stateGate)
            {
                return isGuardedAlertActive || isGuardedRedHandoff || isGuardedFailurePending;
            }
        }
    }

    public event EventHandler? Acknowledged;

    public event EventHandler? GuardedContinueRequested;

    public event EventHandler? GuardedStopRequested;

    public event EventHandler<GuardedAlertFailOpenEventArgs>? GuardedFailOpen;

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
            if (isDisposed || isActive || isGuardedAlertActive || isGuardedFailurePending)
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

    public void BeginGuardedInputGuard(TimeSpan watchdogTimeout, ScreenRegion? anchorRegion = null)
    {
        if (watchdogTimeout < TimeSpan.FromMilliseconds(50) ||
            watchdogTimeout > TimeSpan.FromSeconds(30))
        {
            throw new ArgumentOutOfRangeException(
                nameof(watchdogTimeout),
                "The Guarded input watchdog must be between 50 ms and 30 seconds.");
        }

        int guardGeneration;
        var armFailed = false;
        var refreshOnly = false;
        lock (stateGate)
        {
            if (isDisposed || isActive || isGuardedFailurePending)
            {
                return;
            }

            if (isGuardedAlertActive)
            {
                if (isGuardedOverlayPresented)
                {
                    return;
                }

                if (guardedPassCycleReady)
                {
                    // A changed frame can beat the low-level completion callback on another
                    // thread. Arm is idempotent for the pass cycle and makes this decision the
                    // new owner without allowing a second click through.
                    if (!pendingMouseInputGuard.Arm())
                    {
                        isGuardedFailurePending = true;
                        guardedPassCycleReady = false;
                        guardGeneration = ++generation;
                        armFailed = true;
                    }
                    else
                    {
                        guardedPassCycleReady = false;
                        guardGeneration = ++generation;
                        guardedDecisionWatchdogTimeout = watchdogTimeout;
                        refreshOnly = true;
                    }

                    goto GuardStateResolved;
                }

                // Progressive/stability slices refresh the Guarded watchdog without releasing
                // and re-arming the already held native gate. The hook therefore remains a
                // continuous owner from the first changed frame through the final decision.
                guardGeneration = ++generation;
                refreshOnly = true;
            }
            else
            {
                isInputGuardActive = false;
                isGuardedAlertActive = true;
                isGuardedOverlayPresented = false;
                guardGeneration = ++generation;
                if (!pendingMouseInputGuard.Arm())
                {
                    isGuardedAlertActive = false;
                    generation++;
                    armFailed = true;
                }
                else
                {
                    guardedDecisionWatchdogTimeout = watchdogTimeout;
                }
            }

        GuardStateResolved:;
        }

        _ = anchorRegion;
        if (refreshOnly)
        {
            _ = FailOpenGuardedInputGuardAsync(guardGeneration, watchdogTimeout);
            return;
        }

        if (armFailed)
        {
            var args = new GuardedAlertFailOpenEventArgs(
                GuardedAlertFailOpenReason.InputGuardUnavailable,
                "The Guarded mouse input hook could not be armed.");
            RaiseGuardedFailOpenSafely(args);
            CompleteGuardedFailOpen(guardGeneration);
            return;
        }

        _ = FailOpenGuardedInputGuardAsync(guardGeneration, watchdogTimeout);
    }

    public void ReleaseInputGuard()
    {
        int releaseGeneration;
        var logicalGuardWasActive = false;
        lock (stateGate)
        {
            if (isDisposed || isActive || isGuardedAlertActive || isGuardedFailurePending)
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

    public void ReleaseGuardedInputGuard()
    {
        int releaseGeneration;
        lock (stateGate)
        {
            if (isDisposed ||
                (!isGuardedAlertActive && !isGuardedRedHandoff && !isGuardedFailurePending))
            {
                return;
            }

            isGuardedAlertActive = false;
            isGuardedOverlayPresented = false;
            isGuardedRedHandoff = false;
            isGuardedFailurePending = false;
            guardedPassCycleReady = false;
            releaseGeneration = ++generation;
            pendingMouseInputGuard.Release();
        }

        _ = TryDispatch(() => CompleteGuardedRelease(releaseGeneration));
    }

    public void ShowGuardedUncertain(
        string title,
        string detail,
        string instruction,
        string continueButtonText,
        string stopButtonText,
        ScreenRegion? anchorRegion = null)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(title);
        ArgumentException.ThrowIfNullOrWhiteSpace(detail);
        ArgumentException.ThrowIfNullOrWhiteSpace(instruction);
        ArgumentException.ThrowIfNullOrWhiteSpace(continueButtonText);
        ArgumentException.ThrowIfNullOrWhiteSpace(stopButtonText);

        int guardedGeneration;
        lock (stateGate)
        {
            if (isDisposed || isActive || !isGuardedAlertActive || isGuardedOverlayPresented)
            {
                return;
            }

            guardedGeneration = generation;
        }

        if (!TryDispatch(() => ShowGuardedOverlay(
                title.Trim(),
                detail.Trim(),
                instruction.Trim(),
                continueButtonText.Trim(),
                stopButtonText.Trim(),
                guardedGeneration,
                anchorRegion)))
        {
            FailOpenGuarded(
                guardedGeneration,
                GuardedAlertFailOpenReason.DispatcherUnavailable,
                "The UI dispatcher was unavailable before the Guarded overlay could be shown.");
            return;
        }

        _ = FailOpenGuardedPresentationAsync(guardedGeneration);
    }

    public bool AwaitNextGuardedClick(
        TimeSpan decisionWatchdogTimeout,
        ScreenRegion? anchorRegion = null)
    {
        if (decisionWatchdogTimeout < TimeSpan.FromMilliseconds(50) ||
            decisionWatchdogTimeout > TimeSpan.FromSeconds(30))
        {
            throw new ArgumentOutOfRangeException(
                nameof(decisionWatchdogTimeout),
                "The Guarded decision watchdog must be between 50 ms and 30 seconds.");
        }

        int failureGeneration = 0;
        lock (stateGate)
        {
            if (isDisposed || isActive || !isGuardedAlertActive || isGuardedFailurePending)
            {
                return false;
            }

            if (guardedPassCycleReady)
            {
                // Continue's overlay hand-off and the monitoring policy can request the same
                // pass cycle synchronously. Never re-enter the native state machine: a very fast
                // user Down between those calls must still be the one and only allowed click.
                return true;
            }

            if (!pendingMouseInputGuard.AwaitNextCausativeClick())
            {
                isGuardedFailurePending = true;
                failureGeneration = ++generation;
            }
            else
            {
                guardedPassCycleReady = true;
                guardedDecisionWatchdogTimeout = decisionWatchdogTimeout;
                // Invalidate the recognition/presentation watchdog. Waiting for the user's next
                // click is intentionally unbounded; a new decision deadline starts on completion.
                generation++;
            }
        }

        _ = anchorRegion;
        if (failureGeneration == 0)
        {
            return true;
        }

        RaiseGuardedFailOpenSafely(
            new GuardedAlertFailOpenEventArgs(
                GuardedAlertFailOpenReason.InputGuardUnavailable,
                "The Guarded mouse input hook could not await the next causative click."));
        CompleteGuardedFailOpen(failureGeneration);
        return false;
    }

    public void Trigger(string? detectedText = null, ScreenRegion? anchorRegion = null)
    {
        int triggerGeneration;

        lock (stateGate)
        {
            if (isDisposed || isActive || isGuardedAlertActive || isGuardedFailurePending)
            {
                return;
            }

            isActive = true;
            isInputGuardActive = false;
            isGuardedRedHandoff = false;
            triggerGeneration = ++generation;
        }

        BeginRedPresentation(detectedText, anchorRegion, triggerGeneration, guardedMatch: false);
    }

    public bool TriggerGuardedMatch(
        string? detectedText = null,
        ScreenRegion? anchorRegion = null)
    {
        int triggerGeneration;
        lock (stateGate)
        {
            if (isDisposed || isActive || !isGuardedAlertActive ||
                isGuardedOverlayPresented || isGuardedFailurePending)
            {
                return false;
            }

            // This lock is the single ownership linearization point shared with every Guarded
            // watchdog. Either Match atomically reclassifies the existing native gate as the red
            // hand-off, or a watchdog has already made failure pending and this call refuses it.
            isActive = true;
            isInputGuardActive = false;
            isGuardedAlertActive = false;
            isGuardedOverlayPresented = false;
            isGuardedRedHandoff = true;
            guardedPassCycleReady = false;
            triggerGeneration = ++generation;
        }

        return BeginRedPresentation(
            detectedText,
            anchorRegion,
            triggerGeneration,
            guardedMatch: true);
    }

    private bool BeginRedPresentation(
        string? detectedText,
        ScreenRegion? anchorRegion,
        int triggerGeneration,
        bool guardedMatch)
    {

        var displayText = string.IsNullOrWhiteSpace(detectedText)
            ? UiText.Current.DefaultDetectedText
            : detectedText.Trim();

        // Queue or present the input shield before starting any non-critical alert work. Sound is
        // deliberately started by ShowOverlay only after the native shield is visible.
        if (!TryDispatch(() => ShowOverlay(
                displayText,
                triggerGeneration,
                anchorRegion,
                guardedMatch)))
        {
            if (guardedMatch)
            {
                FailOpenGuardedRedHandoff(
                    triggerGeneration,
                    GuardedAlertFailOpenReason.DispatcherUnavailable,
                    "The UI dispatcher was unavailable before the Guarded match alert could be shown.");
            }
            else
            {
                CancelTrigger(triggerGeneration);
            }

            return false;
        }

        // A queued dispatcher presentation must not leave the system-wide low-level hook armed
        // forever if the UI thread stalls. The red window may still appear later, but the hook
        // independently fails open after this bounded hand-off interval.
        _ = FailOpenAlertHandoffAsync(triggerGeneration, guardedMatch);
        return true;
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

            if (isActive || isInputGuardActive || isGuardedAlertActive)
            {
                var wasAlertActive = isActive;
                isActive = false;
                isInputGuardActive = false;
                isGuardedAlertActive = false;
                isGuardedOverlayPresented = false;
                isGuardedRedHandoff = false;
                isGuardedFailurePending = false;
                guardedPassCycleReady = false;
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
            isGuardedAlertActive = false;
            isGuardedOverlayPresented = false;
            isGuardedRedHandoff = false;
            isGuardedFailurePending = false;
            guardedPassCycleReady = false;
            generation++;
            pendingMouseInputGuard.CausativeClickCompleted -= OnCausativeClickCompleted;
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
        ScreenRegion? anchorRegion,
        bool guardedMatch)
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
            catch (Exception exception)
            {
                CloseOverlay();
                HandleRedPresentationFailure(
                    triggerGeneration,
                    guardedMatch,
                    exception);
            }
        }
        catch (Exception exception)
        {
            CloseOverlay();
            HandleRedPresentationFailure(triggerGeneration, guardedMatch, exception);
        }
    }

    private void HandleRedPresentationFailure(
        int triggerGeneration,
        bool guardedMatch,
        Exception exception)
    {
        if (guardedMatch)
        {
            FailOpenGuardedRedHandoff(
                triggerGeneration,
                GuardedAlertFailOpenReason.PresentationFailed,
                $"The Guarded match alert could not be presented: {exception.Message}");
            return;
        }

        CancelTrigger(triggerGeneration);
    }

    private void ShowGuardedOverlay(
        string title,
        string detail,
        string instruction,
        string continueButtonText,
        string stopButtonText,
        int guardedGeneration,
        ScreenRegion? anchorRegion)
    {
        lock (stateGate)
        {
            if (isDisposed || !isGuardedAlertActive || isGuardedOverlayPresented ||
                generation != guardedGeneration)
            {
                return;
            }
        }

        try
        {
            overlay ??= CreateOverlay();
            overlay.ArmGuarded(
                title,
                detail,
                instruction,
                continueButtonText,
                stopButtonText,
                guardedGeneration,
                anchorRegion);
            overlay.Present();
        }
        catch (Exception exception)
        {
            try
            {
                overlay?.ReleaseForReuse();
            }
            finally
            {
                FailOpenGuarded(
                    guardedGeneration,
                    GuardedAlertFailOpenReason.PresentationFailed,
                    $"The Guarded overlay could not be presented: {exception.Message}");
            }
        }
    }

    private void PrepareOverlay()
    {
        lock (stateGate)
        {
            if (isDisposed || isActive || isInputGuardActive || isGuardedAlertActive)
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

    private async Task FailOpenGuardedInputGuardAsync(
        int guardGeneration,
        TimeSpan watchdogTimeout)
    {
        await Task.Delay(watchdogTimeout).ConfigureAwait(false);
        FailOpenGuarded(
            guardGeneration,
            GuardedAlertFailOpenReason.InputGuardTimedOut,
            "The Guarded decision watchdog expired before a conclusive decision.");
    }

    private async Task FailOpenGuardedPresentationAsync(int guardedGeneration)
    {
        await Task.Delay(AlertHandoffFailOpenTimeout).ConfigureAwait(false);
        FailOpenGuarded(
            guardedGeneration,
            GuardedAlertFailOpenReason.PresentationTimedOut,
            "The UI dispatcher did not present the Guarded overlay before its hand-off deadline.");
    }

    private async Task FailOpenAlertHandoffAsync(int triggerGeneration, bool guardedMatch)
    {
        await Task.Delay(AlertHandoffFailOpenTimeout).ConfigureAwait(false);
        if (guardedMatch)
        {
            FailOpenGuardedRedHandoff(
                triggerGeneration,
                GuardedAlertFailOpenReason.PresentationTimedOut,
                "The UI dispatcher did not present the Guarded match alert before its hand-off deadline.");
            return;
        }

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

    private void CompleteGuardedRelease(int releaseGeneration)
    {
        lock (stateGate)
        {
            if (isDisposed || isGuardedAlertActive || generation != releaseGeneration)
            {
                return;
            }
        }

        HideOverlay();
    }

    private void FailOpenGuarded(
        int guardedGeneration,
        GuardedAlertFailOpenReason reason,
        string detail)
    {
        int failureGeneration;
        lock (stateGate)
        {
            if (isDisposed || !isGuardedAlertActive || isGuardedOverlayPresented ||
                isGuardedFailurePending || generation != guardedGeneration)
            {
                return;
            }

            // Keep the native gate continuously armed while the terminal fault is delivered.
            // TriggerGuardedMatch shares this lock and must now refuse ownership, while the
            // synchronous subscriber gets the first opportunity to stop the monitor and release.
            isGuardedFailurePending = true;
            failureGeneration = ++generation;
        }

        RaiseGuardedFailOpenSafely(new GuardedAlertFailOpenEventArgs(reason, detail));
        CompleteGuardedFailOpen(failureGeneration);
    }

    private void FailOpenGuardedRedHandoff(
        int triggerGeneration,
        GuardedAlertFailOpenReason reason,
        string detail)
    {
        int failureGeneration;
        lock (stateGate)
        {
            if (isDisposed || !isActive || !isGuardedRedHandoff ||
                isGuardedFailurePending || generation != triggerGeneration)
            {
                return;
            }

            // The old Guarded watchdog was invalidated when the match incremented generation.
            // This new red-handoff watchdog owns the same still-armed hook and marks failure
            // before notifying, so no late Trigger or ordinary release can create an input gap.
            isGuardedFailurePending = true;
            failureGeneration = ++generation;
        }

        RaiseGuardedFailOpenSafely(new GuardedAlertFailOpenEventArgs(reason, detail));
        CompleteGuardedFailOpen(failureGeneration);
    }

    private void CompleteGuardedFailOpen(int failureGeneration)
    {
        lock (stateGate)
        {
            if (!isDisposed && isGuardedFailurePending && generation == failureGeneration)
            {
                isActive = false;
                isInputGuardActive = false;
                isGuardedAlertActive = false;
                isGuardedOverlayPresented = false;
                isGuardedRedHandoff = false;
                isGuardedFailurePending = false;
                generation++;
                sound.Stop();
                pendingMouseInputGuard.Release();
            }
        }

        // This can run on an independent watchdog while WPF is stalled. It is an idempotent
        // native fallback after subscribers have had a synchronous chance to release normally.
        overlay?.FailOpenNativeInputShield();
        _ = TryDispatch(HideOverlay);
    }

    private void RaiseGuardedFailOpenSafely(GuardedAlertFailOpenEventArgs args)
    {
        var handlers = GuardedFailOpen;
        if (handlers is null)
        {
            return;
        }

        foreach (EventHandler<GuardedAlertFailOpenEventArgs> handler in
                 handlers.GetInvocationList())
        {
            try
            {
                handler(this, args);
            }
            catch
            {
                // Protection has already failed open. A subscriber must not crash the watchdog
                // thread or prevent other owners from receiving the terminal fault.
            }
        }
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
        window.GuardedActionRequested += OnOverlayGuardedActionRequested;
        return window;
    }

    private void OnOverlayAlertPresented(object? sender, OverlayPresentedEventArgs e)
    {
        lock (stateGate)
        {
            if (!ReferenceEquals(sender, overlay) || isDisposed || generation != e.Generation)
            {
                return;
            }

            if (isGuardedAlertActive)
            {
                // This event is synchronous with native visibility. The full-desktop yellow HWND
                // now owns all input, so the system-wide hook can be released without a gap.
                isGuardedOverlayPresented = true;
                pendingMouseInputGuard.TransferToBlockingOverlay();
                return;
            }

            if (!isActive)
            {
                return;
            }

            // AffixHitOverlayWindow raises this only after its native topmost HWND is visible and
            // hit-testable. Hand input ownership to that window before doing audible work.
            isGuardedRedHandoff = false;
            pendingMouseInputGuard.TransferToBlockingOverlay();
            sound.Start();
        }
    }

    private void OnCausativeClickCompleted(object? sender, EventArgs e)
    {
        int watchdogGeneration;
        TimeSpan watchdogTimeout;
        lock (stateGate)
        {
            if (isDisposed || isActive || !isGuardedAlertActive || isGuardedOverlayPresented ||
                isGuardedFailurePending || !guardedPassCycleReady ||
                pendingMouseInputGuard.Mode != MouseInputGuardMode.Guarding)
            {
                return;
            }

            // The low-level CAS has already changed to Guarding before raising this event. From
            // this instant the decision has a bounded lifetime even if the allowed click failed
            // to change the tooltip and the monitor never observes a new fingerprint.
            guardedPassCycleReady = false;
            watchdogGeneration = ++generation;
            watchdogTimeout = guardedDecisionWatchdogTimeout;
        }

        _ = FailOpenGuardedInputGuardAsync(watchdogGeneration, watchdogTimeout);
    }

    private void OnOverlayGuardedActionRequested(
        object? sender,
        GuardedOverlayActionRequestedEventArgs e)
    {
        EventHandler? callback;
        GuardedAlertFailOpenEventArgs? failure = null;
        int failureGeneration = 0;
        lock (stateGate)
        {
            if (!ReferenceEquals(sender, overlay) || isDisposed || !isGuardedAlertActive ||
                !isGuardedOverlayPresented || generation != e.Generation)
            {
                return;
            }

            if (e.Action == GuardedOverlayAction.ContinueMonitoring)
            {
                // The full-desktop overlay still owns input here. Install the one-click native
                // gate before removing that HWND, so Continue never exposes an unguarded gap.
                if (!pendingMouseInputGuard.AwaitNextCausativeClick())
                {
                    isGuardedFailurePending = true;
                    failureGeneration = ++generation;
                    callback = null;
                    failure = new GuardedAlertFailOpenEventArgs(
                        GuardedAlertFailOpenReason.InputGuardUnavailable,
                        "The Guarded mouse input hook could not resume after manual review.");
                }
                else
                {
                    guardedPassCycleReady = true;
                    isGuardedOverlayPresented = false;
                    generation++;
                    callback = GuardedContinueRequested;
                }
            }
            else
            {
                isGuardedAlertActive = false;
                isGuardedOverlayPresented = false;
                guardedPassCycleReady = false;
                generation++;
                pendingMouseInputGuard.Release();
                callback = GuardedStopRequested;
            }
        }

        // The replacement native owner is now active (Continue), or terminal input release has
        // begun (Stop). It is safe to remove the yellow HWND before invoking application code.
        overlay?.ReleaseForReuse();
        if (failure is not null)
        {
            RaiseGuardedFailOpenSafely(failure);
            CompleteGuardedFailOpen(failureGeneration);
            return;
        }

        callback?.Invoke(this, EventArgs.Empty);
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
