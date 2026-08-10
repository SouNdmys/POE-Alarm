namespace PoeAlarm.App.Monitoring;

public sealed record MonitorSnapshot(
    MonitorState State,
    long ScanCount,
    TimeSpan CaptureElapsed,
    TimeSpan OcrElapsed,
    IReadOnlyList<string> LastLines,
    string? Detail = null,
    bool OcrWasCached = false);
