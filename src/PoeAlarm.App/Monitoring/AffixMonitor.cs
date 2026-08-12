using System.Diagnostics;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;

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
    /// Raised before OCR starts on a changed frame for recognizers that expose a semantic
    /// fingerprint. The UI uses this to install a short, invisible input guard while that frame
    /// is being decided.
    /// </summary>
    public event EventHandler? InputGuardRequested;

    /// <summary>Raised when a guarded changed frame completes without a target match.</summary>
    public event EventHandler? InputGuardReleased;

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

            _loopCancellation?.Dispose();
            _loopCancellation = new CancellationTokenSource();
            _state = MonitorState.Running;
            _loopTask = RunLoopAsync(matcher, region, _loopCancellation.Token);
        }
    }

    public async Task StopAsync()
    {
        Task? loopTask;
        var ownsStop = false;
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
            }

            loopTask = _loopTask;
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
