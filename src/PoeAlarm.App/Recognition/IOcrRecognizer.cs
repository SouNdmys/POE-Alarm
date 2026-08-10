using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Recognition;

public interface IOcrRecognizer : IDisposable
{
    Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default);
}
