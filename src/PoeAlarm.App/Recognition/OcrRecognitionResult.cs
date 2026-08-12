using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Recognition;

public sealed record OcrRecognitionResult(
    IReadOnlyList<string> Lines,
    TimeSpan PreprocessingElapsed,
    TimeSpan RecognitionElapsed,
    bool WasCached = false,
    LogicalAffixMatch? TargetAssistedMatch = null,
    bool RequiresRescan = false)
{
    public TimeSpan TotalElapsed => PreprocessingElapsed + RecognitionElapsed;
}
