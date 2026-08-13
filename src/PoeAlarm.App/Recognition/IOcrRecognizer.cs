using PoeAlarm.App.Capture;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Recognition;

/// <summary>
/// Describes whether a recognizer can safely evaluate a structured target set in one shared
/// frame pass. Unsupported recognizers must not silently fall back to target-free OCR because
/// that would discard their verified target-assisted/progressive behavior.
/// </summary>
public enum StructuredRuleOcrSupport
{
    Unsupported,
    StrictBatch,
    ConfirmedStrictBatch,
}

public interface IOcrRecognizer : IDisposable
{
    StructuredRuleOcrSupport StructuredRuleSupport => StructuredRuleOcrSupport.Unsupported;

    Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default);

    Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken = default) =>
        RecognizeAsync(frame, cancellationToken);

    /// <summary>
    /// Recognizes one frame for a compiled, deduplicated target set. Callers must never emulate
    /// this by invoking the single-target overload once per rule. The default rejects the call:
    /// each production recognizer must explicitly opt in after preserving its safety contract.
    /// </summary>
    Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        IReadOnlyList<FullLineAffixMatcher> targets,
        CancellationToken cancellationToken = default) =>
        throw new NotSupportedException(
            $"{GetType().Name} does not support structured-rule batch recognition.");
}
