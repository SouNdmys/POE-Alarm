using System.Diagnostics;
using System.Runtime.InteropServices.WindowsRuntime;
using PoeAlarm.App.Capture;
using PoeAlarm.Core.Matching;
using Windows.Foundation;
using Windows.Globalization;
using Windows.Graphics.Imaging;
using Windows.Media.Ocr;

namespace PoeAlarm.App.Recognition;

/// <summary>
/// Low-latency Chinese path for POE tooltips. Unlike Paddle's packaged recognition-only model,
/// Windows OCR performs its own multi-line layout analysis. Feeding it one combined blue-text
/// mask therefore remains correct when unrelated stash icons bridge several compact affix rows.
/// This class is deliberately separate from <see cref="WindowsOcrRecognizer"/> so the proven
/// English path and its row/caching behavior cannot be changed by Chinese-specific experiments.
/// </summary>
public sealed class WindowsChineseOcrRecognizer : IOcrRecognizer, IFrameFingerprintProvider
{
    private readonly PoeTextPreprocessor _preprocessor;
    private readonly PoeTextPreprocessor _paddleFallbackPreprocessor = new(
        new PoeTextPreprocessorSettings(
            MinimumBlue: 100,
            MinimumBlueDominance: 14,
            MaximumWarmChannelDifference: 72));
    private readonly OcrEngine _engine;
    private readonly SemaphoreSlim _recognitionGate = new(1, 1);

    private CapturedFrame? _prefetchedSourceFrame;
    private PreparedFrame? _prefetchedFrame;
    private byte[] _refinementBuffer = [];
    private PaddleCtcSession? _paddleFallbackSession;
    private IReadOnlyList<string> _lastLines = [];
    private LogicalAffixMatch? _lastAssistedMatch;
    private string _lastTargetKey = string.Empty;
    private ulong _lastFingerprint;
    private bool _hasCachedRecognition;
    private BatchCache? _batchCache;
    private long _batchPrimaryPassCount;
    private long _batchLocalizedWorkUnitCount;
    private bool _disposed;

    public WindowsChineseOcrRecognizer(PoeTextPreprocessor? preprocessor = null)
    {
        _preprocessor = preprocessor ?? new PoeTextPreprocessor();
        var installedLanguage = FindBestInstalledChineseLanguage();
        _engine = installedLanguage is null
            ? throw new InvalidOperationException(
                "Windows 中文 OCR 不可用。请在 Windows 语言设置中安装繁體中文（台湾）或简体中文 OCR 语言功能。")
            : OcrEngine.TryCreateFromLanguage(installedLanguage)
              ?? throw new InvalidOperationException(
                  $"Windows 无法初始化中文 OCR（{installedLanguage.LanguageTag}）。");
    }

    public string RecognizerLanguageTag => _engine.RecognizerLanguage.LanguageTag;

    public StructuredRuleOcrSupport StructuredRuleSupport =>
        StructuredRuleOcrSupport.StrictBatch;

    internal long BatchPrimaryPassCount => _batchPrimaryPassCount;

    internal long BatchLocalizedWorkUnitCount => _batchLocalizedWorkUnitCount;

    public ulong ComputeFrameFingerprint(CapturedFrame frame)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        ArgumentNullException.ThrowIfNull(frame);

        // AffixMonitor needs a semantic fingerprint before deciding whether to arm the mouse
        // guard. Keep the exact mask for the immediately following OCR call so the large ROI is
        // never walked and copied twice.
        var prepared = _preprocessor.PrepareBlueAffixText(frame, scale: 1);
        _prefetchedSourceFrame = frame;
        _prefetchedFrame = prepared;
        return CombineFingerprint(prepared, frame.Width, frame.Height);
    }

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default) =>
        RecognizeCoreAsync(frame, target: null, cancellationToken);

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(target);
        return RecognizeCoreAsync(frame, target, cancellationToken);
    }

    public async Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        IReadOnlyList<FullLineAffixMatcher> targets,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(frame);
        var targetSet = Poe2EnglishRecognizer.NormalizeTargetSet(targets);
        if (targetSet.Count == 0)
        {
            return await RecognizeAsync(frame, cancellationToken).ConfigureAwait(false);
        }

        await _recognitionGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            var stopwatch = Stopwatch.StartNew();
            PreparedFrame prepared;
            if (ReferenceEquals(frame, _prefetchedSourceFrame) && _prefetchedFrame is not null)
            {
                prepared = _prefetchedFrame;
                _prefetchedSourceFrame = null;
                _prefetchedFrame = null;
            }
            else
            {
                prepared = _preprocessor.PrepareBlueAffixText(frame, scale: 1);
            }

            var preprocessingElapsed = stopwatch.Elapsed;
            var fingerprint = CombineFingerprint(prepared, frame.Width, frame.Height);
            var targetKey = Poe2EnglishRecognizer.CreateTargetSetKey(targetSet);
            if (_batchCache is { } cached &&
                cached.Fingerprint == fingerprint &&
                string.Equals(cached.TargetKey, targetKey, StringComparison.Ordinal))
            {
                return cached.Result with
                {
                    PreprocessingElapsed = preprocessingElapsed,
                    RecognitionElapsed = TimeSpan.Zero,
                    WasCached = true,
                };
            }

            if (prepared.Width > OcrEngine.MaxImageDimension ||
                prepared.Height > OcrEngine.MaxImageDimension)
            {
                throw new InvalidOperationException(
                    $"繁中识别区域超过 Windows OCR 的 {OcrEngine.MaxImageDimension}px 限制；请框选更小的装备提示区域。");
            }

            stopwatch.Restart();
            using var bitmap = SoftwareBitmap.CreateCopyFromBuffer(
                prepared.BgraPixels.AsBuffer(0, prepared.ByteLength),
                BitmapPixelFormat.Bgra8,
                prepared.Width,
                prepared.Height,
                BitmapAlphaMode.Premultiplied);
            var recognized = await _engine
                .RecognizeAsync(bitmap)
                .AsTask(cancellationToken)
                .ConfigureAwait(false);
            _batchPrimaryPassCount++;
            var detectedLines = recognized.Lines
                .Where(line => line.Words.Count > 0)
                .ToArray();
            var projection = BuildLogicalProjection(detectedLines, prepared);
            var assisted = new List<OcrAssistedObservation>();
            var candidateEvidence = new List<OcrCandidateEvidence>();

            // Windows supplies all candidate bounds from one shared full-frame layout pass. A
            // batch spends no more than two localized recovery work units regardless of target
            // count, ordered by edit distance and then physical position.
            var jobs = targetSet
                .Where(target => !target.TryFindMatch(projection.Lines, out _))
                .SelectMany(target => FindRefinementCandidates(detectedLines, target)
                    .Select(candidate => new BatchRefinementJob(target, candidate)))
                .OrderBy(job => job.Candidate.EditDistance)
                .ThenBy(job => job.Candidate.LineCount)
                .ThenBy(job => job.Candidate.StartLine)
                .GroupBy(job => CreateDetectedBandId(
                    detectedLines,
                    job.Candidate.StartLine,
                    job.Candidate.LineCount,
                    prepared.ContentFingerprint))
                .Select(static group => new BatchRefinementBandJob(
                    group.First().Candidate,
                    group.Select(static item => item.Target).ToArray()))
                .ToArray();
            var plan = BoundedLocalizedRecoveryPlanner.Plan(jobs, maximumWorkUnits: 2);
            foreach (var job in plan.WorkItems)
            {
                _batchLocalizedWorkUnitCount++;
                var recovery = await RefineCandidateBatchAsync(
                        frame,
                        prepared,
                        job.Targets,
                        job.Candidate,
                        cancellationToken)
                    .ConfigureAwait(false);
                var bandId = CreateDetectedBandId(
                    detectedLines,
                    job.Candidate.StartLine,
                    job.Candidate.LineCount,
                    prepared.ContentFingerprint);
                var relatedBandIds = Enumerable.Range(
                        job.Candidate.StartLine,
                        job.Candidate.LineCount)
                    .Select(index =>
                    {
                        var bounds = GetBounds(detectedLines[index]);
                        return CreateBandId(
                            prepared.ContentFingerprint,
                            (int)Math.Floor(bounds.Top),
                            (int)Math.Ceiling(bounds.Bottom));
                    })
                    .ToArray();
                foreach (var verified in recovery.Verified)
                {
                    assisted.Add(new OcrAssistedObservation(
                        bandId,
                        verified.ObservedText,
                        verified.Target.Template.Text,
                        (int)Math.Floor(job.Candidate.Bounds.Top),
                        (int)Math.Ceiling(job.Candidate.Bounds.Bottom),
                        relatedBandIds));
                }

                var observed = string.Join(' ', detectedLines
                    .Skip(job.Candidate.StartLine)
                    .Take(job.Candidate.LineCount)
                    .Select(line => NormalizeChineseNumericDashes(line.Text.Trim())));
                foreach (var unresolved in recovery.Unresolved)
                {
                    candidateEvidence.Add(new OcrCandidateEvidence(
                        bandId,
                        observed,
                        unresolved.Template.Text,
                        ClassifyCandidateEvidence(observed, unresolved),
                        (int)Math.Floor(job.Candidate.Bounds.Top),
                        (int)Math.Ceiling(job.Candidate.Bounds.Bottom)));
                }
            }

            // Keep the expensive refinement ceiling independent of target count, while preserving
            // every remaining conservative locator as unresolved evidence for Guarded yellow-stop.
            foreach (var job in plan.DeferredItems)
            {
                var bandId = CreateDetectedBandId(
                    detectedLines,
                    job.Candidate.StartLine,
                    job.Candidate.LineCount,
                    prepared.ContentFingerprint);
                var observed = string.Join(' ', detectedLines
                    .Skip(job.Candidate.StartLine)
                    .Take(job.Candidate.LineCount)
                    .Select(line => NormalizeChineseNumericDashes(line.Text.Trim())));
                var bounds = job.Candidate.Bounds;
                foreach (var target in job.Targets)
                {
                    candidateEvidence.Add(new OcrCandidateEvidence(
                        bandId,
                        observed,
                        target.Template.Text,
                        OcrCandidateEvidenceKind.LocalizedLexicalCandidate,
                        (int)Math.Floor(bounds.Top),
                        (int)Math.Ceiling(bounds.Bottom)));
                }
            }

            var result = new OcrRecognitionResult(
                projection.Lines,
                preprocessingElapsed,
                stopwatch.Elapsed,
                WasCached: false,
                TargetAssistedMatch: null,
                RequiresRescan: false,
                AssistedObservations: assisted,
                PhysicalLines: projection.PhysicalLines,
                CandidateEvidence: candidateEvidence);
            _batchCache = new BatchCache(fingerprint, targetKey, result);
            return result;
        }
        finally
        {
            _recognitionGate.Release();
        }
    }

    private async Task<OcrRecognitionResult> RecognizeCoreAsync(
        CapturedFrame frame,
        FullLineAffixMatcher? target,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(frame);
        await _recognitionGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            var stopwatch = Stopwatch.StartNew();
            PreparedFrame prepared;
            if (ReferenceEquals(frame, _prefetchedSourceFrame) && _prefetchedFrame is not null)
            {
                prepared = _prefetchedFrame;
                _prefetchedSourceFrame = null;
                _prefetchedFrame = null;
            }
            else
            {
                prepared = _preprocessor.PrepareBlueAffixText(frame, scale: 1);
            }

            var preprocessingElapsed = stopwatch.Elapsed;
            var fingerprint = CombineFingerprint(prepared, frame.Width, frame.Height);
            var targetKey = target?.Template.Text ?? string.Empty;
            if (_hasCachedRecognition &&
                fingerprint == _lastFingerprint &&
                string.Equals(targetKey, _lastTargetKey, StringComparison.Ordinal))
            {
                return new OcrRecognitionResult(
                    _lastLines,
                    preprocessingElapsed,
                    TimeSpan.Zero,
                    WasCached: true,
                    _lastAssistedMatch);
            }

            if (prepared.Width > OcrEngine.MaxImageDimension ||
                prepared.Height > OcrEngine.MaxImageDimension)
            {
                throw new InvalidOperationException(
                    $"繁中识别区域超过 Windows OCR 的 {OcrEngine.MaxImageDimension}px 限制；请框选更小的装备提示区域。");
            }

            stopwatch.Restart();
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
            var detectedLines = result.Lines
                .Where(line => line.Words.Count > 0)
                .ToArray();
            var lines = BuildLogicalLines(detectedLines);
            LogicalAffixMatch? assistedMatch = null;
            if (target is not null && !target.TryFindMatch(lines, out _))
            {
                assistedMatch = await TryRefineTargetAsync(
                        frame,
                        prepared,
                        detectedLines,
                        target,
                        cancellationToken)
                    .ConfigureAwait(false);
            }

            var recognitionElapsed = stopwatch.Elapsed;

            _lastLines = lines;
            _lastFingerprint = fingerprint;
            _lastTargetKey = targetKey;
            _lastAssistedMatch = assistedMatch;
            _hasCachedRecognition = true;
            return new OcrRecognitionResult(
                _lastLines,
                preprocessingElapsed,
                recognitionElapsed,
                TargetAssistedMatch: assistedMatch);
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
            _prefetchedSourceFrame = null;
            _prefetchedFrame = null;
            _refinementBuffer = [];
            _paddleFallbackSession?.Dispose();
            _paddleFallbackSession = null;
        }
        finally
        {
            _recognitionGate.Release();
        }
    }

    private static Language? FindBestInstalledChineseLanguage()
    {
        var installed = OcrEngine.AvailableRecognizerLanguages;
        return installed.FirstOrDefault(language =>
                   language.LanguageTag.Equals("zh-TW", StringComparison.OrdinalIgnoreCase) ||
                   language.LanguageTag.StartsWith("zh-Hant", StringComparison.OrdinalIgnoreCase))
               ?? installed.FirstOrDefault(language =>
                   language.LanguageTag.StartsWith("zh-Hans", StringComparison.OrdinalIgnoreCase) ||
                   language.LanguageTag.StartsWith("zh-CN", StringComparison.OrdinalIgnoreCase));
    }

    private async Task<LogicalAffixMatch?> TryRefineTargetAsync(
        CapturedFrame source,
        PreparedFrame prepared,
        IReadOnlyList<OcrLine> detectedLines,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken)
    {
        if (target.Template.Tokens.Count < 2 || detectedLines.Count == 0)
        {
            return null;
        }

        var candidates = new List<RefinementCandidate>();
        var editAllowance = target.Template.Tokens.Count <= 8
            ? Math.Min(4, target.Template.Tokens.Count)
            : Math.Clamp((target.Template.Tokens.Count + 1) / 2, 4, 8);
        var maximumCandidateAttempts = target.Template.Tokens.Count >= 10 ? 4 : 2;
        for (var start = 0; start < detectedLines.Count; start++)
        {
            var maximumSpan = Math.Min(
                target.MaximumLineSpan,
                detectedLines.Count - start);
            var text = string.Empty;
            var bounds = new Rect();
            for (var span = 1; span <= maximumSpan; span++)
            {
                var line = detectedLines[start + span - 1];
                if (span > 1 && HasLogicalBoundary(detectedLines[start + span - 2], line))
                {
                    break;
                }

                var normalizedLine = NormalizeChineseNumericDashes(line.Text.Trim());
                text = text.Length == 0 ? normalizedLine : $"{text} {normalizedLine}";
                bounds = span == 1 ? GetBounds(line) : Union(bounds, GetBounds(line));
                var canonical = AffixCanonicalizer.Normalize(text);
                var distance = TokenEditDistance(
                    target.Template.Tokens,
                    canonical.Tokens,
                    maximum: editAllowance);
                if (distance is >= 1 && distance <= editAllowance)
                {
                    candidates.Add(new RefinementCandidate(start, span, bounds, distance));
                }
            }
        }

        // Refinement is verification, never fuzzy acceptance: the original colour crop must pass
        // the normal strict matcher. Limit attempts so an unrelated screen cannot turn into an
        // expensive full-page OCR sweep.
        foreach (var candidate in candidates
                     .OrderBy(item => item.EditDistance)
                     .ThenBy(item => item.LineCount)
                     .ThenBy(item => item.StartLine)
                     .Take(maximumCandidateAttempts))
        {
            cancellationToken.ThrowIfCancellationRequested();
            using var crop = CreateOriginalCrop(
                source,
                prepared,
                candidate.Bounds,
                out var cropFrame);
            var refined = await _engine
                .RecognizeAsync(crop)
                .AsTask(cancellationToken)
                .ConfigureAwait(false);
            var refinedLines = BuildLogicalLines(refined.Lines);
            if (!target.TryFindMatch(refinedLines, out var strictMatch) || strictMatch is null)
            {
                var paddleMatch = TryPaddleFallback(cropFrame, target, candidate.StartLine);
                if (paddleMatch is not null)
                {
                    return paddleMatch;
                }

                continue;
            }

            return new LogicalAffixMatch(
                candidate.StartLine,
                strictMatch.PhysicalLineCount,
                strictMatch.OriginalText,
                strictMatch.CanonicalText);
        }

        return null;
    }

    private static IReadOnlyList<RefinementCandidate> FindRefinementCandidates(
        IReadOnlyList<OcrLine> detectedLines,
        FullLineAffixMatcher target)
    {
        if (target.Template.Tokens.Count < 2 || detectedLines.Count == 0)
        {
            return [];
        }

        var candidates = new Dictionary<(int Start, int Span), RefinementCandidate>();
        var editAllowance = target.Template.Tokens.Count <= 8
            ? Math.Min(4, target.Template.Tokens.Count)
            : Math.Clamp((target.Template.Tokens.Count + 1) / 2, 4, 8);
        for (var start = 0; start < detectedLines.Count; start++)
        {
            var maximumSpan = Math.Min(target.MaximumLineSpan, detectedLines.Count - start);
            var text = string.Empty;
            var bounds = new Rect();
            for (var span = 1; span <= maximumSpan; span++)
            {
                var line = detectedLines[start + span - 1];
                if (span > 1 && HasLogicalBoundary(detectedLines[start + span - 2], line))
                {
                    break;
                }

                var normalizedLine = NormalizeChineseNumericDashes(line.Text.Trim());
                text = text.Length == 0 ? normalizedLine : $"{text} {normalizedLine}";
                bounds = span == 1 ? GetBounds(line) : Union(bounds, GetBounds(line));
                var canonical = AffixCanonicalizer.Normalize(text);
                var distance = TokenEditDistance(target.Template.Tokens, canonical.Tokens, editAllowance);
                if (distance is >= 1 && distance <= editAllowance)
                {
                    var key = (start, span);
                    var candidate = new RefinementCandidate(start, span, bounds, distance);
                    if (!candidates.TryGetValue(key, out var existing) ||
                        candidate.EditDistance < existing.EditDistance)
                    {
                        candidates[key] = candidate;
                    }
                }
            }
        }

        return candidates.Values.ToArray();
    }

    private async Task<LogicalAffixMatch?> RefineCandidateAsync(
        CapturedFrame source,
        PreparedFrame prepared,
        FullLineAffixMatcher target,
        RefinementCandidate candidate,
        CancellationToken cancellationToken)
    {
        using var crop = CreateOriginalCrop(source, prepared, candidate.Bounds, out var cropFrame);
        var refined = await _engine
            .RecognizeAsync(crop)
            .AsTask(cancellationToken)
            .ConfigureAwait(false);
        var refinedLines = BuildLogicalLines(refined.Lines);
        if (target.TryFindMatch(refinedLines, out var strictMatch) && strictMatch is not null)
        {
            return new LogicalAffixMatch(
                candidate.StartLine,
                strictMatch.PhysicalLineCount,
                strictMatch.OriginalText,
                strictMatch.CanonicalText);
        }

        return TryPaddleFallback(cropFrame, target, candidate.StartLine);
    }

    private async Task<BatchRefinementResult> RefineCandidateBatchAsync(
        CapturedFrame source,
        PreparedFrame prepared,
        IReadOnlyList<FullLineAffixMatcher> targets,
        RefinementCandidate candidate,
        CancellationToken cancellationToken)
    {
        using var crop = CreateOriginalCrop(source, prepared, candidate.Bounds, out var cropFrame);
        var refined = await _engine
            .RecognizeAsync(crop)
            .AsTask(cancellationToken)
            .ConfigureAwait(false);
        var refinedLines = BuildLogicalLines(refined.Lines);
        var verified = new List<BatchVerifiedTarget>();
        var unresolved = new List<FullLineAffixMatcher>();
        foreach (var target in targets)
        {
            if (target.TryFindMatch(refinedLines, out var strict) && strict is not null)
            {
                verified.Add(new BatchVerifiedTarget(target, strict.OriginalText));
            }
            else
            {
                unresolved.Add(target);
            }
        }

        if (unresolved.Count == 0)
        {
            return new BatchRefinementResult(verified, []);
        }

        // One Paddle inference supplies logits for every still-relevant target. This preserves
        // the same independent localized verifier as Quick mode without scaling OCR work by N.
        var bands = _paddleFallbackPreprocessor.PrepareBlueAffixBands(cropFrame, scale: 1)
            .Where(static item => !item.IsFallback)
            .Take(2)
            .ToArray();
        if (bands.Length == 1)
        {
            var batch = GetPaddleFallbackSession().RecognizeBatch(bands[0], unresolved);
            var remaining = new List<FullLineAffixMatcher>();
            foreach (var target in unresolved)
            {
                var accepted = target.IsMatch(batch.Recognition.Text) ||
                               batch.TargetSupports.TryGetValue(
                                   target.Template.Text,
                                   out var support) && support.StronglySupported;
                if (accepted)
                {
                    verified.Add(new BatchVerifiedTarget(target, batch.Recognition.Text));
                }
                else
                {
                    remaining.Add(target);
                }
            }

            unresolved = remaining;
        }

        return new BatchRefinementResult(verified, unresolved);
    }

    private SoftwareBitmap CreateOriginalCrop(
        CapturedFrame source,
        PreparedFrame prepared,
        Rect detectedBounds,
        out CapturedFrame cropFrame)
    {
        // Windows sometimes drops the final Traditional glyph and consequently reports a word
        // box that ends one full character early (for example 力量 -> 力). Preserve enough source
        // context for the independent colour/Paddle verification to recover that unseen glyph.
        const int horizontalMargin = 40;
        const int verticalMargin = 8;
        // Chinese combined masks are prepared at scale 1. SourceTop denotes the logical glyph
        // top, while LogicalContentTop is its offset inside the crop (the quiet top margin).
        var preparedCropTop = prepared.SourceTop - prepared.LogicalContentTop;
        var left = Math.Clamp(
            prepared.SourceLeft + (int)Math.Floor(detectedBounds.Left) - horizontalMargin,
            0,
            source.Width - 1);
        var top = Math.Clamp(
            preparedCropTop + (int)Math.Floor(detectedBounds.Top) - verticalMargin,
            0,
            source.Height - 1);
        var right = Math.Clamp(
            prepared.SourceLeft + (int)Math.Ceiling(detectedBounds.Right) + horizontalMargin,
            left,
            source.Width - 1);
        var bottom = Math.Clamp(
            preparedCropTop + (int)Math.Ceiling(detectedBounds.Bottom) + verticalMargin,
            top,
            source.Height - 1);
        var width = right - left + 1;
        var height = bottom - top + 1;
        var stride = checked(width * 4);
        var byteLength = checked(stride * height);
        if (_refinementBuffer.Length < byteLength)
        {
            _refinementBuffer = new byte[byteLength];
        }

        for (var row = 0; row < height; row++)
        {
            Buffer.BlockCopy(
                source.BgraPixels,
                ((top + row) * source.Stride) + (left * 4),
                _refinementBuffer,
                row * stride,
                stride);
        }

        cropFrame = new CapturedFrame(
            width,
            height,
            stride,
            _refinementBuffer,
            source.CapturedAt);

        return SoftwareBitmap.CreateCopyFromBuffer(
            _refinementBuffer.AsBuffer(0, byteLength),
            BitmapPixelFormat.Bgra8,
            width,
            height,
            BitmapAlphaMode.Premultiplied);
    }

    private LogicalAffixMatch? TryPaddleFallback(
        CapturedFrame crop,
        FullLineAffixMatcher target,
        int sourceLineIndex)
    {
        var bands = _paddleFallbackPreprocessor.PrepareBlueAffixBands(crop, scale: 1);
        if (bands.Count == 0)
        {
            return null;
        }

        var session = GetPaddleFallbackSession();
        var assistedCandidates = 0;
        string? assistedText = null;
        foreach (var band in bands.Where(item => !item.IsFallback).Take(4))
        {
            var recognition = PaddleTargetRecovery.RecognizePrimary(session, band, target);
            if (target.IsMatch(recognition.Primary.Text))
            {
                return new LogicalAffixMatch(
                    sourceLineIndex,
                    PhysicalLineCount: 1,
                    recognition.Primary.Text,
                    target.Template.Text);
            }

            if (recognition.AssistanceKind == PaddleTargetAssistanceKind.CtcTargetSupport &&
                !string.IsNullOrWhiteSpace(recognition.AssistedText))
            {
                assistedCandidates++;
                assistedText = recognition.AssistedText;
            }
        }

        // A target-conditioned CTC substitution is accepted only after Windows independently
        // localized one of a small bounded set of close text candidates and Paddle found exactly one
        // strongly supported row inside that crop. This retains the old recognizer's
        // single-candidate safety rule without running its progressive full-ROI recovery pipeline.
        return assistedCandidates == 1 && assistedText is not null
            ? new LogicalAffixMatch(
                sourceLineIndex,
                PhysicalLineCount: 1,
                assistedText,
                target.Template.Text)
            : null;
    }

    private PaddleCtcSession GetPaddleFallbackSession()
    {
        if (_paddleFallbackSession is not null)
        {
            return _paddleFallbackSession;
        }

        var assets = PackagedPaddleOcrAssets.Load();
        _paddleFallbackSession = new PaddleCtcSession(
            assets.ModelBytes,
            assets.Dictionary,
            threads: 2,
            polarity: TextPolarity.BrightOnDark,
            inkMargin: 2,
            rightPadding: 8);
        return _paddleFallbackSession;
    }

    private static int TokenEditDistance(
        IReadOnlyList<AffixToken> expected,
        IReadOnlyList<AffixToken> actual,
        int maximum)
    {
        if (Math.Abs(expected.Count - actual.Count) > maximum)
        {
            return maximum + 1;
        }

        var previous = Enumerable.Range(0, actual.Count + 1).ToArray();
        var current = new int[actual.Count + 1];
        for (var row = 1; row <= expected.Count; row++)
        {
            current[0] = row;
            var rowMinimum = current[0];
            for (var column = 1; column <= actual.Count; column++)
            {
                var substitution = expected[row - 1].Kind == actual[column - 1].Kind &&
                                   string.Equals(
                                       expected[row - 1].Text,
                                       actual[column - 1].Text,
                                       StringComparison.Ordinal)
                    ? 0
                    : 1;
                current[column] = Math.Min(
                    Math.Min(current[column - 1] + 1, previous[column] + 1),
                    previous[column - 1] + substitution);
                rowMinimum = Math.Min(rowMinimum, current[column]);
            }

            if (rowMinimum > maximum)
            {
                return maximum + 1;
            }

            (previous, current) = (current, previous);
        }

        return previous[^1];
    }

    private static OcrCandidateEvidenceKind ClassifyCandidateEvidence(
        string observed,
        FullLineAffixMatcher target)
    {
        var expectedNumeric = target.Template.Tokens.Count(static token =>
            token.Kind != AffixTokenKind.Word);
        var actualNumeric = AffixCanonicalizer.Normalize(observed).Tokens.Count(static token =>
            token.Kind != AffixTokenKind.Word);
        if (actualNumeric < expectedNumeric)
        {
            return OcrCandidateEvidenceKind.MissingNumericValue;
        }

        return actualNumeric > expectedNumeric
            ? OcrCandidateEvidenceKind.ConflictingNumericValue
            : OcrCandidateEvidenceKind.LocalizedLexicalCandidate;
    }

    private static string NormalizeChineseNumericDashes(string text)
    {
        var firstCandidate = text.IndexOf('一');
        if (firstCandidate < 0)
        {
            return text;
        }

        var characters = text.ToCharArray();
        var changed = false;
        for (var index = firstCandidate; index < characters.Length; index++)
        {
            if (characters[index] != '一')
            {
                continue;
            }

            var previous = index - 1;
            while (previous >= 0 && char.IsWhiteSpace(characters[previous]))
            {
                previous--;
            }

            var next = index + 1;
            while (next < characters.Length && char.IsWhiteSpace(characters[next]))
            {
                next++;
            }

            if (previous >= 0 && next < characters.Length &&
                char.IsDigit(characters[previous]) && char.IsDigit(characters[next]))
            {
                characters[index] = '-';
                changed = true;
            }
        }

        return changed ? new string(characters) : text;
    }

    private static string[] BuildLogicalLines(IReadOnlyList<OcrLine> detectedLines)
    {
        var lines = new List<string>(detectedLines.Count * 2);
        OcrLine? previous = null;
        foreach (var detectedLine in detectedLines)
        {
            var text = NormalizeChineseNumericDashes(detectedLine.Text.Trim());
            if (text.Length == 0)
            {
                continue;
            }

            if (previous is not null && HasLogicalBoundary(previous, detectedLine))
            {
                // FullLineAffixMatcher treats an empty physical line as a hard span boundary.
                // Windows OCR omits whitespace-only rows, so reconstruct only unmistakably large
                // spatial gaps here. Normal wrapping and tightly stacked POE modifiers remain
                // adjacent; distant blue UI text cannot be concatenated into one target.
                lines.Add(string.Empty);
            }

            lines.Add(text);
            previous = detectedLine;
        }

        return lines.ToArray();
    }

    private static LogicalProjection BuildLogicalProjection(
        IReadOnlyList<OcrLine> detectedLines,
        PreparedFrame prepared)
    {
        var lines = new List<string>(detectedLines.Count * 2);
        var physical = new List<OcrPhysicalLine>(detectedLines.Count);
        OcrLine? previous = null;
        foreach (var detectedLine in detectedLines)
        {
            var text = NormalizeChineseNumericDashes(detectedLine.Text.Trim());
            if (text.Length == 0)
            {
                continue;
            }

            if (previous is not null && HasLogicalBoundary(previous, detectedLine))
            {
                lines.Add(string.Empty);
            }

            var bounds = GetBounds(detectedLine);
            var index = lines.Count;
            lines.Add(text);
            var top = (int)Math.Floor(bounds.Top);
            var bottom = (int)Math.Ceiling(bounds.Bottom);
            physical.Add(new OcrPhysicalLine(
                index,
                CreateBandId(prepared.ContentFingerprint, top, bottom),
                top,
                bottom));
            previous = detectedLine;
        }

        return new LogicalProjection(lines, physical);
    }

    private static string CreateDetectedBandId(
        IReadOnlyList<OcrLine> detectedLines,
        int start,
        int count,
        ulong fingerprint)
    {
        var bounds = GetBounds(detectedLines[start]);
        for (var offset = 1; offset < count; offset++)
        {
            bounds = Union(bounds, GetBounds(detectedLines[start + offset]));
        }

        return CreateBandId(
            fingerprint,
            (int)Math.Floor(bounds.Top),
            (int)Math.Ceiling(bounds.Bottom));
    }

    private static string CreateBandId(ulong fingerprint, int top, int bottom) =>
        $"winzh:{fingerprint:x16}:{top}:{bottom}";

    private static bool HasLogicalBoundary(OcrLine previous, OcrLine current)
    {
        var previousBounds = GetBounds(previous);
        var currentBounds = GetBounds(current);
        if (currentBounds.Top < previousBounds.Top)
        {
            return true;
        }

        var verticalGap = currentBounds.Top - previousBounds.Bottom;
        var referenceHeight = Math.Max(previousBounds.Height, currentBounds.Height);
        return verticalGap > Math.Max(12.0, referenceHeight * 0.9);
    }

    private static Rect GetBounds(OcrLine line)
    {
        var left = line.Words.Min(word => word.BoundingRect.Left);
        var top = line.Words.Min(word => word.BoundingRect.Top);
        var right = line.Words.Max(word => word.BoundingRect.Right);
        var bottom = line.Words.Max(word => word.BoundingRect.Bottom);
        return new Rect(left, top, right - left, bottom - top);
    }

    private static Rect Union(Rect left, Rect right)
    {
        var minimumX = Math.Min(left.Left, right.Left);
        var minimumY = Math.Min(left.Top, right.Top);
        var maximumX = Math.Max(left.Right, right.Right);
        var maximumY = Math.Max(left.Bottom, right.Bottom);
        return new Rect(minimumX, minimumY, maximumX - minimumX, maximumY - minimumY);
    }

    private static ulong CombineFingerprint(PreparedFrame frame, int sourceWidth, int sourceHeight)
    {
        var fingerprint = frame.ContentFingerprint;
        fingerprint ^= (ulong)frame.Width;
        fingerprint *= 1099511628211UL;
        fingerprint ^= (ulong)frame.Height;
        fingerprint *= 1099511628211UL;
        fingerprint ^= (ulong)sourceWidth;
        fingerprint *= 1099511628211UL;
        fingerprint ^= (ulong)sourceHeight;
        fingerprint *= 1099511628211UL;
        return fingerprint;
    }

    private readonly record struct RefinementCandidate(
        int StartLine,
        int LineCount,
        Rect Bounds,
        int EditDistance);

    private sealed record LogicalProjection(
        IReadOnlyList<string> Lines,
        IReadOnlyList<OcrPhysicalLine> PhysicalLines);

    private sealed record BatchRefinementJob(
        FullLineAffixMatcher Target,
        RefinementCandidate Candidate);

    private sealed record BatchRefinementBandJob(
        RefinementCandidate Candidate,
        IReadOnlyList<FullLineAffixMatcher> Targets);

    private sealed record BatchRefinementResult(
        IReadOnlyList<BatchVerifiedTarget> Verified,
        IReadOnlyList<FullLineAffixMatcher> Unresolved);

    private sealed record BatchVerifiedTarget(
        FullLineAffixMatcher Target,
        string ObservedText);

    private sealed record BatchCache(
        ulong Fingerprint,
        string TargetKey,
        OcrRecognitionResult Result);
}
