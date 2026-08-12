using PoeAlarm.App.Capture;

namespace PoeAlarm.TransientReplay;

internal interface IReplayCapture : IScreenCapture
{
    ScreenRegion Region { get; }

    double ElapsedMilliseconds { get; }

    void Configure(double firstClickMs, double dwellMs);

    IReadOnlyList<CaptureSample> GetSamples();

    CaptureSample? GetLastSample();

    void OnGuardRequested();

    void OnGuardReleased();

    void OnMatch();

    void RecordDecision(long scanCount, double ocrMs, bool cached);

    ReplayTrialStats GetStats();

    IReadOnlyList<ReplayTraceEvent> GetTraceEvents();
}

internal sealed record ReplayTrialStats(
    int PhysicalClicks,
    int AcceptedClicks,
    int SwallowedClicks,
    double? TargetOnsetMs,
    double? TargetEndMs,
    bool Overroll,
    double? FirstGuardRequestedMs,
    double? LastGuardReleasedMs,
    double GuardHeldMs);

internal sealed record ReplayTraceEvent(double ElapsedMs, string Kind, string Detail);
