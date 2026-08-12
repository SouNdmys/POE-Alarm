using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.TransientReplay;

/// <summary>
/// Benchmark-only adapter used to measure the POE2 English profile with the same changed-frame
/// input-guard contract as the production Chinese recognizer. It intentionally does not alter
/// the proven POE1 English recognizer or application defaults.
/// </summary>
internal sealed class FingerprintingRecognizerAdapter(
    IOcrRecognizer inner,
    PoeTextPreprocessor? preprocessor = null) : IOcrRecognizer, IFrameFingerprintProvider
{
    private readonly IOcrRecognizer _inner = inner;
    private readonly PoeTextPreprocessor _preprocessor = preprocessor ?? new PoeTextPreprocessor();

    public ulong ComputeFrameFingerprint(CapturedFrame frame) =>
        _preprocessor.ComputeBlueTextFingerprint(frame);

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default) =>
        _inner.RecognizeAsync(frame, cancellationToken);

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken = default) =>
        _inner.RecognizeAsync(frame, target, cancellationToken);

    public void Dispose() => _inner.Dispose();
}
