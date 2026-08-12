using PoeAlarm.App.Capture;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Recognition;

public interface IOcrRecognizer : IDisposable
{
    Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default);

    Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken = default) =>
        RecognizeAsync(frame, cancellationToken);
}
