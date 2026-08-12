using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.TransientReplay;

/// <summary>
/// Reassembles the 0.4.1 capture cadence on top of the sliced recognizer: all deferred row
/// batches for one captured bitmap are exhausted before Capture is allowed to run again.
/// It intentionally lives in the benchmark, so production code needs no baseline switch.
/// </summary>
internal sealed class ExhaustiveFrameRecognizer(IOcrRecognizer inner) : IOcrRecognizer
{
    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default) =>
        RecognizeCoreAsync(frame, target: null, cancellationToken);

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken = default) =>
        RecognizeCoreAsync(frame, target, cancellationToken);

    private async Task<OcrRecognitionResult> RecognizeCoreAsync(
        CapturedFrame frame,
        FullLineAffixMatcher? target,
        CancellationToken cancellationToken)
    {
        var preprocessing = TimeSpan.Zero;
        var recognition = TimeSpan.Zero;
        for (var slice = 0; slice < 128; slice++)
        {
            var result = target is null
                ? await inner.RecognizeAsync(frame, cancellationToken).ConfigureAwait(false)
                : await inner.RecognizeAsync(frame, target, cancellationToken).ConfigureAwait(false);
            preprocessing += result.PreprocessingElapsed;
            recognition += result.RecognitionElapsed;
            if (!result.RequiresRescan)
            {
                return result with
                {
                    PreprocessingElapsed = preprocessing,
                    RecognitionElapsed = recognition,
                    RequiresRescan = false,
                };
            }
        }

        throw new InvalidOperationException("Recognizer did not exhaust one frame within 128 row slices.");
    }

    public void Dispose() => inner.Dispose();
}
