using System.Diagnostics;
using System.Runtime.InteropServices.WindowsRuntime;
using Windows.Globalization;
using Windows.Graphics.Imaging;
using Windows.Media.Ocr;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Recognition;

public enum OcrRecognitionLanguage
{
    English,
    TraditionalChinese,
}

public sealed class WindowsOcrRecognizer : IOcrRecognizer
{
    private const int MaximumCachedBands = 256;

    private readonly PoeTextPreprocessor _preprocessor;
    private readonly OcrEngine _engine;
    private readonly SemaphoreSlim _recognitionGate = new(1, 1);
    private readonly int _scale;
    private readonly Dictionary<BandCacheKey, string[]> _bandRecognitionCache = [];
    private readonly Queue<BandCacheKey> _bandCacheInsertionOrder = [];
    private readonly bool _retainBandRecognitionDetails;
    private IReadOnlyList<WindowsRecognizedBand> _lastRecognizedBands = [];
    private IReadOnlyList<string> _lastLines = [];
    private ulong _lastFingerprint;
    private bool _hasCachedRecognition;
    private bool _disposed;

    public WindowsOcrRecognizer(
        PoeTextPreprocessor? preprocessor = null,
        int scale = 2,
        OcrRecognitionLanguage language = OcrRecognitionLanguage.English)
        : this(preprocessor, scale, language, retainBandRecognitionDetails: false)
    {
    }

    internal WindowsOcrRecognizer(
        PoeTextPreprocessor? preprocessor,
        int scale,
        OcrRecognitionLanguage language,
        bool retainBandRecognitionDetails)
    {
        _preprocessor = preprocessor ?? new PoeTextPreprocessor();
        _scale = scale;
        _retainBandRecognitionDetails = retainBandRecognitionDetails;
        var installedLanguage = OcrEngine.AvailableRecognizerLanguages.FirstOrDefault(candidate =>
            MatchesLanguage(candidate.LanguageTag, language));
        if (installedLanguage is null)
        {
            throw new InvalidOperationException(language == OcrRecognitionLanguage.TraditionalChinese
                ? "Windows 繁體中文 OCR 不可用。请先在 Windows 语言设置中安装繁體中文（台湾）的 OCR 语言功能（zh-TW）。"
                : "Windows English OCR is unavailable. Install the English OCR language capability in Windows Settings.");
        }

        _engine = OcrEngine.TryCreateFromLanguage(installedLanguage)
                  ?? throw new InvalidOperationException(
                      $"Windows could not initialize its OCR engine ({installedLanguage.LanguageTag}).");
    }

    public string RecognizerLanguageTag => _engine.RecognizerLanguage.LanguageTag;

    /// <summary>
    /// Optional per-band detail used by the POE2 English target verifier and structured physical
    /// identity projection. The normal POE1 quick constructor leaves collection disabled,
    /// preserving its allocation and recognition path.
    /// </summary>
    internal IReadOnlyList<WindowsRecognizedBand> LastRecognizedBands => _lastRecognizedBands;

    public StructuredRuleOcrSupport StructuredRuleSupport =>
        StructuredRuleOcrSupport.StrictBatch;

    public async Task<OcrRecognitionResult> RecognizeAsync(
        Capture.CapturedFrame frame,
        IReadOnlyList<FullLineAffixMatcher> targets,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(targets);
        if (targets.Any(static target => target is null))
        {
            throw new ArgumentException("The structured target set contains a null target.", nameof(targets));
        }

        // Windows English OCR has no target-conditioned inference. One exhaustive pass is the
        // complete verified path; every strict rule is evaluated later against these shared lines.
        var result = await RecognizeInternalAsync(
                frame,
                retainBandDetails: true,
                cancellationToken)
            .ConfigureAwait(false);
        var structuredLines = CreateStructuredLogicalLines(_lastRecognizedBands, frame.Width);
        return result with
        {
            Lines = structuredLines,
            PhysicalLines = CreatePhysicalLineMap(structuredLines, _lastRecognizedBands),
        };
    }

    public async Task<OcrRecognitionResult> RecognizeAsync(
        Capture.CapturedFrame frame,
        CancellationToken cancellationToken = default) =>
        await RecognizeInternalAsync(frame, _retainBandRecognitionDetails, cancellationToken)
            .ConfigureAwait(false);

    private async Task<OcrRecognitionResult> RecognizeInternalAsync(
        Capture.CapturedFrame frame,
        bool retainBandDetails,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(frame);

        await _recognitionGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            var stopwatch = Stopwatch.StartNew();
            var preparedBands = _preprocessor.PrepareBlueAffixBands(frame, _scale);
            var preprocessingElapsed = stopwatch.Elapsed;
            var fingerprint = CombineFingerprints(preparedBands);

            if (_hasCachedRecognition && fingerprint == _lastFingerprint &&
                (!retainBandDetails || _lastRecognizedBands.Count > 0 || _lastLines.Count == 0))
            {
                return new OcrRecognitionResult(
                    _lastLines,
                    preprocessingElapsed,
                    TimeSpan.Zero,
                    WasCached: true);
            }

            if (preparedBands.Any(prepared =>
                    prepared.Width > OcrEngine.MaxImageDimension || prepared.Height > OcrEngine.MaxImageDimension))
            {
                throw new InvalidOperationException(
                    $"A prepared OCR row exceeds Windows OCR's {OcrEngine.MaxImageDimension}-pixel limit. " +
                    "Select a narrower tooltip region.");
            }

            stopwatch.Restart();
            var lines = new List<string>(preparedBands.Count);
            List<WindowsRecognizedBand>? recognizedBands = retainBandDetails
                ? new List<WindowsRecognizedBand>(preparedBands.Count)
                : null;
            int? previousBottom = null;
            var previousHeight = 0;
            var previousWidth = 0;
            var everyBandWasCached = true;
            foreach (var prepared in preparedBands)
            {
                cancellationToken.ThrowIfCancellationRequested();
                var cacheKey = new BandCacheKey(
                    prepared.ContentFingerprint,
                    prepared.Width,
                    prepared.Height);
                if (!_bandRecognitionCache.TryGetValue(cacheKey, out var recognizedLines))
                {
                    everyBandWasCached = false;
                    using var bitmap = SoftwareBitmap.CreateCopyFromBuffer(
                        prepared.BgraPixels.AsBuffer(0, prepared.ByteLength),
                        BitmapPixelFormat.Bgra8,
                        prepared.Width,
                        prepared.Height,
                        BitmapAlphaMode.Premultiplied);

                    var result = await _engine
                        .RecognizeAsync(bitmap)
                        .AsTask(cancellationToken)
                        .ConfigureAwait(false);
                    recognizedLines = result.Lines
                        .Select(line => line.Text.Trim())
                        .Where(line => line.Length > 0)
                        .ToArray();
                    CacheBandRecognition(cacheKey, recognizedLines);
                }

                recognizedBands?.Add(new WindowsRecognizedBand(prepared, recognizedLines));

                if (recognizedLines.Length == 0)
                {
                    continue;
                }

                if (prepared.IsFallback)
                {
                    AddLogicalBoundary(lines);
                    foreach (var recognizedLine in recognizedLines)
                    {
                        lines.Add(recognizedLine);
                        AddLogicalBoundary(lines);
                    }

                    previousBottom = null;
                    previousHeight = 0;
                    previousWidth = 0;
                    continue;
                }

                var currentHeight = prepared.SourceBottom - prepared.SourceTop + 1;
                if (previousBottom is not null)
                {
                    var gap = prepared.SourceTop - previousBottom.Value - 1;
                    var closeEnoughForWrapping = gap <= Math.Max(12, Math.Max(previousHeight, currentHeight));
                    var previousLineFilledTooltip = previousWidth >= frame.Width * 0.70;
                    if (!closeEnoughForWrapping || !previousLineFilledTooltip)
                    {
                        AddLogicalBoundary(lines);
                    }
                }

                lines.AddRange(recognizedLines);
                previousBottom = prepared.SourceBottom;
                previousHeight = currentHeight;
                previousWidth = prepared.Width;
            }

            var recognitionElapsed = stopwatch.Elapsed;

            _lastLines = lines.ToArray();
            if (recognizedBands is not null)
            {
                _lastRecognizedBands = recognizedBands;
            }
            _lastFingerprint = fingerprint;
            _hasCachedRecognition = true;

            return new OcrRecognitionResult(
                _lastLines,
                preprocessingElapsed,
                recognitionElapsed,
                WasCached: everyBandWasCached);
        }
        finally
        {
            _recognitionGate.Release();
        }
    }

    public void Dispose()
    {
        _recognitionGate.Wait();
        try
        {
            _disposed = true;
        }
        finally
        {
            // Keep the tiny SemaphoreSlim alive so a caller already queued in WaitAsync can
            // wake, observe _disposed, and exit without racing ObjectDisposedException.
            _recognitionGate.Release();
        }
    }

    private static ulong CombineFingerprints(IReadOnlyList<PreparedFrame> frames)
    {
        var combined = 14695981039346656037UL;
        foreach (var frame in frames)
        {
            combined ^= frame.ContentFingerprint;
            combined *= 1099511628211UL;
            combined ^= (ulong)frame.Width;
            combined *= 1099511628211UL;
            combined ^= (ulong)frame.Height;
            combined *= 1099511628211UL;
            combined ^= (ulong)(frame.SourceTop + 1);
            combined *= 1099511628211UL;
            combined ^= (ulong)(frame.SourceBottom + 1);
            combined *= 1099511628211UL;
            combined ^= frame.IsFallback ? 1UL : 0UL;
            combined *= 1099511628211UL;
        }

        combined ^= (ulong)frames.Count;
        return combined;
    }

    private static void AddLogicalBoundary(List<string> lines)
    {
        if (lines.Count > 0 && lines[^1].Length > 0)
        {
            lines.Add(string.Empty);
        }
    }

    private static IReadOnlyList<OcrPhysicalLine> CreatePhysicalLineMap(
        IReadOnlyList<string> lines,
        IReadOnlyList<WindowsRecognizedBand> recognizedBands)
    {
        var result = new List<OcrPhysicalLine>();
        var searchStart = 0;
        foreach (var projection in CreateStructuredBandProjection(recognizedBands))
        {
            foreach (var bandLine in projection.Lines)
            {
                var lineIndex = -1;
                for (var index = searchStart; index < lines.Count; index++)
                {
                    if (string.Equals(lines[index], bandLine, StringComparison.Ordinal))
                    {
                        lineIndex = index;
                        break;
                    }
                }

                if (lineIndex < 0)
                {
                    continue;
                }

                result.Add(new OcrPhysicalLine(
                    lineIndex,
                    CreatePhysicalBandId(projection.Frame),
                    projection.Frame.SourceTop,
                    projection.Frame.SourceBottom));
                searchStart = lineIndex + 1;
            }
        }

        return result;
    }

    private static IReadOnlyList<string> CreateStructuredLogicalLines(
        IReadOnlyList<WindowsRecognizedBand> recognizedBands,
        int roiWidth)
    {
        var lines = new List<string>();
        int? previousBottom = null;
        var previousHeight = 0;
        var previousWidth = 0;
        foreach (var projection in CreateStructuredBandProjection(recognizedBands))
        {
            var prepared = projection.Frame;
            if (prepared.IsFallback)
            {
                AddLogicalBoundary(lines);
                foreach (var recognizedLine in projection.Lines)
                {
                    lines.Add(recognizedLine);
                    AddLogicalBoundary(lines);
                }

                previousBottom = null;
                previousHeight = 0;
                previousWidth = 0;
                continue;
            }

            var currentHeight = prepared.SourceBottom - prepared.SourceTop + 1;
            if (previousBottom is not null)
            {
                var gap = prepared.SourceTop - previousBottom.Value - 1;
                var closeEnoughForWrapping = gap <= Math.Max(12, Math.Max(previousHeight, currentHeight));
                var previousLineFilledTooltip = previousWidth >= roiWidth * 0.70;
                if (!closeEnoughForWrapping || !previousLineFilledTooltip)
                {
                    AddLogicalBoundary(lines);
                }
            }

            lines.AddRange(projection.Lines);
            previousBottom = prepared.SourceBottom;
            previousHeight = currentHeight;
            previousWidth = prepared.Width;
        }

        return lines;
    }

    private static IReadOnlyList<StructuredBandProjection> CreateStructuredBandProjection(
        IReadOnlyList<WindowsRecognizedBand> recognizedBands)
    {
        var regularCanonical = recognizedBands
            .Where(static band => !band.Frame.IsFallback)
            .SelectMany(static band => band.Lines)
            .Select(static line => AffixCanonicalizer.Normalize(line).Text)
            .Where(static line => !string.IsNullOrWhiteSpace(line))
            .ToHashSet(StringComparer.Ordinal);
        var projections = new List<StructuredBandProjection>();
        foreach (var band in recognizedBands)
        {
            var lines = band.Lines
                .Where(static line => !string.IsNullOrWhiteSpace(line))
                .Where(line => !band.Frame.IsFallback ||
                               !regularCanonical.Contains(AffixCanonicalizer.Normalize(line).Text))
                .ToArray();
            if (lines.Length > 0)
            {
                projections.Add(new StructuredBandProjection(band.Frame, lines));
            }
        }

        return projections;
    }

    private static string CreatePhysicalBandId(PreparedFrame band) =>
        $"win:{band.ContentFingerprint:x16}:{band.SourceTop}:" +
        $"{band.SourceBottom}:{band.SourceLeft}:{band.SourceRight}";

    private sealed record StructuredBandProjection(
        PreparedFrame Frame,
        IReadOnlyList<string> Lines);

    private void CacheBandRecognition(BandCacheKey key, string[] lines)
    {
        if (_bandRecognitionCache.ContainsKey(key))
        {
            return;
        }

        _bandRecognitionCache.Add(key, lines);
        _bandCacheInsertionOrder.Enqueue(key);
        while (_bandRecognitionCache.Count > MaximumCachedBands &&
               _bandCacheInsertionOrder.TryDequeue(out var oldest))
        {
            _bandRecognitionCache.Remove(oldest);
        }
    }

    private static bool MatchesLanguage(string languageTag, OcrRecognitionLanguage language) =>
        language switch
        {
            OcrRecognitionLanguage.English =>
                languageTag.StartsWith("en", StringComparison.OrdinalIgnoreCase),
            OcrRecognitionLanguage.TraditionalChinese =>
                languageTag.Equals("zh-TW", StringComparison.OrdinalIgnoreCase) ||
                languageTag.StartsWith("zh-Hant", StringComparison.OrdinalIgnoreCase),
            _ => false,
        };

    private readonly record struct BandCacheKey(ulong Fingerprint, int Width, int Height);
}

internal sealed record WindowsRecognizedBand(
    PreparedFrame Frame,
    IReadOnlyList<string> Lines);
