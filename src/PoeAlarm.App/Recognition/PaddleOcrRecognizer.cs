using System.Diagnostics;
using System.Text;
using PoeAlarm.App.Capture;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Recognition;

/// <summary>
/// Low-latency Traditional Chinese recognizer backed by the packaged PP-OCRv5 ONNX model.
/// It keeps exact full-affix matching authoritative and exposes narrowly target-conditioned
/// CTC evidence only as a separate assisted result.
/// </summary>
public sealed class PaddleOcrRecognizer : IOcrRecognizer, IFrameFingerprintProvider
{
    private const int MaximumCachedBands = 256;
    private const int MaximumParallelBands = 2;
    private static readonly PoeTextPreprocessorSettings TraditionalChinesePreprocessing = new(
        MinimumBlue: 100,
        MinimumBlueDominance: 14,
        MaximumWarmChannelDifference: 72,
        IntensityMode: BlueMaskIntensityMode.Dominance);

    private readonly PoeTextPreprocessor _preprocessor;
    private readonly PaddleCtcSession _session;
    private readonly SemaphoreSlim _recognitionGate = new(1, 1);
    private readonly Dictionary<BandCacheKey, BandRecognition> _bandRecognitionCache = [];
    private readonly Queue<BandCacheKey> _bandCacheInsertionOrder = [];
    private IReadOnlyList<string> _lastLines = [];
    private LogicalAffixMatch? _lastAssistedMatch;
    private ulong _lastFingerprint;
    private string _lastTargetKey = string.Empty;
    private int _lastFrameWidth;
    private int _lastFrameHeight;
    private bool _hasCachedRecognition;
    private ulong _progressFingerprint;
    private string _progressTargetKey = string.Empty;
    private int _progressFrameWidth;
    private int _progressFrameHeight;
    private bool _hasProgressFrame;
    private BatchProgress? _batchProgress;
    private BatchCache? _batchCache;
    private long _batchPrimaryInferenceCount;
    private long _batchRecoveryInferenceCount;
    private CapturedFrame? _prefetchedSourceFrame;
    private IReadOnlyList<PreparedFrame>? _prefetchedBands;
    private bool _disposed;

    public StructuredRuleOcrSupport StructuredRuleSupport =>
        StructuredRuleOcrSupport.StrictBatch;

    internal long BatchPrimaryInferenceCount => _batchPrimaryInferenceCount;

    internal long BatchRecoveryInferenceCount => _batchRecoveryInferenceCount;

    public PaddleOcrRecognizer()
        : this(CreatePackagedSession(), new PoeTextPreprocessor(TraditionalChinesePreprocessing))
    {
    }

    public PaddleOcrRecognizer(
        string modelPath,
        string dictionaryPath,
        int threads = 2,
        PoeTextPreprocessor? preprocessor = null)
        : this(
            new PaddleCtcSession(
                modelPath,
                dictionaryPath,
                threads,
                TextPolarity.BrightOnDark,
                inkMargin: 2),
            preprocessor ?? new PoeTextPreprocessor(TraditionalChinesePreprocessing))
    {
    }

    private PaddleOcrRecognizer(PaddleCtcSession session, PoeTextPreprocessor preprocessor)
    {
        _session = session;
        _preprocessor = preprocessor;
    }

    public ulong ComputeFrameFingerprint(CapturedFrame frame)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        ArgumentNullException.ThrowIfNull(frame);
        var bands = _preprocessor.PrepareBlueAffixBands(frame, scale: 1);
        _prefetchedSourceFrame = frame;
        _prefetchedBands = bands;

        var fingerprint = CombineFingerprints(bands);
        fingerprint ^= (ulong)frame.Width;
        fingerprint *= 1099511628211UL;
        fingerprint ^= (ulong)frame.Height;
        fingerprint *= 1099511628211UL;
        return fingerprint;
    }

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default) =>
        RecognizeInternalAsync(frame, target: null, cancellationToken);

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(target);
        return RecognizeInternalAsync(frame, target, cancellationToken);
    }

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        IReadOnlyList<FullLineAffixMatcher> targets,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(frame);
        var targetSet = Poe2EnglishRecognizer.NormalizeTargetSet(targets);
        return RecognizeBatchInternalAsync(frame, targetSet, cancellationToken);
    }

    private async Task<OcrRecognitionResult> RecognizeBatchInternalAsync(
        CapturedFrame frame,
        IReadOnlyList<FullLineAffixMatcher> targets,
        CancellationToken cancellationToken)
    {
        await _recognitionGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            return await Task.Run(
                    () => RecognizeBatchCore(frame, targets, cancellationToken),
                    cancellationToken)
                .ConfigureAwait(false);
        }
        finally
        {
            _recognitionGate.Release();
        }
    }

    private OcrRecognitionResult RecognizeBatchCore(
        CapturedFrame frame,
        IReadOnlyList<FullLineAffixMatcher> targets,
        CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        if (targets.Count == 0)
        {
            return RecognizeCore(frame, target: null, cancellationToken);
        }

        var stopwatch = Stopwatch.StartNew();
        IReadOnlyList<PreparedFrame> preparedBands;
        if (ReferenceEquals(frame, _prefetchedSourceFrame) && _prefetchedBands is not null)
        {
            preparedBands = _prefetchedBands;
            _prefetchedSourceFrame = null;
            _prefetchedBands = null;
        }
        else
        {
            preparedBands = _preprocessor.PrepareBlueAffixBands(frame, scale: 1);
        }

        var preprocessingElapsed = stopwatch.Elapsed;
        var fingerprint = CombineFingerprints(preparedBands);
        var targetKey = Poe2EnglishRecognizer.CreateTargetSetKey(targets);
        if (_batchCache is { } cached &&
            cached.Fingerprint == fingerprint &&
            cached.Width == frame.Width &&
            cached.Height == frame.Height &&
            string.Equals(cached.TargetKey, targetKey, StringComparison.Ordinal))
        {
            return cached.Result with
            {
                PreprocessingElapsed = preprocessingElapsed,
                RecognitionElapsed = TimeSpan.Zero,
                WasCached = true,
                RequiresRescan = false,
            };
        }

        var sameProgress = _batchProgress is { } prior &&
                           prior.Fingerprint == fingerprint &&
                           prior.Width == frame.Width &&
                           prior.Height == frame.Height &&
                           string.Equals(prior.TargetKey, targetKey, StringComparison.Ordinal);
        if (!sameProgress)
        {
            _batchProgress = new BatchProgress(
                fingerprint,
                frame.Width,
                frame.Height,
                targetKey,
                new BatchBandRecognition?[preparedBands.Count],
                nextBandIndex: 0,
                recoveryQueue: new Queue<BatchRecoveryWork>());
        }

        var progress = _batchProgress!;
        stopwatch.Restart();
        var workUnits = 0;
        var authoritativeHit = false;

        // One scan advances at most two physical rows. Target support for all rules is derived
        // from each row's shared CTC logits; target count never multiplies model inference.
        while (progress.NextBandIndex < preparedBands.Count && workUnits < MaximumParallelBands)
        {
            var index = progress.NextBandIndex++;
            var prepared = preparedBands[index];
            var batch = _session.RecognizeBatch(prepared, targets);
            _batchPrimaryInferenceCount++;
            progress.Bands[index] = new BatchBandRecognition(batch);
            authoritativeHit |= targets.Any(target =>
                target.IsMatch(batch.Recognition.Text) ||
                batch.TargetSupports.TryGetValue(target.Template.Text, out var support) &&
                support.StronglySupported);
            if (!prepared.IsFallback)
            {
                var recoveryTarget = SelectRecoveryTarget(batch, targets);
                if (recoveryTarget is not null)
                {
                    progress.RecoveryQueue.Enqueue(new BatchRecoveryWork(index, recoveryTarget));
                }
            }

            workUnits++;
            if (authoritativeHit)
            {
                // A lexical hit may still fail a numeric constraint in the caller, but all
                // visible evidence needed to evaluate that row is complete. Do not spend this
                // latency-sensitive scan on unrelated remaining bands; the next scan resumes if
                // the complete rule is not yet satisfied.
                break;
            }
        }

        // Only after every primary row is available, spend at most one additional inference per
        // later scan on the best localized recovery. Never append it to a scan that already ran
        // primary work, keeping the hard per-scan ceiling at two model inferences.
        if (workUnits == 0 && progress.NextBandIndex >= preparedBands.Count &&
            progress.RecoveryQueue.TryDequeue(out var recovery))
        {
            var current = progress.Bands[recovery.BandIndex]!;
            var segmented = _session.Recognize(
                preparedBands[recovery.BandIndex],
                recovery.Target.SourceTemplate,
                maximumSegmentWidthOverride: 400);
            _batchRecoveryInferenceCount++;
            if (recovery.Target.IsMatch(segmented.Text))
            {
                current.SegmentedStrict.Add(new BatchAssistedText(
                    segmented.Text,
                    recovery.Target.Template.Text));
            }
            workUnits++;
        }

        var lines = BuildBatchLogicalLines(
            preparedBands,
            progress.Bands,
            frame.Width,
            out var assisted,
            out var physicalLines,
            out var candidateEvidence);
        var requiresRescan = progress.NextBandIndex < preparedBands.Count ||
                             progress.RecoveryQueue.Count > 0;
        // Near-target evidence is meaningful only after every bounded verifier has completed.
        // Emitting it during a progressive pass could make Guarded pause on a row that the next
        // slice would have strictly recovered.
        if (requiresRescan)
        {
            candidateEvidence = [];
        }
        var result = new OcrRecognitionResult(
            lines,
            preprocessingElapsed,
            stopwatch.Elapsed,
            WasCached: sameProgress && workUnits == 0,
            TargetAssistedMatch: null,
            RequiresRescan: requiresRescan,
            AssistedObservations: assisted,
            PhysicalLines: physicalLines,
            CandidateEvidence: candidateEvidence);
        if (!requiresRescan)
        {
            _batchProgress = null;
            _batchCache = new BatchCache(
                fingerprint,
                frame.Width,
                frame.Height,
                targetKey,
                result);
        }

        return result;
    }

    private async Task<OcrRecognitionResult> RecognizeInternalAsync(
        CapturedFrame frame,
        FullLineAffixMatcher? target,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(frame);
        await _recognitionGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            // The first live scan is initiated by a WPF button handler. Run CPU inference on a
            // worker so the monitoring HUD can paint immediately instead of waiting on ONNX.
            return await Task.Run(
                    () => RecognizeCore(frame, target, cancellationToken),
                    cancellationToken)
                .ConfigureAwait(false);
        }
        finally
        {
            _recognitionGate.Release();
        }
    }

    private OcrRecognitionResult RecognizeCore(
        CapturedFrame frame,
        FullLineAffixMatcher? target,
        CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        var stopwatch = Stopwatch.StartNew();
        IReadOnlyList<PreparedFrame> preparedBands;
        if (ReferenceEquals(frame, _prefetchedSourceFrame) && _prefetchedBands is not null)
        {
            // AffixMonitor asks for a semantic fingerprint immediately before OCR. Reuse that
            // exact preprocessing result instead of walking and masking the whole ROI twice.
            preparedBands = _prefetchedBands;
            _prefetchedSourceFrame = null;
            _prefetchedBands = null;
        }
        else
        {
            preparedBands = _preprocessor.PrepareBlueAffixBands(frame, scale: 1);
        }
        var preprocessingElapsed = stopwatch.Elapsed;
        var fingerprint = CombineFingerprints(preparedBands);
        var targetKey = target?.Template.Text ?? string.Empty;

        if (_hasCachedRecognition &&
            fingerprint == _lastFingerprint &&
            frame.Width == _lastFrameWidth &&
            frame.Height == _lastFrameHeight &&
            string.Equals(targetKey, _lastTargetKey, StringComparison.Ordinal))
        {
            return new OcrRecognitionResult(
                _lastLines,
                preprocessingElapsed,
                TimeSpan.Zero,
                WasCached: true,
                _lastAssistedMatch);
        }

        stopwatch.Restart();
        var bandRecognitions = new BandRecognition?[preparedBands.Count];
        var bandCacheKeys = new BandCacheKey[preparedBands.Count];
        var uncachedBands = new List<UncachedBand>();
        for (var index = 0; index < preparedBands.Count; index++)
        {
            var prepared = preparedBands[index];
            var cacheKey = new BandCacheKey(
                prepared.ContentFingerprint,
                prepared.Width,
                prepared.Height,
                targetKey);
            bandCacheKeys[index] = cacheKey;
            if (_bandRecognitionCache.TryGetValue(cacheKey, out var cachedRecognition))
            {
                bandRecognitions[index] = cachedRecognition;
            }
            else
            {
                uncachedBands.Add(new UncachedBand(
                    index,
                    cacheKey,
                    prepared,
                    target is null ? 0 : TargetWidthDistance(prepared, target, frame.Width)));
            }
        }

        if (target is not null)
        {
            uncachedBands.Sort((left, right) =>
            {
                var priority = left.TargetWidthDistance.CompareTo(right.TargetWidthDistance);
                return priority != 0 ? priority : left.Index.CompareTo(right.Index);
            });

            if (TryFindAuthoritativeBandHit(bandRecognitions, target, out var cachedMatchText))
            {
                _hasProgressFrame = false;
                return CreateStrictHitResult(
                    cachedMatchText,
                    preprocessingElapsed,
                    stopwatch.Elapsed,
                    wasCached: true);
            }

            var cachedPartialLines = BuildLogicalLines(
                preparedBands,
                bandRecognitions,
                frame.Width,
                cancellationToken,
                out _);
            if (target.TryFindMatch(cachedPartialLines, out _))
            {
                _hasProgressFrame = false;
                return new OcrRecognitionResult(
                    cachedPartialLines,
                    preprocessingElapsed,
                    stopwatch.Elapsed,
                    WasCached: true);
            }
        }

        var everyBandWasCached = uncachedBands.Count == 0;
        if (target is null)
        {
            // Target-free diagnostics retain exhaustive behavior because there is no compiled
            // affix to prioritize and their callers do not participate in progressive rescans.
            for (var batchStart = 0; batchStart < uncachedBands.Count;
                 batchStart += MaximumParallelBands)
            {
                var batch = uncachedBands
                    .Skip(batchStart)
                    .Take(MaximumParallelBands)
                    .ToArray();
                RecognizePrimaryBatch(batch, bandRecognitions, target, cancellationToken);
            }
        }
        else
        {
            var sameProgressFrame = IsSameProgressFrame(
                fingerprint,
                targetKey,
                frame.Width,
                frame.Height);
            var strongRecoveryIndex = sameProgressFrame
                ? FindPendingRecoveryIndex(
                    preparedBands,
                    bandRecognitions,
                    target,
                    frame.Width,
                    requireStrongCtc: true)
                : null;

            if (strongRecoveryIndex is int strongIndex)
            {
                CompleteOneRecoveryUnit(
                    strongIndex,
                    preparedBands,
                    bandRecognitions,
                    bandCacheKeys,
                    target,
                    cancellationToken);
                everyBandWasCached = false;
            }
            else if (uncachedBands.Count > 0)
            {
                // One primary pair is the complete work unit for a newly observed frame.  In
                // particular, do not immediately append every possible segmentation pass.
                RecognizePrimaryBatch(
                    uncachedBands.Take(MaximumParallelBands).ToArray(),
                    bandRecognitions,
                    target,
                    cancellationToken);
            }
            else if (FindPendingRecoveryIndex(
                         preparedBands,
                         bandRecognitions,
                         target,
                         frame.Width,
                         requireStrongCtc: false) is int recoveryIndex)
            {
                CompleteOneRecoveryUnit(
                    recoveryIndex,
                    preparedBands,
                    bandRecognitions,
                    bandCacheKeys,
                    target,
                    cancellationToken);
                everyBandWasCached = false;
            }

            if (TryFindAuthoritativeBandHit(bandRecognitions, target, out var matchText))
            {
                _hasProgressFrame = false;
                return CreateStrictHitResult(
                    matchText,
                    preprocessingElapsed,
                    stopwatch.Elapsed,
                    wasCached: everyBandWasCached);
            }
        }

        var lines = BuildLogicalLines(
            preparedBands,
            bandRecognitions,
            frame.Width,
            cancellationToken,
            out var assistedCandidates);
        var requiresRescan = target is not null && bandRecognitions.Any(item =>
            item is null || !item.Recovery.IsComplete);
        var recognitionElapsed = stopwatch.Elapsed;

        if (target is not null && target.TryFindMatch(lines, out _))
        {
            _hasProgressFrame = false;
            return new OcrRecognitionResult(
                lines,
                preprocessingElapsed,
                recognitionElapsed,
                WasCached: everyBandWasCached);
        }

        if (requiresRescan)
        {
            RememberProgressFrame(fingerprint, targetKey, frame.Width, frame.Height);
            // Unrecognized physical slots become explicit logical boundaries in `lines`, so the
            // monitor may safely detect a completed adjacent multi-line target without ever
            // joining text across work that has not run yet.
            return new OcrRecognitionResult(
                lines,
                preprocessingElapsed,
                recognitionElapsed,
                WasCached: everyBandWasCached,
                TargetAssistedMatch: null,
                RequiresRescan: true);
        }

        LogicalAffixMatch? assistedMatch = null;
        if (target is not null && assistedCandidates.Count == 1)
        {
            var candidate = assistedCandidates[0];
            assistedMatch = new LogicalAffixMatch(
                candidate.LineIndex,
                PhysicalLineCount: 1,
                candidate.Text,
                target.Template.Text);
        }

        _lastLines = lines;
        _lastAssistedMatch = assistedMatch;
        _lastFingerprint = fingerprint;
        _lastTargetKey = targetKey;
        _lastFrameWidth = frame.Width;
        _lastFrameHeight = frame.Height;
        _hasCachedRecognition = true;
        _hasProgressFrame = false;

        return new OcrRecognitionResult(
            _lastLines,
            preprocessingElapsed,
            recognitionElapsed,
            WasCached: everyBandWasCached,
            assistedMatch);
    }

    private static BandRecognition ToBandRecognition(
        PaddleTargetRecognition recognition,
        FullLineAffixMatcher? target) =>
        new(target is null
            ? PaddleTargetRecoveryWork.Completed(recognition)
            : PaddleTargetRecovery.CreateSegmentationWork(recognition, target));

    private void RecognizePrimaryBatch(
        IReadOnlyList<UncachedBand> batch,
        BandRecognition?[] bandRecognitions,
        FullLineAffixMatcher? target,
        CancellationToken cancellationToken)
    {
        if (batch.Count == 0)
        {
            return;
        }

        var preliminary = new PaddleTargetRecognition?[batch.Count];
        Parallel.For(
            0,
            batch.Count,
            new ParallelOptions
            {
                CancellationToken = cancellationToken,
                MaxDegreeOfParallelism = MaximumParallelBands,
            },
            offset =>
            {
                preliminary[offset] = PaddleTargetRecovery.RecognizePrimary(
                    _session,
                    batch[offset].Frame,
                    target);
            });

        // Parallel inference writes only to the local result array. Commit the cache as one
        // serialized step so cancellation or a worker failure cannot expose a half-populated
        // batch to another recognition call.
        cancellationToken.ThrowIfCancellationRequested();
        for (var offset = 0; offset < batch.Count; offset++)
        {
            var item = batch[offset];
            var recognized = preliminary[offset]
                ?? throw new InvalidOperationException("Parallel primary OCR did not return a row result.");
            var bandRecognition = ToBandRecognition(recognized, target);
            bandRecognitions[item.Index] = bandRecognition;
            CacheBandRecognition(item.CacheKey, bandRecognition);
        }
    }

    private void CompleteOneRecoveryUnit(
        int bandIndex,
        IReadOnlyList<PreparedFrame> preparedBands,
        BandRecognition?[] bandRecognitions,
        IReadOnlyList<BandCacheKey> cacheKeys,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        var current = bandRecognitions[bandIndex]
            ?? throw new InvalidOperationException("Cannot recover an OCR row before primary recognition.");
        var advanced = PaddleTargetRecovery.CompleteNextSegmentation(
            _session,
            preparedBands[bandIndex],
            target,
            current.Recovery);
        cancellationToken.ThrowIfCancellationRequested();
        var updated = new BandRecognition(advanced);
        bandRecognitions[bandIndex] = updated;
        CacheBandRecognition(cacheKeys[bandIndex], updated);
    }

    private static int? FindPendingRecoveryIndex(
        IReadOnlyList<PreparedFrame> preparedBands,
        IReadOnlyList<BandRecognition?> bandRecognitions,
        FullLineAffixMatcher target,
        int roiWidth,
        bool requireStrongCtc)
    {
        return Enumerable.Range(0, bandRecognitions.Count)
            .Where(index => bandRecognitions[index] is { } recognition &&
                            !recognition.Recovery.IsComplete &&
                            (!requireStrongCtc || recognition.AssistanceKind ==
                                PaddleTargetAssistanceKind.CtcTargetSupport))
            .OrderBy(index => TargetWidthDistance(preparedBands[index], target, roiWidth))
            .ThenBy(index => index)
            .Select(index => (int?)index)
            .FirstOrDefault();
    }

    private static bool TryFindAuthoritativeBandHit(
        IReadOnlyList<BandRecognition?> recognitions,
        FullLineAffixMatcher target,
        out string matchText)
    {
        foreach (var recognition in recognitions)
        {
            if (recognition is not null &&
                TryGetAuthoritativeMatchText(recognition, target, out matchText))
            {
                return true;
            }
        }

        matchText = string.Empty;
        return false;
    }

    private static IReadOnlyList<string> BuildLogicalLines(
        IReadOnlyList<PreparedFrame> preparedBands,
        IReadOnlyList<BandRecognition?> bandRecognitions,
        int roiWidth,
        CancellationToken cancellationToken,
        out List<AssistedCandidate> assistedCandidates)
    {
        var lines = new List<string>(preparedBands.Count);
        assistedCandidates = [];
        int? previousBottom = null;
        var previousHeight = 0;
        var previousWidth = 0;

        for (var bandIndex = 0; bandIndex < preparedBands.Count; bandIndex++)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var prepared = preparedBands[bandIndex];
            var recognized = bandRecognitions[bandIndex];

            // An unprocessed (or unreadable) physical slot is a hard boundary.  This is the
            // essential invariant that permits partial multi-line matching without ever joining
            // two recognized rows across text whose OCR has not completed.
            if (recognized is null || string.IsNullOrWhiteSpace(recognized.PrimaryText))
            {
                AddLogicalBoundary(lines);
                previousBottom = null;
                previousHeight = 0;
                previousWidth = 0;
                continue;
            }

            if (prepared.IsFallback)
            {
                AddLogicalBoundary(lines);
                lines.Add(recognized.PrimaryText);
                AddLogicalBoundary(lines);
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

            var lineIndex = lines.Count;
            lines.Add(recognized.PrimaryText);
            if (recognized.AssistanceKind == PaddleTargetAssistanceKind.CtcTargetSupport &&
                !string.IsNullOrWhiteSpace(recognized.AssistedText))
            {
                assistedCandidates.Add(new AssistedCandidate(lineIndex, recognized.AssistedText));
            }

            previousBottom = prepared.SourceBottom;
            previousHeight = currentHeight;
            previousWidth = prepared.Width;
        }

        return lines;
    }

    private static FullLineAffixMatcher? SelectRecoveryTarget(
        CtcBatchRecognition batch,
        IReadOnlyList<FullLineAffixMatcher> targets)
    {
        if (targets.Any(target => target.IsMatch(batch.Recognition.Text)))
        {
            return null;
        }

        // At most one target owns recovery work for a physical band. Strong CTC evidence wins;
        // ties use the stable normalized target order. This prevents one noisy row from
        // generating N segmentation inferences or N independently accepted alternatives.
        return targets.FirstOrDefault(target =>
            batch.TargetSupports.TryGetValue(target.Template.Text, out var support) &&
            support.StronglySupported);
    }

    private static IReadOnlyList<string> BuildBatchLogicalLines(
        IReadOnlyList<PreparedFrame> preparedBands,
        IReadOnlyList<BatchBandRecognition?> recognitions,
        int roiWidth,
        out IReadOnlyList<OcrAssistedObservation> assisted,
        out IReadOnlyList<OcrPhysicalLine> physicalLines,
        out IReadOnlyList<OcrCandidateEvidence> candidateEvidence)
    {
        var lines = new List<string>(preparedBands.Count * 2);
        var assistedItems = new List<OcrAssistedObservation>();
        var candidateItems = new List<OcrCandidateEvidence>();
        var physicalItems = new List<OcrPhysicalLine>();
        int? previousBottom = null;
        var previousHeight = 0;
        var previousWidth = 0;

        for (var bandIndex = 0; bandIndex < preparedBands.Count; bandIndex++)
        {
            var prepared = preparedBands[bandIndex];
            var recognized = recognitions[bandIndex];
            if (recognized is null || string.IsNullOrWhiteSpace(recognized.Batch.Recognition.Text))
            {
                AddLogicalBoundary(lines);
                previousBottom = null;
                previousHeight = 0;
                previousWidth = 0;
                continue;
            }

            if (prepared.IsFallback)
            {
                AddLogicalBoundary(lines);
            }
            else if (previousBottom is not null)
            {
                var currentHeight = prepared.SourceBottom - prepared.SourceTop + 1;
                var gap = prepared.SourceTop - previousBottom.Value - 1;
                var closeEnough = gap <= Math.Max(12, Math.Max(previousHeight, currentHeight));
                var previousFilled = previousWidth >= roiWidth * 0.70;
                if (!closeEnough || !previousFilled)
                {
                    AddLogicalBoundary(lines);
                }
            }

            var lineIndex = lines.Count;
            var text = recognized.Batch.Recognition.Text;
            lines.Add(text);
            var bandId = CreateBandId(prepared);
            physicalItems.Add(new OcrPhysicalLine(
                lineIndex,
                bandId,
                prepared.SourceTop,
                prepared.SourceBottom));

            foreach (var target in recognized.Batch.TargetSupports
                         .Where(static pair => pair.Value.StronglySupported))
            {
                assistedItems.Add(new OcrAssistedObservation(
                    bandId,
                    text,
                    target.Key,
                    prepared.SourceTop,
                    prepared.SourceBottom));
            }


            foreach (var target in recognized.Batch.TargetSupports
                         .Where(static pair => pair.Value.ShapeCompatible &&
                                               !pair.Value.StronglySupported))
            {
                candidateItems.Add(new OcrCandidateEvidence(
                    bandId,
                    text,
                    target.Key,
                    ClassifyBatchCandidate(text, target.Key),
                    prepared.SourceTop,
                    prepared.SourceBottom));
            }

            foreach (var segmented in recognized.SegmentedStrict)
            {
                assistedItems.Add(new OcrAssistedObservation(
                    bandId,
                    segmented.Text,
                    segmented.CanonicalTarget,
                    prepared.SourceTop,
                    prepared.SourceBottom));
            }

            if (prepared.IsFallback)
            {
                AddLogicalBoundary(lines);
                previousBottom = null;
                previousHeight = 0;
                previousWidth = 0;
            }
            else
            {
                previousBottom = prepared.SourceBottom;
                previousHeight = prepared.SourceBottom - prepared.SourceTop + 1;
                previousWidth = prepared.Width;
            }
        }

        assisted = assistedItems
            .GroupBy(item => $"{item.PhysicalBandId}\n{item.CanonicalTarget}", StringComparer.Ordinal)
            .Select(static group => group.First())
            .ToArray();
        physicalLines = physicalItems;
        candidateEvidence = candidateItems;
        return lines;
    }

    private static OcrCandidateEvidenceKind ClassifyBatchCandidate(
        string observed,
        string canonicalTarget)
    {
        var actualNumeric = AffixCanonicalizer.Normalize(observed).Tokens.Count(static token =>
            token.Kind != AffixTokenKind.Word);
        var expectedNumeric = AffixCanonicalizer.Normalize(canonicalTarget).Tokens.Count(static token =>
            token.Kind != AffixTokenKind.Word);
        if (actualNumeric < expectedNumeric)
        {
            return OcrCandidateEvidenceKind.MissingNumericValue;
        }

        return actualNumeric > expectedNumeric
            ? OcrCandidateEvidenceKind.ConflictingNumericValue
            : OcrCandidateEvidenceKind.LocalizedLexicalCandidate;
    }

    private static string CreateBandId(PreparedFrame band) =>
        $"{band.ContentFingerprint:x16}:{band.SourceTop}:{band.SourceBottom}:{band.SourceLeft}:{band.SourceRight}";

    private bool IsSameProgressFrame(
        ulong fingerprint,
        string targetKey,
        int frameWidth,
        int frameHeight) =>
        _hasProgressFrame &&
        fingerprint == _progressFingerprint &&
        frameWidth == _progressFrameWidth &&
        frameHeight == _progressFrameHeight &&
        string.Equals(targetKey, _progressTargetKey, StringComparison.Ordinal);

    private void RememberProgressFrame(
        ulong fingerprint,
        string targetKey,
        int frameWidth,
        int frameHeight)
    {
        _progressFingerprint = fingerprint;
        _progressTargetKey = targetKey;
        _progressFrameWidth = frameWidth;
        _progressFrameHeight = frameHeight;
        _hasProgressFrame = true;
    }

    private static bool TryGetAuthoritativeMatchText(
        BandRecognition recognition,
        FullLineAffixMatcher target,
        out string matchText)
    {
        if (target.IsMatch(recognition.PrimaryText))
        {
            matchText = recognition.PrimaryText;
            return true;
        }

        // Segmentation produces a second OCR transcript that itself passed the exact canonical
        // matcher. CTC substitution support is deliberately excluded here: that route remains
        // subject to the full-frame single-candidate guard before it can alert.
        if (recognition.AssistanceKind == PaddleTargetAssistanceKind.SegmentedStrict &&
            !string.IsNullOrWhiteSpace(recognition.AssistedText) &&
            target.IsMatch(recognition.AssistedText))
        {
            matchText = recognition.AssistedText;
            return true;
        }

        matchText = string.Empty;
        return false;
    }

    private static OcrRecognitionResult CreateStrictHitResult(
        string primaryText,
        TimeSpan preprocessingElapsed,
        TimeSpan recognitionElapsed,
        bool wasCached) =>
        new(
            [primaryText],
            preprocessingElapsed,
            recognitionElapsed,
            wasCached,
            TargetAssistedMatch: null);

    private static double TargetWidthDistance(
        PreparedFrame candidate,
        FullLineAffixMatcher target,
        int roiWidth)
    {
        var inkHeight = Math.Max(
            1,
            candidate.LogicalContentBottom - candidate.LogicalContentTop + 1);
        var expectedWidth = Math.Min(
            roiWidth,
            20 + (EstimateTemplateWidthUnits(target.SourceTemplate) * inkHeight * 1.05));
        return Math.Abs(DominantInkWidth(candidate, inkHeight) - expectedWidth);
    }

    private static double EstimateTemplateWidthUnits(string template)
    {
        var units = 0d;
        foreach (var rune in template.Normalize(NormalizationForm.FormKC).EnumerateRunes())
        {
            if (Rune.IsWhiteSpace(rune))
            {
                continue;
            }

            units += rune.Value switch
            {
                '#' => 1.5,
                '%' => 0.72,
                '+' or '-' => 0.62,
                _ when IsHan(rune) => 1.0,
                _ when Rune.IsLetterOrDigit(rune) => 0.58,
                _ => 0.55,
            };
        }

        return units;
    }

    private static int DominantInkWidth(PreparedFrame frame, int inkHeight)
    {
        var firstRow = Math.Clamp(frame.LogicalContentTop, 0, frame.Height - 1);
        var lastRow = Math.Clamp(frame.LogicalContentBottom, firstRow, frame.Height - 1);
        var maximumGap = Math.Max(8, inkHeight);
        var groupStart = -1;
        var groupLastInk = -1;
        var groupInk = 0;
        var bestWidth = 0;
        var bestInk = 0;

        for (var x = 0; x < frame.Width; x++)
        {
            var columnInk = 0;
            for (var y = firstRow; y <= lastRow; y++)
            {
                if (frame.BgraPixels[(y * frame.Stride) + (x * 4)] != 0)
                {
                    columnInk++;
                }
            }

            if (columnInk == 0)
            {
                continue;
            }

            if (groupStart >= 0 && x - groupLastInk - 1 > maximumGap)
            {
                CommitGroup();
                groupStart = -1;
                groupInk = 0;
            }

            groupStart = groupStart < 0 ? x : groupStart;
            groupLastInk = x;
            groupInk += columnInk;
        }

        CommitGroup();
        return bestWidth > 0 ? Math.Min(frame.Width, bestWidth + 20) : frame.Width;

        void CommitGroup()
        {
            if (groupStart < 0 || groupInk <= bestInk)
            {
                return;
            }

            bestInk = groupInk;
            bestWidth = groupLastInk - groupStart + 1;
        }
    }

    private static bool IsHan(Rune rune) =>
        rune.Value is >= 0x3400 and <= 0x4DBF or
            >= 0x4E00 and <= 0x9FFF or
            >= 0xF900 and <= 0xFAFF or
            >= 0x20000 and <= 0x323AF;

    public void Dispose()
    {
        _recognitionGate.Wait();
        try
        {
            if (_disposed)
            {
                return;
            }

            _disposed = true;
            _batchProgress = null;
            _batchCache = null;
            _session.Dispose();
        }
        finally
        {
            // Do not dispose the semaphore: an already queued caller must be able to wake and
            // observe the recognizer's disposed state without racing ObjectDisposedException.
            _recognitionGate.Release();
        }
    }

    private static PaddleCtcSession CreatePackagedSession()
    {
        var assets = PackagedPaddleOcrAssets.Load();
        return new PaddleCtcSession(
            assets.ModelBytes,
            assets.Dictionary,
            threads: 2,
            TextPolarity.BrightOnDark,
            inkMargin: 2);
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

    private void CacheBandRecognition(BandCacheKey key, BandRecognition recognition)
    {
        if (_bandRecognitionCache.ContainsKey(key))
        {
            _bandRecognitionCache[key] = recognition;
            return;
        }

        _bandRecognitionCache.Add(key, recognition);
        _bandCacheInsertionOrder.Enqueue(key);
        while (_bandRecognitionCache.Count > MaximumCachedBands &&
               _bandCacheInsertionOrder.TryDequeue(out var oldest))
        {
            _bandRecognitionCache.Remove(oldest);
        }
    }

    private readonly record struct BandCacheKey(
        ulong Fingerprint,
        int Width,
        int Height,
        string TargetKey);

    private sealed record BandRecognition(PaddleTargetRecoveryWork Recovery)
    {
        public string PrimaryText => Recovery.Recognition.Primary.Text;

        public string? AssistedText => Recovery.Recognition.AssistedText;

        public PaddleTargetAssistanceKind AssistanceKind => Recovery.Recognition.AssistanceKind;
    }

    private readonly record struct UncachedBand(
        int Index,
        BandCacheKey CacheKey,
        PreparedFrame Frame,
        double TargetWidthDistance);

    private readonly record struct AssistedCandidate(int LineIndex, string Text);

    private sealed class BatchProgress
    {
        public BatchProgress(
            ulong fingerprint,
            int width,
            int height,
            string targetKey,
            BatchBandRecognition?[] bands,
            int nextBandIndex,
            Queue<BatchRecoveryWork> recoveryQueue)
        {
            Fingerprint = fingerprint;
            Width = width;
            Height = height;
            TargetKey = targetKey;
            Bands = bands;
            NextBandIndex = nextBandIndex;
            RecoveryQueue = recoveryQueue;
        }

        public ulong Fingerprint { get; }

        public int Width { get; }

        public int Height { get; }

        public string TargetKey { get; }

        public BatchBandRecognition?[] Bands { get; }

        public int NextBandIndex { get; set; }

        public Queue<BatchRecoveryWork> RecoveryQueue { get; }
    }

    private sealed record BatchCache(
        ulong Fingerprint,
        int Width,
        int Height,
        string TargetKey,
        OcrRecognitionResult Result);

    private sealed record BatchBandRecognition(CtcBatchRecognition Batch)
    {
        public List<BatchAssistedText> SegmentedStrict { get; } = [];
    }

    private sealed record BatchRecoveryWork(
        int BandIndex,
        FullLineAffixMatcher Target);

    private sealed record BatchAssistedText(string Text, string CanonicalTarget);
}
