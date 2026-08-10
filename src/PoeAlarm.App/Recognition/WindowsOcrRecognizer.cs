using System.Diagnostics;
using System.Runtime.InteropServices.WindowsRuntime;
using Windows.Globalization;
using Windows.Graphics.Imaging;
using Windows.Media.Ocr;

namespace PoeAlarm.App.Recognition;

public sealed class WindowsOcrRecognizer : IOcrRecognizer
{
    private readonly PoeTextPreprocessor _preprocessor;
    private readonly OcrEngine _engine;
    private readonly SemaphoreSlim _recognitionGate = new(1, 1);
    private readonly int _scale;
    private IReadOnlyList<string> _lastLines = [];
    private ulong _lastFingerprint;
    private DateTimeOffset _lastRecognitionAt;
    private bool _hasCachedRecognition;
    private bool _disposed;

    public WindowsOcrRecognizer(PoeTextPreprocessor? preprocessor = null, int scale = 2)
    {
        _preprocessor = preprocessor ?? new PoeTextPreprocessor();
        _scale = scale;
        var englishLanguage = OcrEngine.AvailableRecognizerLanguages.FirstOrDefault(language =>
            language.LanguageTag.StartsWith("en", StringComparison.OrdinalIgnoreCase));
        _engine = englishLanguage is null
            ? throw new InvalidOperationException(
                "Windows English OCR is unavailable. Install the English OCR language capability in Windows Settings.")
            : OcrEngine.TryCreateFromLanguage(englishLanguage)
              ?? throw new InvalidOperationException(
                  $"Windows could not initialize its English OCR engine ({englishLanguage.LanguageTag}).");
    }

    public async Task<OcrRecognitionResult> RecognizeAsync(
        Capture.CapturedFrame frame,
        CancellationToken cancellationToken = default)
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

            if (_hasCachedRecognition &&
                fingerprint == _lastFingerprint &&
                DateTimeOffset.UtcNow - _lastRecognitionAt < TimeSpan.FromMilliseconds(75))
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
            int? previousBottom = null;
            var previousHeight = 0;
            var previousWidth = 0;
            foreach (var prepared in preparedBands)
            {
                cancellationToken.ThrowIfCancellationRequested();
                using var bitmap = SoftwareBitmap.CreateCopyFromBuffer(
                    prepared.BgraPixels.AsBuffer(0, prepared.ByteLength),
                    BitmapPixelFormat.Bgra8,
                    prepared.Width,
                    prepared.Height,
                    BitmapAlphaMode.Premultiplied);

                var result = await _engine.RecognizeAsync(bitmap).AsTask(cancellationToken).ConfigureAwait(false);
                var recognizedLines = result.Lines
                    .Select(line => line.Text.Trim())
                    .Where(line => line.Length > 0)
                    .ToArray();
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
            _lastFingerprint = fingerprint;
            _lastRecognitionAt = DateTimeOffset.UtcNow;
            _hasCachedRecognition = true;

            return new OcrRecognitionResult(_lastLines, preprocessingElapsed, recognitionElapsed);
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
}
