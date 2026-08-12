using System.Diagnostics;
using PoeAlarm.App.Capture;

namespace PoeAlarm.TransientReplay;

/// <summary>
/// Models real roll attempts: clicks occur at a fixed physical cadence, but a held monitor
/// input guard consumes them without changing the tooltip. A consumed click is never replayed.
/// </summary>
internal sealed class GuardedRollCapture(TransientFrameSet frames) : IReplayCapture
{
    private readonly object _gate = new();
    private readonly List<CaptureSample> _samples = [];
    private readonly List<ReplayTraceEvent> _trace = [];
    private long _sequenceStart;
    private double _nextClickMs;
    private double _dwellMs;
    private RollState _state;
    private bool _guardHeld;
    private bool _matched;
    private int _physicalClicks;
    private int _acceptedClicks;
    private int _swallowedClicks;
    private double? _targetOnsetMs;
    private double? _targetEndMs;
    private bool _overroll;
    private double? _firstGuardRequestedMs;
    private double? _lastGuardReleasedMs;
    private double? _guardStartedMs;
    private double _guardHeldMs;

    public ScreenRegion Region { get; } = new(0, 0, frames.Width, frames.Height);

    public double ElapsedMilliseconds =>
        Stopwatch.GetElapsedTime(Volatile.Read(ref _sequenceStart)).TotalMilliseconds;

    public void Configure(double firstClickMs, double dwellMs)
    {
        lock (_gate)
        {
            frames.BeginSequence();
            _samples.Clear();
            _trace.Clear();
            _nextClickMs = firstClickMs;
            _dwellMs = dwellMs;
            _state = RollState.PreTarget;
            _guardHeld = false;
            _matched = false;
            _physicalClicks = 0;
            _acceptedClicks = 0;
            _swallowedClicks = 0;
            _targetOnsetMs = null;
            _targetEndMs = null;
            _overroll = false;
            _firstGuardRequestedMs = null;
            _lastGuardReleasedMs = null;
            _guardStartedMs = null;
            _guardHeldMs = 0;
            _sequenceStart = Stopwatch.GetTimestamp();
        }
    }

    public CapturedFrame Capture(ScreenRegion region)
    {
        if (region != Region)
        {
            throw new ArgumentOutOfRangeException(nameof(region), region, "Unexpected replay ROI.");
        }

        bool present;
        double elapsed;
        lock (_gate)
        {
            elapsed = ElapsedMilliseconds;
            AdvanceClicks(elapsed);
            present = _state == RollState.Target;
            _samples.Add(new CaptureSample(elapsed, present));
            _trace.Add(new ReplayTraceEvent(
                elapsed,
                "capture",
                $"source={(present ? "target" : "absent")} guard={_guardHeld}"));
        }

        return frames.Next(present);
    }

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

    public void OnGuardRequested()
    {
        lock (_gate)
        {
            var elapsed = ElapsedMilliseconds;
            AdvanceClicks(elapsed);
            if (!_guardHeld)
            {
                _guardHeld = true;
                _guardStartedMs = elapsed;
                _firstGuardRequestedMs ??= elapsed;
            }

            _trace.Add(new ReplayTraceEvent(elapsed, "guard-request", "guard=true"));
        }
    }

    public void OnGuardReleased()
    {
        lock (_gate)
        {
            var elapsed = ElapsedMilliseconds;
            AdvanceClicks(elapsed);
            if (_guardHeld)
            {
                AccumulateGuard(elapsed);
                _guardHeld = false;
                _lastGuardReleasedMs = elapsed;
            }

            _trace.Add(new ReplayTraceEvent(elapsed, "guard-release", "guard=false"));
        }
    }

    public void OnMatch()
    {
        lock (_gate)
        {
            var elapsed = ElapsedMilliseconds;
            AdvanceClicks(elapsed);
            if (_guardHeld)
            {
                AccumulateGuard(elapsed);
                _guardStartedMs = elapsed;
            }

            _matched = true;
            _trace.Add(new ReplayTraceEvent(elapsed, "match", "monitor matched; click clock stopped"));
        }
    }

    public void RecordDecision(long scanCount, double ocrMs, bool cached)
    {
        lock (_gate)
        {
            var elapsed = ElapsedMilliseconds;
            _trace.Add(new ReplayTraceEvent(
                elapsed,
                "decision",
                $"scan={scanCount} source={(_samples.Count > 0 && _samples[^1].TargetPresent ? "target" : "absent")} " +
                $"ocr={ocrMs:F2}ms cached={cached} guard={_guardHeld}"));
        }
    }

    public ReplayTrialStats GetStats()
    {
        lock (_gate)
        {
            var guardHeld = _guardHeld && _guardStartedMs is not null
                ? _guardHeldMs + Math.Max(0, ElapsedMilliseconds - _guardStartedMs.Value)
                : _guardHeldMs;
            return new ReplayTrialStats(
                _physicalClicks,
                _acceptedClicks,
                _swallowedClicks,
                _targetOnsetMs,
                _targetEndMs,
                _overroll,
                _firstGuardRequestedMs,
                _lastGuardReleasedMs,
                guardHeld);
        }
    }

    public IReadOnlyList<ReplayTraceEvent> GetTraceEvents()
    {
        lock (_gate)
        {
            return _trace.OrderBy(item => item.ElapsedMs).ToArray();
        }
    }

    private void AdvanceClicks(double elapsed)
    {
        while (!_matched && _nextClickMs <= elapsed)
        {
            var clickMs = _nextClickMs;
            _nextClickMs += _dwellMs;
            _physicalClicks++;
            if (_guardHeld)
            {
                _swallowedClicks++;
                _trace.Add(new ReplayTraceEvent(clickMs, "click-swallowed", "guard=true; frame unchanged"));
                continue;
            }

            _acceptedClicks++;
            switch (_state)
            {
                case RollState.PreTarget:
                    _state = RollState.Target;
                    _targetOnsetMs = clickMs;
                    _trace.Add(new ReplayTraceEvent(clickMs, "click-accepted", "absent->target"));
                    break;
                case RollState.Target:
                    _state = RollState.PostTarget;
                    _targetEndMs = clickMs;
                    _overroll = true;
                    _trace.Add(new ReplayTraceEvent(clickMs, "click-accepted", "target->absent OVERROLL"));
                    break;
                case RollState.PostTarget:
                    _trace.Add(new ReplayTraceEvent(clickMs, "click-accepted", "post-target remains absent"));
                    break;
            }
        }
    }

    private void AccumulateGuard(double elapsed)
    {
        if (_guardStartedMs is not null)
        {
            _guardHeldMs += Math.Max(0, elapsed - _guardStartedMs.Value);
            _guardStartedMs = null;
        }
    }

    public void Dispose()
    {
    }

    private enum RollState
    {
        PreTarget,
        Target,
        PostTarget,
    }
}
