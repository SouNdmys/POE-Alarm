using System.Diagnostics;
using System.IO;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;
using PoeAlarm.Core.Rules;

namespace PoeAlarm.App.Replay;

/// <summary>
/// Runs an archived screenshot through the production OCR and full-line matcher.
/// This service has no UI or alert side effects and does not own the injected OCR recognizer.
/// </summary>
public sealed class ScreenshotAffixAnalyzer
{
    private const int MaximumContinuationScans = 128;
    private readonly IOcrRecognizer _ocrRecognizer;
    private readonly IScreenshotFrameLoader _frameLoader;

    public ScreenshotAffixAnalyzer(
        IOcrRecognizer ocrRecognizer,
        IScreenshotFrameLoader? frameLoader = null)
    {
        ArgumentNullException.ThrowIfNull(ocrRecognizer);

        _ocrRecognizer = ocrRecognizer;
        _frameLoader = frameLoader ?? new ScreenshotFrameLoader();
    }

    public async Task<ScreenshotAffixAnalysis> AnalyzeAsync(
        string imagePath,
        string affixTemplate,
        ScreenRegion? cropRegion = null,
        CancellationToken cancellationToken = default)
    {
        return await AnalyzeAsync(
                imagePath,
                affixTemplate,
                cropRegion,
                FullLineAffixMatcher.MaximumPhysicalLineSpan,
                cancellationToken)
            .ConfigureAwait(false);
    }

    public async Task<ScreenshotAffixAnalysis> AnalyzeAsync(
        string imagePath,
        string affixTemplate,
        ScreenRegion? cropRegion,
        int maximumPhysicalLineSpan,
        CancellationToken cancellationToken = default)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(imagePath);
        ArgumentException.ThrowIfNullOrWhiteSpace(affixTemplate);
        cancellationToken.ThrowIfCancellationRequested();

        // Compile before doing file I/O so an invalid template fails immediately.
        var matcher = new FullLineAffixMatcher(
            affixTemplate,
            maximumPhysicalLineSpan);
        var resolvedPath = Path.GetFullPath(imagePath);

        var loadWatch = Stopwatch.StartNew();
        var frame = await _frameLoader
            .LoadAsync(resolvedPath, cropRegion, cancellationToken)
            .ConfigureAwait(false);
        loadWatch.Stop();

        var preprocessingElapsed = TimeSpan.Zero;
        var recognitionElapsed = TimeSpan.Zero;
        OcrRecognitionResult? ocrResult = null;
        LogicalAffixMatch? match = null;
        for (var scan = 0; scan < MaximumContinuationScans; scan++)
        {
            ocrResult = await _ocrRecognizer
                .RecognizeAsync(frame, matcher, cancellationToken)
                .ConfigureAwait(false);
            preprocessingElapsed += ocrResult.PreprocessingElapsed;
            recognitionElapsed += ocrResult.RecognitionElapsed;

            cancellationToken.ThrowIfCancellationRequested();
            matcher.TryFindMatch(ocrResult.Lines, out match);
            if (match is null &&
                ocrResult.TargetAssistedMatch is { } assisted &&
                string.Equals(assisted.CanonicalText, matcher.Template.Text, StringComparison.Ordinal))
            {
                match = assisted;
            }

            if (match is not null || !ocrResult.RequiresRescan)
            {
                break;
            }
        }

        if (ocrResult is null || (ocrResult.RequiresRescan && match is null))
        {
            throw new InvalidDataException(
                $"OCR did not finish after {MaximumContinuationScans} progressive screenshot scans.");
        }

        return new ScreenshotAffixAnalysis(
            resolvedPath,
            cropRegion,
            frame.Width,
            frame.Height,
            ocrResult.Lines,
            loadWatch.Elapsed,
            preprocessingElapsed,
            recognitionElapsed,
            match);
    }

    /// <summary>
    /// Evaluates every compiled condition over one batched OCR request per scan. A recognizer may
    /// request bounded progressive continuation on the same frame, but this never invokes OCR once
    /// per condition, so rule count cannot multiply full-frame recognition work.
    /// </summary>
    public async Task<ScreenshotRuleAnalysis> AnalyzeRulesAsync(
        string imagePath,
        CompiledRuleSet rules,
        ScreenRegion? cropRegion = null,
        CancellationToken cancellationToken = default)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(imagePath);
        ArgumentNullException.ThrowIfNull(rules);
        if (_ocrRecognizer.StructuredRuleSupport == StructuredRuleOcrSupport.Unsupported)
        {
            throw new NotSupportedException(
                $"{_ocrRecognizer.GetType().Name} has not passed structured-rule batch OCR validation.");
        }

        cancellationToken.ThrowIfCancellationRequested();

        var resolvedPath = Path.GetFullPath(imagePath);
        var loadWatch = Stopwatch.StartNew();
        var frame = await _frameLoader
            .LoadAsync(resolvedPath, cropRegion, cancellationToken)
            .ConfigureAwait(false);
        loadWatch.Stop();

        var preprocessingElapsed = TimeSpan.Zero;
        var recognitionElapsed = TimeSpan.Zero;
        var evaluationElapsed = TimeSpan.Zero;
        OcrRecognitionResult? ocrResult = null;
        RuleEvaluationResult? evaluation = null;
        for (var scan = 0; scan < MaximumContinuationScans; scan++)
        {
            // One request represents the complete, deduplicated target set. A progressive
            // recognizer may continue the same frame, but target count never multiplies calls.
            ocrResult = await _ocrRecognizer
                .RecognizeAsync(frame, rules.Targets, cancellationToken)
                .ConfigureAwait(false);
            preprocessingElapsed += ocrResult.PreprocessingElapsed;
            recognitionElapsed += ocrResult.RecognitionElapsed;
            cancellationToken.ThrowIfCancellationRequested();

            var evaluationWatch = Stopwatch.StartNew();
            evaluation = rules.Evaluate(
                ocrResult.Lines,
                ocrResult.BatchAssistedObservations.Select(static observation =>
                    new AssistedModifierObservation(
                        observation.PhysicalBandId,
                        observation.OriginalText,
                        observation.CanonicalTarget,
                        observation.RelatedPhysicalBandIds)).ToArray(),
                ocrResult.BatchPhysicalLines.Select(static line =>
                    new PhysicalLineIdentity(line.LineIndex, line.PhysicalBandId)).ToArray());
            evaluationWatch.Stop();
            evaluationElapsed += evaluationWatch.Elapsed;
            if (evaluation.IsMatch || !ocrResult.RequiresRescan)
            {
                break;
            }
        }

        if (ocrResult is null || evaluation is null ||
            (ocrResult.RequiresRescan && !evaluation.IsMatch))
        {
            throw new InvalidDataException(
                $"OCR did not finish after {MaximumContinuationScans} progressive structured-rule scans.");
        }

        return new ScreenshotRuleAnalysis(
            resolvedPath,
            cropRegion,
            frame.Width,
            frame.Height,
            ocrResult.Lines,
            loadWatch.Elapsed,
            preprocessingElapsed,
            recognitionElapsed,
            evaluationElapsed,
            evaluation);
    }
}

public sealed record ScreenshotAffixAnalysis(
    string ImagePath,
    ScreenRegion? CropRegion,
    int FrameWidth,
    int FrameHeight,
    IReadOnlyList<string> Lines,
    TimeSpan ImageLoadElapsed,
    TimeSpan PreprocessingElapsed,
    TimeSpan RecognitionElapsed,
    LogicalAffixMatch? Match)
{
    public bool IsMatch => Match is not null;

    public TimeSpan OcrElapsed => PreprocessingElapsed + RecognitionElapsed;

    public TimeSpan TotalElapsed => ImageLoadElapsed + OcrElapsed;
}

public sealed record ScreenshotRuleAnalysis(
    string ImagePath,
    ScreenRegion? CropRegion,
    int FrameWidth,
    int FrameHeight,
    IReadOnlyList<string> Lines,
    TimeSpan ImageLoadElapsed,
    TimeSpan PreprocessingElapsed,
    TimeSpan RecognitionElapsed,
    TimeSpan RuleEvaluationElapsed,
    RuleEvaluationResult Evaluation)
{
    public bool IsMatch => Evaluation.IsMatch;

    public TimeSpan OcrElapsed => PreprocessingElapsed + RecognitionElapsed;

    public TimeSpan TotalElapsed => ImageLoadElapsed + OcrElapsed + RuleEvaluationElapsed;
}
