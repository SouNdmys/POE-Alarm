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

    public AffixMonitor(IScreenCapture screenCapture, IOcrRecognizer ocrRecognizer)
    {
        _screenCapture = screenCapture;
        _ocrRecognizer = ocrRecognizer;
    }

    public event EventHandler<MonitorSnapshot>? SnapshotChanged;

    public event EventHandler<AffixDetectedEventArgs>? AffixDetected;

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

    public void Start(string template, ScreenRegion region)
    {
        if (!region.IsValid)
        {
            throw new ArgumentOutOfRangeException(nameof(region), "Select a valid tooltip region before monitoring.");
        }

        var matcher = new FullLineAffixMatcher(template);

        lock (_stateGate)
        {
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

        try
        {
            while (!cancellationToken.IsCancellationRequested)
            {
                var captureWatch = Stopwatch.StartNew();
                var frame = _screenCapture.Capture(region);
                var captureElapsed = captureWatch.Elapsed;

                var ocrResult = await _ocrRecognizer
                    .RecognizeAsync(frame, cancellationToken)
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
                if (matcher.TryFindMatch(ocrResult.Lines, out var match) && match is not null)
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

                // Yield briefly so the UI and game retain priority. OCR time remains the
                // dominant cadence; this does not impose a coarse polling interval.
                await Task.Delay(ocrResult.WasCached ? 8 : 4, cancellationToken).ConfigureAwait(false);
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

    public async ValueTask DisposeAsync()
    {
        await StopAsync().ConfigureAwait(false);
        _ocrRecognizer.Dispose();
        _screenCapture.Dispose();
        _loopCancellation?.Dispose();
    }
}
