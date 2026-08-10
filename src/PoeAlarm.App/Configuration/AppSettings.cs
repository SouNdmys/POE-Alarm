using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Configuration;

public sealed record AppSettings
{
    public string TargetAffix { get; init; } = string.Empty;

    public ScreenRegion? CaptureRegion { get; init; }

    public int OcrScale { get; init; } = 2;
}
