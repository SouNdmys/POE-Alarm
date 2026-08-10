namespace PoeAlarm.App.Recognition;

public sealed record OcrRecognitionResult(
    IReadOnlyList<string> Lines,
    TimeSpan PreprocessingElapsed,
    TimeSpan RecognitionElapsed,
    bool WasCached = false)
{
    public TimeSpan TotalElapsed => PreprocessingElapsed + RecognitionElapsed;
}
