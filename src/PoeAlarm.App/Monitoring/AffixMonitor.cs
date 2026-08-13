using System.Diagnostics;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Monitoring.Policies;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;
using PoeAlarm.Core.Rules;

namespace PoeAlarm.App.Monitoring;

public sealed class AffixMonitor : IAsyncDisposable
{
    private readonly IScreenCapture _screenCapture;
    private readonly IOcrRecognizer _ocrRecognizer;
    private readonly object _stateGate = new();
    private CancellationTokenSource? _loopCancellation;
    private Task? _loopTask;
    private MonitorState _state = MonitorState.Idle;
    private bool _stopInProgress;
    private GuardedPolicyStateMachine? _guardedStateMachine;
    private TaskCompletionSource _guardedResumeSignal = CreateGuardedResumeSignal();
    private long _guardedCausativeClickCompletedUtcTicks;
    private int _guardedCausativeClickStarted;
    private Task? _disposeTask;
    private bool _disposed;

    public AffixMonitor(IScreenCapture screenCapture, IOcrRecognizer ocrRecognizer)
    {
        _screenCapture = screenCapture;
        _ocrRecognizer = ocrRecognizer;
    }

    public event EventHandler<MonitorSnapshot>? SnapshotChanged;

    public event EventHandler<AffixDetectedEventArgs>? AffixDetected;

    /// <summary>
    /// Raised for terminal Guarded decisions. Match also raises <see cref="AffixDetected"/> so
    /// the existing red latched alert remains the authoritative match owner.
    /// </summary>
    public event EventHandler<GuardedDecisionEventArgs>? GuardedDecisionChanged;

    /// <summary>
    /// Raised before OCR starts on a changed frame for recognizers that expose a semantic
    /// fingerprint. The UI uses this to install a short, invisible input guard while that frame
    /// is being decided.
    /// </summary>
    public event EventHandler? InputGuardRequested;

    /// <summary>Raised when a guarded changed frame completes without a target match.</summary>
    public event EventHandler? InputGuardReleased;

    /// <summary>
    /// Raised after a conclusive Guarded miss or an explicit human Continue. The native gate must
    /// remain installed, pass exactly the next complete crafting click, and guard every click
    /// after that pair until the resulting tooltip has been decided.
    /// </summary>
    public event EventHandler? GuardedPassCycleRequested;

    public MonitorState State
    {
        get
        {
            lock (_stateGate)
            {
                return _state;
            }
        }
    }

    public void Start(
        string template,
        ScreenRegion region,
        int maximumPhysicalLineSpan = FullLineAffixMatcher.MaximumPhysicalLineSpan)
    {
        if (!region.IsValid)
        {
            throw new ArgumentOutOfRangeException(nameof(region), "Select a valid tooltip region before monitoring.");
        }

        var matcher = new FullLineAffixMatcher(template, maximumPhysicalLineSpan);

        lock (_stateGate)
        {
            EnsureCanStart();
            _loopCancellation?.Dispose();
            _loopCancellation = new CancellationTokenSource();
            _guardedStateMachine = null;
            _state = MonitorState.Running;
            _loopTask = RunLoopAsync(matcher, region, _loopCancellation.Token);
        }
    }

    public GuardedSessionState? GuardedState
    {
        get
        {
            lock (_stateGate)
            {
                return _guardedStateMachine?.State;
            }
        }
    }

    public StructuredRuleOcrSupport StructuredRuleSupport =>
        _ocrRecognizer.StructuredRuleSupport;

    public bool SupportsGuardedMonitoring =>
        _ocrRecognizer.StructuredRuleSupport != StructuredRuleOcrSupport.Unsupported &&
        _ocrRecognizer is IFrameFingerprintProvider;

    /// <summary>
    /// Starts the structured-rule path. The legacy string overload above remains unchanged and
    /// keeps its target-aware 0.6.1 behavior. Structured rules perform one batched OCR call per
    /// frame and then evaluate all conditions in memory; they never multiply OCR work by the
    /// number of conditions. Recognizers must explicitly opt in after preserving their verified
    /// safety contract; an unsupported recognizer is rejected instead of silently degrading.
    /// </summary>
    public void Start(CompiledRuleSet rules, ScreenRegion region)
    {
        ArgumentNullException.ThrowIfNull(rules);
        if (_ocrRecognizer.StructuredRuleSupport == StructuredRuleOcrSupport.Unsupported)
        {
            throw new NotSupportedException(
                $"{_ocrRecognizer.GetType().Name} has not passed structured-rule batch OCR validation.");
        }

        if (!region.IsValid)
        {
            throw new ArgumentOutOfRangeException(nameof(region), "Select a valid tooltip region before monitoring.");
        }

        lock (_stateGate)
        {
            EnsureCanStart();
            _loopCancellation?.Dispose();
            _loopCancellation = new CancellationTokenSource();
            _guardedStateMachine = null;
            _state = MonitorState.Running;
            _loopTask = RunRuleLoopAsync(rules, region, _loopCancellation.Token);
        }
    }

    /// <summary>
    /// Starts structured monitoring under an explicit input policy. Passing Fast is identical to
    /// the existing structured overload; Guarded uses its own state machine and loop.
    /// </summary>
    public void Start(
        CompiledRuleSet rules,
        ScreenRegion region,
        IMonitoringPolicy policy)
    {
        ArgumentNullException.ThrowIfNull(policy);
        if (policy is FastMonitoringPolicy || policy.Kind == MonitoringPolicyKind.Fast)
        {
            Start(rules, region);
            return;
        }

        if (policy is not GuardedMonitoringPolicy guardedPolicy ||
            policy.Kind != MonitoringPolicyKind.Guarded)
        {
            throw new ArgumentException("Unsupported monitoring policy.", nameof(policy));
        }

        ArgumentNullException.ThrowIfNull(rules);
        if (_ocrRecognizer.StructuredRuleSupport == StructuredRuleOcrSupport.Unsupported)
        {
            throw new NotSupportedException(
                $"{_ocrRecognizer.GetType().Name} has not passed structured-rule batch OCR validation.");
        }

        if (_ocrRecognizer is not IFrameFingerprintProvider)
        {
            throw new NotSupportedException(
                $"{_ocrRecognizer.GetType().Name} does not expose the semantic fingerprint required by Guarded monitoring.");
        }

        if (!region.IsValid)
        {
            throw new ArgumentOutOfRangeException(nameof(region), "Select a valid tooltip region before monitoring.");
        }

        lock (_stateGate)
        {
            EnsureCanStart();
            _loopCancellation?.Dispose();
            _loopCancellation = new CancellationTokenSource();
            _guardedStateMachine = new GuardedPolicyStateMachine(guardedPolicy.Options);
            _guardedResumeSignal = CreateGuardedResumeSignal();
            Interlocked.Exchange(ref _guardedCausativeClickCompletedUtcTicks, 0);
            Interlocked.Exchange(ref _guardedCausativeClickStarted, 0);
            _state = MonitorState.Running;
            _loopTask = RunGuardedRuleLoopAsync(
                rules,
                region,
                _guardedStateMachine,
                _loopCancellation.Token);
        }
    }

    /// <summary>
    /// Delivers the native gate's proof that one complete crafting click reached the game. Frame
    /// changes are ignored until this signal, so overlay/background changes cannot self-trigger
    /// a Guarded decision.
    /// </summary>
    public bool NotifyGuardedCausativeClickCompleted()
    {
        lock (_stateGate)
        {
            var guardedState = _guardedStateMachine?.State;
            if (_disposed ||
                guardedState is not GuardedSessionState.WaitingForChange and
                    not GuardedSessionState.PausedUncertain ||
                _state != MonitorState.Running)
            {
                return false;
            }
        }

        Interlocked.Exchange(
            ref _guardedCausativeClickCompletedUtcTicks,
            DateTimeOffset.UtcNow.UtcDateTime.Ticks);
        return true;
    }

    public bool NotifyGuardedCausativeClickStarted()
    {
        lock (_stateGate)
        {
            var guardedState = _guardedStateMachine?.State;
            if (_disposed ||
                guardedState is not GuardedSessionState.WaitingForChange and
                    not GuardedSessionState.PausedUncertain ||
                _state != MonitorState.Running)
            {
                return false;
            }
        }

        Interlocked.Exchange(ref _guardedCausativeClickStarted, 1);
        return true;
    }

    /// <summary>
    /// Accepts the currently visible uncertain roll and resumes Guarded monitoring. Input is
    /// released without replay; the next physical click is a fresh user action.
    /// </summary>
    public bool ContinueGuarded()
    {
        GuardedFrameTransition transition;
        TaskCompletionSource resumeSignal;
        lock (_stateGate)
        {
            if (_disposed || _guardedStateMachine is null ||
                _guardedStateMachine.State != GuardedSessionState.PausedUncertain)
            {
                return false;
            }

            transition = _guardedStateMachine.ContinueAfterUncertain();
            resumeSignal = _guardedResumeSignal;
            _guardedResumeSignal = CreateGuardedResumeSignal();
        }

        ApplyGuardTransition(transition);
        resumeSignal.TrySetResult();
        return true;
    }

    /// <summary>
    /// Fails open when the native input guard or its visible hand-off reports that protection is
    /// no longer available. The Guarded session is cancelled and may not silently continue.
    /// </summary>
    public bool FailGuardedSession(
        string detail,
        GuardedFaultReason reason = GuardedFaultReason.InputGuardUnavailable)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(detail);
        GuardedFrameTransition transition;
        CancellationTokenSource? cancellation;
        TaskCompletionSource resumeSignal;
        lock (_stateGate)
        {
            if (_disposed || _guardedStateMachine is null ||
                _guardedStateMachine.State is GuardedSessionState.Stopped or
                    GuardedSessionState.Faulted)
            {
                return false;
            }

            transition = _guardedStateMachine.Fault(detail.Trim(), reason);
            cancellation = _loopCancellation;
            resumeSignal = _guardedResumeSignal;
            _guardedResumeSignal = CreateGuardedResumeSignal();
            _state = MonitorState.Faulted;
        }

        cancellation?.Cancel();
        resumeSignal.TrySetResult();
        ApplyGuardTransition(transition);
        RaiseSnapshot(new MonitorSnapshot(
            MonitorState.Faulted,
            0,
            TimeSpan.Zero,
            TimeSpan.Zero,
            [],
            $"Guarded {reason}: {detail.Trim()}"));
        return true;
    }

    public async Task StopAsync()
    {
        Task? loopTask;
        var ownsStop = false;
        GuardedFrameTransition? guardedStop = null;
        TaskCompletionSource? guardedResume = null;
        lock (_stateGate)
        {
            if (_disposed)
            {
                return;
            }

            if (!_stopInProgress)
            {
                _stopInProgress = true;
                ownsStop = true;
                _loopCancellation?.Cancel();
                if (_guardedStateMachine is not null)
                {
                    guardedStop = _guardedStateMachine.Stop();
                    guardedResume = _guardedResumeSignal;
                    _guardedResumeSignal = CreateGuardedResumeSignal();
                }
            }

            loopTask = _loopTask;
        }

        if (guardedStop is { } stopTransition)
        {
            ApplyGuardTransition(stopTransition);
            guardedResume?.TrySetResult();
        }

        try
        {
            if (loopTask is not null)
            {
                try
                {
                    await loopTask.ConfigureAwait(false);
                }
                catch (OperationCanceledException)
                {
                    // A user stop is an expected state transition.
                }
            }
        }
        finally
        {
            if (ownsStop)
            {
                lock (_stateGate)
                {
                    if (ReferenceEquals(_loopTask, loopTask))
                    {
                        _state = MonitorState.Idle;
                    }

                    _stopInProgress = false;
                }
            }
        }

        if (ownsStop)
        {
            RaiseSnapshot(new MonitorSnapshot(MonitorState.Idle, 0, TimeSpan.Zero, TimeSpan.Zero, []));
        }
    }

    private async Task RunLoopAsync(
        FullLineAffixMatcher matcher,
        ScreenRegion region,
        CancellationToken cancellationToken)
    {
        long scanCount = 0;
        var fingerprintProvider = _ocrRecognizer as IFrameFingerprintProvider;
        ulong? acceptedFingerprint = null;
        var inputGuardHeld = false;

        try
        {
            while (!cancellationToken.IsCancellationRequested)
            {
                var captureWatch = Stopwatch.StartNew();
                var frame = _screenCapture.Capture(region);

                ulong? frameFingerprint = null;
                if (fingerprintProvider is not null)
                {
                    frameFingerprint = fingerprintProvider.ComputeFrameFingerprint(frame);
                    if (!acceptedFingerprint.HasValue ||
                        frameFingerprint.Value != acceptedFingerprint.Value)
                    {
                        inputGuardHeld = true;
                        // Re-request on every progressive slice. The alert service treats this as
                        // a watchdog refresh, so one unexpectedly stuck OCR call still fails open.
                        InputGuardRequested?.Invoke(this, EventArgs.Empty);
                    }
                }

                // For frame-aware recognizers this also includes the cheap blue-mask fingerprint
                // pass. OCR itself can then reuse the prepared bands without doing that work twice.
                var captureElapsed = captureWatch.Elapsed;

                var ocrResult = await _ocrRecognizer
                    .RecognizeAsync(frame, matcher, cancellationToken)
                    .ConfigureAwait(false);
                scanCount++;

                var snapshot = new MonitorSnapshot(
                    MonitorState.Running,
                    scanCount,
                    captureElapsed,
                    ocrResult.TotalElapsed,
                    ocrResult.Lines,
                    OcrWasCached: ocrResult.WasCached);

                cancellationToken.ThrowIfCancellationRequested();
                matcher.TryFindMatch(ocrResult.Lines, out var match);
                match ??= IsAssistedMatchForTarget(ocrResult.TargetAssistedMatch, matcher)
                    ? ocrResult.TargetAssistedMatch
                    : null;
                if (match is not null)
                {
                    if (!TrySetMatchFound(cancellationToken))
                    {
                        return;
                    }

                    snapshot = snapshot with
                    {
                        State = MonitorState.MatchFound,
                        Detail = match.OriginalText,
                    };
                    RaiseSnapshot(snapshot);
                    AffixDetected?.Invoke(this, new AffixDetectedEventArgs(match, snapshot));
                    return;
                }

                RaiseSnapshot(snapshot);

                if (!ocrResult.RequiresRescan && frameFingerprint.HasValue)
                {
                    acceptedFingerprint = frameFingerprint.Value;
                    if (inputGuardHeld)
                    {
                        InputGuardReleased?.Invoke(this, EventArgs.Empty);
                        inputGuardHeld = false;
                    }
                }

                // Yield briefly so the UI and game retain priority. OCR time remains the
                // dominant cadence; this does not impose a coarse polling interval.
                if (ocrResult.RequiresRescan)
                {
                    // Task.Delay(1) can become a full Windows timer tick. A cooperative yield is
                    // enough to keep the UI responsive while letting the next transient capture
                    // start without an artificial 10-16 ms blind spot.
                    await Task.Yield();
                    cancellationToken.ThrowIfCancellationRequested();
                }
                else
                {
                    await Task.Delay(ocrResult.WasCached ? 8 : 4, cancellationToken)
                        .ConfigureAwait(false);
                }
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // Expected when the user stops or acknowledges an alert.
        }
        catch (Exception exception)
        {
            SetState(MonitorState.Faulted);
            RaiseSnapshot(new MonitorSnapshot(
                MonitorState.Faulted,
                scanCount,
                TimeSpan.Zero,
                TimeSpan.Zero,
                [],
                exception.Message));
        }
        finally
        {
            // The monitor owns only the short pre-recognition request, so it always relinquishes
            // that request on exit. A subscriber that accepted the match must synchronously put
            // the alert service into its latched state before returning from AffixDetected; the
            // service then ignores this guard-only release until the red alert takes ownership.
            if (inputGuardHeld)
            {
                InputGuardReleased?.Invoke(this, EventArgs.Empty);
            }
        }
    }

    private async Task RunRuleLoopAsync(
        CompiledRuleSet rules,
        ScreenRegion region,
        CancellationToken cancellationToken)
    {
        long scanCount = 0;
        var fingerprintProvider = _ocrRecognizer as IFrameFingerprintProvider;
        ulong? acceptedFingerprint = null;
        var inputGuardHeld = false;

        try
        {
            while (!cancellationToken.IsCancellationRequested)
            {
                var captureWatch = Stopwatch.StartNew();
                var frame = _screenCapture.Capture(region);

                ulong? frameFingerprint = null;
                if (fingerprintProvider is not null)
                {
                    frameFingerprint = fingerprintProvider.ComputeFrameFingerprint(frame);
                    if (!acceptedFingerprint.HasValue ||
                        frameFingerprint.Value != acceptedFingerprint.Value)
                    {
                        inputGuardHeld = true;
                        InputGuardRequested?.Invoke(this, EventArgs.Empty);
                    }
                }

                var captureElapsed = captureWatch.Elapsed;
                var ocrResult = await _ocrRecognizer
                    .RecognizeAsync(frame, rules.Targets, cancellationToken)
                    .ConfigureAwait(false);
                scanCount++;

                cancellationToken.ThrowIfCancellationRequested();
                var evaluation = EvaluateRules(rules, ocrResult);
                var snapshot = new MonitorSnapshot(
                    MonitorState.Running,
                    scanCount,
                    captureElapsed,
                    ocrResult.TotalElapsed,
                    ocrResult.Lines,
                    OcrWasCached: ocrResult.WasCached);

                if (evaluation.IsMatch)
                {
                    if (!TrySetMatchFound(cancellationToken))
                    {
                        return;
                    }

                    var detectedText = CreateRuleMatchDetail(evaluation);
                    snapshot = snapshot with
                    {
                        State = MonitorState.MatchFound,
                        Detail = detectedText,
                    };
                    RaiseSnapshot(snapshot);
                    AffixDetected?.Invoke(
                        this,
                        new AffixDetectedEventArgs(evaluation, detectedText, snapshot));
                    return;
                }

                RaiseSnapshot(snapshot);
                if (!ocrResult.RequiresRescan && frameFingerprint.HasValue)
                {
                    acceptedFingerprint = frameFingerprint.Value;
                    if (inputGuardHeld)
                    {
                        InputGuardReleased?.Invoke(this, EventArgs.Empty);
                        inputGuardHeld = false;
                    }
                }

                if (ocrResult.RequiresRescan)
                {
                    await Task.Yield();
                    cancellationToken.ThrowIfCancellationRequested();
                }
                else
                {
                    await Task.Delay(ocrResult.WasCached ? 8 : 4, cancellationToken)
                        .ConfigureAwait(false);
                }
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // Expected when the user stops or acknowledges an alert.
        }
        catch (Exception exception)
        {
            SetState(MonitorState.Faulted);
            RaiseSnapshot(new MonitorSnapshot(
                MonitorState.Faulted,
                scanCount,
                TimeSpan.Zero,
                TimeSpan.Zero,
                [],
                exception.Message));
        }
        finally
        {
            if (inputGuardHeld)
            {
                InputGuardReleased?.Invoke(this, EventArgs.Empty);
            }
        }
    }

    private async Task RunGuardedRuleLoopAsync(
        CompiledRuleSet rules,
        ScreenRegion region,
        GuardedPolicyStateMachine guarded,
        CancellationToken cancellationToken)
    {
        long scanCount = 0;
        MonitorSnapshot? lastSnapshot = null;

        try
        {
            while (!cancellationToken.IsCancellationRequested)
            {
                if (guarded.State == GuardedSessionState.PausedUncertain)
                {
                    Task resumeTask;
                    lock (_stateGate)
                    {
                        resumeTask = _guardedResumeSignal.Task;
                    }

                    await resumeTask.WaitAsync(cancellationToken).ConfigureAwait(false);
                    continue;
                }

                var captureWatch = Stopwatch.StartNew();
                var frame = _screenCapture.Capture(region);
                var fingerprint = ComputeGuardedFrameFingerprint(frame);
                var captureElapsed = captureWatch.Elapsed;

                // Drain native click evidence only after the captured frame has its fingerprint.
                // A complete Down/Up can land while GDI is copying the tooltip. Consuming the
                // mailbox before Capture would let that changed frame run through the idle path
                // and replace the pre-click baseline, permanently hiding the crafted result.
                if (Interlocked.Exchange(ref _guardedCausativeClickStarted, 0) != 0)
                {
                    guarded.NotifyCausativeClickStarted();
                }
                var causativeClickTicks = Interlocked.Exchange(
                    ref _guardedCausativeClickCompletedUtcTicks,
                    0);
                if (causativeClickTicks != 0)
                {
                    guarded.NotifyCausativeClickCompleted(
                        new DateTimeOffset(causativeClickTicks, TimeSpan.Zero));
                }

                var frameTransition = guarded.ObserveFrame(fingerprint, DateTimeOffset.UtcNow);
                ApplyGuardTransition(frameTransition, lastSnapshot: lastSnapshot);
                if (frameTransition.Decision?.Kind == GuardedDecisionKind.Faulted)
                {
                    SetGuardedFault(
                        scanCount,
                        frameTransition.Decision.Detail,
                        frameTransition.Decision.FaultReason ??
                        GuardedFaultReason.DecisionTimedOut,
                        lastSnapshot);
                    return;
                }

                if (!frameTransition.ShouldRecognize)
                {
                    // Stability is sampled at capture cadence. This is cooperative scheduling,
                    // not a fixed safety delay; there is no two-second timer in this path.
                    await Task.Yield();
                    cancellationToken.ThrowIfCancellationRequested();
                    continue;
                }

                OcrRecognitionResult ocrResult;
                try
                {
                    var remaining = guarded.RemainingDecisionTime(DateTimeOffset.UtcNow);
                    if (remaining <= TimeSpan.Zero)
                    {
                        var timeout = guarded.Fault(
                            "Guarded decision watchdog expired before OCR started.",
                            GuardedFaultReason.DecisionTimedOut);
                        ApplyGuardTransition(timeout, lastSnapshot: lastSnapshot);
                        SetGuardedFault(
                            scanCount,
                            timeout.Decision!.Detail,
                            GuardedFaultReason.DecisionTimedOut,
                            lastSnapshot);
                        return;
                    }

                    using var decisionCancellation = CancellationTokenSource
                        .CreateLinkedTokenSource(cancellationToken);
                    decisionCancellation.CancelAfter(remaining);
                    try
                    {
                        ocrResult = await _ocrRecognizer
                            .RecognizeAsync(frame, rules.Targets, decisionCancellation.Token)
                            .ConfigureAwait(false);
                    }
                    catch (OperationCanceledException)
                        when (!cancellationToken.IsCancellationRequested &&
                              decisionCancellation.IsCancellationRequested)
                    {
                        var timeout = guarded.Fault(
                            "Guarded OCR exceeded its decision watchdog.",
                            GuardedFaultReason.DecisionTimedOut);
                        ApplyGuardTransition(timeout, lastSnapshot: lastSnapshot);
                        SetGuardedFault(
                            scanCount,
                            timeout.Decision!.Detail,
                            GuardedFaultReason.DecisionTimedOut,
                            lastSnapshot);
                        return;
                    }
                }
                catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
                {
                    throw;
                }

                scanCount++;
                cancellationToken.ThrowIfCancellationRequested();
                var evaluation = EvaluateRules(rules, ocrResult);
                lastSnapshot = new MonitorSnapshot(
                    MonitorState.Running,
                    scanCount,
                    captureElapsed,
                    ocrResult.TotalElapsed,
                    ocrResult.Lines,
                    OcrWasCached: ocrResult.WasCached);
                RaiseSnapshot(lastSnapshot);

                var evidence = CreateGuardedRuleEvidence(evaluation, ocrResult);
                var decisionTransition = guarded.ObserveRecognition(evidence);

                if (decisionTransition.Decision?.Kind == GuardedDecisionKind.Match)
                {
                    if (!TrySetMatchFound(cancellationToken))
                    {
                        return;
                    }

                    ApplyGuardTransition(
                        decisionTransition,
                        evaluation,
                        lastSnapshot);
                    var detectedText = CreateRuleMatchDetail(evaluation);
                    var matchedSnapshot = lastSnapshot with
                    {
                        State = MonitorState.MatchFound,
                        Detail = detectedText,
                    };
                    RaiseSnapshot(matchedSnapshot);
                    AffixDetected?.Invoke(
                        this,
                        new AffixDetectedEventArgs(evaluation, detectedText, matchedSnapshot));
                    return;
                }

                ApplyGuardTransition(
                    decisionTransition,
                    evaluation,
                    lastSnapshot);

                if (decisionTransition.Decision?.Kind == GuardedDecisionKind.Uncertain)
                {
                    continue;
                }

                if (ocrResult.RequiresRescan)
                {
                    await Task.Yield();
                }
                else
                {
                    await Task.Delay(ocrResult.WasCached ? 8 : 4, cancellationToken)
                        .ConfigureAwait(false);
                }
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // Explicit Stop/Dispose owns the fail-open transition outside the loop.
        }
        catch (Exception exception)
        {
            var transition = guarded.Fault(
                exception.Message,
                GuardedFaultReason.CaptureOrRecognitionFailed);
            ApplyGuardTransition(transition, lastSnapshot: lastSnapshot);
            SetGuardedFault(
                scanCount,
                exception.Message,
                GuardedFaultReason.CaptureOrRecognitionFailed,
                lastSnapshot);
        }
        finally
        {
            if (guarded.IsInputGuardHeld &&
                guarded.State is not GuardedSessionState.PausedUncertain and
                not GuardedSessionState.MatchFound)
            {
                ApplyGuardTransition(guarded.Stop(), lastSnapshot: lastSnapshot);
            }
        }
    }

    private static GuardedRuleEvidence CreateGuardedRuleEvidence(
        RuleEvaluationResult evaluation,
        OcrRecognitionResult ocrResult)
    {
        var relevantConditions = evaluation.Groups
            .SelectMany(static group => group.Conditions)
            .Where(static condition => condition.TextMatched)
            .OrderBy(static condition => condition.Template, StringComparer.Ordinal)
            .ThenBy(static condition => condition.StartLineIndex)
            .ToArray();
        var relevantCandidates = ocrResult.BatchCandidateEvidence
            .Where(candidate => IsCompiledTarget(candidate.CandidateTarget, evaluation))
            .OrderBy(static candidate => candidate.CandidateTarget, StringComparer.Ordinal)
            .ThenBy(static candidate => candidate.PhysicalBandId, StringComparer.Ordinal)
            .ToArray();

        // Signatures deliberately omit unrelated OCR lines. Only a strict target observation or
        // recognizer-localized near-target candidate may make repeated evidence disagree.
        var signature = string.Join('|', relevantConditions.Select(static condition =>
                $"{condition.Template}:{string.Join(',', condition.ActualValues.Select(value => value?.ToString(System.Globalization.CultureInfo.InvariantCulture) ?? "?"))}:{condition.IsMatched}")) +
            "#" +
            string.Join('|', relevantCandidates.Select(static candidate =>
                $"{candidate.CandidateTarget}:{candidate.PhysicalBandId}:{candidate.Kind}"));
        if (signature == "#")
        {
            signature = "none";
        }

        return new GuardedRuleEvidence(
            evaluation.IsMatch,
            ocrResult.RequiresRescan,
            HasTargetCandidate: relevantConditions.Length > 0 || relevantCandidates.Length > 0,
            HasUnverifiedTargetCandidate:
                relevantCandidates.Any(static candidate =>
                    candidate.Kind == OcrCandidateEvidenceKind.LocalizedLexicalCandidate),
            HasMissingRequiredNumericValue:
                relevantConditions.Any(static condition =>
                    condition.HasMissingRequiredNumericValue) ||
                relevantCandidates.Any(static candidate =>
                    candidate.Kind == OcrCandidateEvidenceKind.MissingNumericValue),
            HasConflictingTargetNumericEvidence:
                relevantCandidates.Any(static candidate =>
                    candidate.Kind == OcrCandidateEvidenceKind.ConflictingNumericValue),
            signature);
    }

    private static bool IsCompiledTarget(
        string candidateTarget,
        RuleEvaluationResult evaluation) =>
        evaluation.Groups
            .SelectMany(static group => group.Conditions)
            .Any(condition => string.Equals(
                AffixCanonicalizer.Normalize(condition.Template).Text,
                candidateTarget,
                StringComparison.Ordinal));

    private void ApplyGuardTransition(
        GuardedFrameTransition transition,
        RuleEvaluationResult? evaluation = null,
        MonitorSnapshot? lastSnapshot = null)
    {
        if (transition.ReleaseInputGuard)
        {
            InputGuardReleased?.Invoke(this, EventArgs.Empty);
        }

        if (transition.AwaitNextCausativeClick)
        {
            if (GuardedPassCycleRequested is not { } passCycleRequested)
            {
                FailGuardedSession(
                    "No owner accepted the Guarded one-click pass cycle.",
                    GuardedFaultReason.InputGuardUnavailable);
                return;
            }

            passCycleRequested.Invoke(this, EventArgs.Empty);
        }

        if (transition.RequestInputGuard)
        {
            InputGuardRequested?.Invoke(this, EventArgs.Empty);
        }

        if (transition.Decision is not { } decision)
        {
            return;
        }

        GuardedDecisionChanged?.Invoke(
            this,
            new GuardedDecisionEventArgs(
                decision.Kind,
                transition.State,
                decision.Detail,
                decision.UncertaintyReason,
                decision.FaultReason,
                evaluation,
                lastSnapshot));
    }

    private void SetGuardedFault(
        long scanCount,
        string detail,
        GuardedFaultReason reason,
        MonitorSnapshot? previousSnapshot)
    {
        SetState(MonitorState.Faulted);
        RaiseSnapshot(new MonitorSnapshot(
            MonitorState.Faulted,
            scanCount,
            previousSnapshot?.CaptureElapsed ?? TimeSpan.Zero,
            previousSnapshot?.OcrElapsed ?? TimeSpan.Zero,
            previousSnapshot?.LastLines ?? [],
            $"Guarded {reason}: {detail}",
            previousSnapshot?.OcrWasCached ?? false));
    }

    private static TaskCompletionSource CreateGuardedResumeSignal() =>
        new(TaskCreationOptions.RunContinuationsAsynchronously);

    private ulong ComputeGuardedFrameFingerprint(CapturedFrame frame) =>
        _ocrRecognizer switch
        {
            IFrameFingerprintProvider legacyProvider =>
                legacyProvider.ComputeFrameFingerprint(frame),
            _ => throw new NotSupportedException(
                $"{_ocrRecognizer.GetType().Name} does not expose a Guarded semantic fingerprint."),
        };

    private void EnsureCanStart()
    {
        if (_disposeTask is not null)
        {
            throw new ObjectDisposedException(nameof(AffixMonitor));
        }

        if (_stopInProgress)
        {
            throw new InvalidOperationException("Monitoring is still stopping.");
        }

        if (_loopTask is { IsCompleted: false })
        {
            throw new InvalidOperationException("Monitoring is already running.");
        }
    }

    private static RuleEvaluationResult EvaluateRules(
        CompiledRuleSet rules,
        OcrRecognitionResult ocrResult) =>
        rules.Evaluate(
            ocrResult.Lines,
            ocrResult.BatchAssistedObservations.Select(static observation =>
                new AssistedModifierObservation(
                    observation.PhysicalBandId,
                    observation.OriginalText,
                    observation.CanonicalTarget,
                    observation.RelatedPhysicalBandIds)).ToArray(),
            ocrResult.BatchPhysicalLines.Select(static line =>
                new PhysicalLineIdentity(line.LineIndex, line.PhysicalBandId)).ToArray());

    private static string CreateRuleMatchDetail(RuleEvaluationResult evaluation)
    {
        var group = evaluation.MatchedGroup;
        if (group is null)
        {
            return "Structured rule matched.";
        }

        var observations = group.Conditions
            .Where(static condition => condition.IsMatched && condition.Observation is not null)
            .Select(static condition => condition.Observation!.OriginalText)
            .Distinct(StringComparer.Ordinal)
            .ToArray();
        return observations.Length == 0
            ? group.Name
            : $"{group.Name}: {string.Join(" + ", observations)}";
    }

    private void SetState(MonitorState state)
    {
        lock (_stateGate)
        {
            _state = state;
        }
    }

    private bool TrySetMatchFound(CancellationToken cancellationToken)
    {
        lock (_stateGate)
        {
            if (_stopInProgress || cancellationToken.IsCancellationRequested ||
                _state != MonitorState.Running)
            {
                return false;
            }

            _state = MonitorState.MatchFound;
            return true;
        }
    }

    private void RaiseSnapshot(MonitorSnapshot snapshot) => SnapshotChanged?.Invoke(this, snapshot);

    private static bool IsAssistedMatchForTarget(
        LogicalAffixMatch? assistedMatch,
        FullLineAffixMatcher matcher) =>
        assistedMatch is not null &&
        string.Equals(
            assistedMatch.CanonicalText,
            matcher.Template.Text,
            StringComparison.Ordinal);

    public ValueTask DisposeAsync()
    {
        lock (_stateGate)
        {
            _disposeTask ??= DisposeCoreAsync();
            return new ValueTask(_disposeTask);
        }
    }

    private async Task DisposeCoreAsync()
    {
        await StopAsync().ConfigureAwait(false);

        CancellationTokenSource? loopCancellation;
        lock (_stateGate)
        {
            if (_disposed)
            {
                return;
            }

            _disposed = true;
            loopCancellation = _loopCancellation;
            _loopCancellation = null;
        }

        try
        {
            _ocrRecognizer.Dispose();
        }
        finally
        {
            _screenCapture.Dispose();
            loopCancellation?.Dispose();
        }
    }
}
