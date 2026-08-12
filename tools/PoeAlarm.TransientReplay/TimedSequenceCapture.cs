using System.Diagnostics;
using PoeAlarm.App.Capture;

namespace PoeAlarm.TransientReplay;

internal sealed class TimedSequenceCapture(TransientFrameSet frames) : IReplayCapture
{
    private readonly object _gate = new();
    private readonly List<CaptureSample> _samples = [];
    private readonly List<ReplayTraceEvent> _trace = [];
    private long _sequenceStart;
    private double _targetStartMs;
    private double _targetEndMs;

    public ScreenRegion Region { get; } = new(0, 0, frames.Width, frames.Height);

    public void Configure(double targetStartMs, double dwellMs)
    {
        lock (_gate)
        {
            frames.BeginSequence();
            _samples.Clear();
            _trace.Clear();
            _targetStartMs = targetStartMs;
            _targetEndMs = targetStartMs + dwellMs;
            _sequenceStart = Stopwatch.GetTimestamp();
        }
    }

    public CapturedFrame Capture(ScreenRegion region)
    {
        if (region != Region)
        {
            throw new ArgumentOutOfRangeException(nameof(region), region, "Unexpected replay ROI.");
        }

        var elapsed = ElapsedMilliseconds;
        var present = elapsed >= _targetStartMs && elapsed < _targetEndMs;
        lock (_gate)
        {
            _samples.Add(new CaptureSample(elapsed, present));
            _trace.Add(new ReplayTraceEvent(
                elapsed,
                "capture",
                present ? "source=target" : "source=absent"));
        }

        return frames.Next(present);
    }

    public double ElapsedMilliseconds =>
        Stopwatch.GetElapsedTime(Volatile.Read(ref _sequenceStart)).TotalMilliseconds;

    public IReadOnlyList<CaptureSample> GetSamples()
    {
        lock (_gate)
        {
            return _samples.ToArray();
        }
    }

    public CaptureSample? GetLastSample()
    {
        lock (_gate)
        {
            return _samples.Count == 0 ? null : _samples[^1];
        }
    }

    public void OnGuardRequested() => Record("guard-request", "fixed-dwell ignores guard");

    public void OnGuardReleased() => Record("guard-release", "fixed-dwell ignores guard");

    public void OnMatch() => Record("match", "monitor matched target");

    public void RecordDecision(long scanCount, double ocrMs, bool cached) =>
        Record("decision", $"scan={scanCount} ocr={ocrMs:F2}ms cached={cached}");

    public ReplayTrialStats GetStats()
    {
        var elapsed = ElapsedMilliseconds;
        var first = elapsed >= _targetStartMs ? 1 : 0;
        var second = elapsed >= _targetEndMs ? 1 : 0;
        return new ReplayTrialStats(
            first + second,
            first + second,
            0,
            _targetStartMs,
            _targetEndMs,
            Overroll: second > 0,
            FirstGuardRequestedMs: null,
            LastGuardReleasedMs: null,
            GuardHeldMs: 0);
    }

    public IReadOnlyList<ReplayTraceEvent> GetTraceEvents()
    {
        lock (_gate)
        {
            return _trace.OrderBy(item => item.ElapsedMs).ToArray();
        }
    }

    private void Record(string kind, string detail)
    {
        var elapsed = ElapsedMilliseconds;
        lock (_gate)
        {
            _trace.Add(new ReplayTraceEvent(elapsed, kind, detail));
        }
    }

    public void Dispose()
    {
    }
}

internal readonly record struct CaptureSample(double ElapsedMilliseconds, bool TargetPresent);
