using System.Diagnostics;
using System.IO;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Replay;

/// <summary>
/// Runs an archived screenshot through the production OCR and full-line matcher.
/// This service has no UI or alert side effects and does not own the injected OCR recognizer.
/// </summary>
public sealed class ScreenshotAffixAnalyzer
{
    private readonly IOcrRecognizer _ocrRecognizer;
    private readonly ScreenshotFrameLoader _frameLoader;

    public ScreenshotAffixAnalyzer(
        IOcrRecognizer ocrRecognizer,
        ScreenshotFrameLoader? frameLoader = null)
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
        ArgumentException.ThrowIfNullOrWhiteSpace(imagePath);
        ArgumentException.ThrowIfNullOrWhiteSpace(affixTemplate);
        cancellationToken.ThrowIfCancellationRequested();

        // Compile before doing file I/O so an invalid template fails immediately.
        var matcher = new FullLineAffixMatcher(affixTemplate);
        var resolvedPath = Path.GetFullPath(imagePath);

        var loadWatch = Stopwatch.StartNew();
        var frame = await _frameLoader
            .LoadAsync(resolvedPath, cropRegion, cancellationToken)
            .ConfigureAwait(false);
        loadWatch.Stop();

        var ocrResult = await _ocrRecognizer
            .RecognizeAsync(frame, cancellationToken)
            .ConfigureAwait(false);

        cancellationToken.ThrowIfCancellationRequested();
        matcher.TryFindMatch(ocrResult.Lines, out var match);

        return new ScreenshotAffixAnalysis(
            resolvedPath,
            cropRegion,
            frame.Width,
            frame.Height,
            ocrResult.Lines,
            loadWatch.Elapsed,
            ocrResult.PreprocessingElapsed,
            ocrResult.RecognitionElapsed,
            match);
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
