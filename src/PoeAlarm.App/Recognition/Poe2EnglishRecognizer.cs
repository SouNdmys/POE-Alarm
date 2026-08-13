using System.Diagnostics;
using System.Text;
using PoeAlarm.App.Capture;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Recognition;

/// <summary>
/// POE2 English profile. The proven POE1 Windows recognizer remains untouched: this composition
/// adds changed-frame confirmation and a narrowly localized CTC verifier only for POE2.
/// </summary>
public sealed class Poe2EnglishRecognizer : IOcrRecognizer, IFrameFingerprintProvider
{
    private readonly PoeTextPreprocessor _preprocessor;
    private readonly WindowsOcrRecognizer _primary;
    private readonly WindowsOcrRecognizer _confirmation;
    private readonly SemaphoreSlim _recognitionGate = new(1, 1);
    private readonly Task<PaddleCtcSession> _paddleSessionTask;

    private CapturedFrame? _prefetchedSourceFrame;
    private ulong _prefetchedFingerprint;
    private PendingNoMatch? _pendingNoMatch;
    private ConfirmedNoMatch? _confirmedNoMatch;
    private PendingNoMatch? _pendingBatchNoMatch;
    private ConfirmedNoMatch? _confirmedBatchNoMatch;
    private BatchLocalizedRecovery? _pendingBatchRecovery;
    private long _batchPrimaryPassCount;
    private long _batchLocalizedWorkUnitCount;
    private bool _disposed;

    public Poe2EnglishRecognizer(PoeTextPreprocessor? preprocessor = null)
    {
        _preprocessor = preprocessor ?? new PoeTextPreprocessor();
        _primary = new WindowsOcrRecognizer(
            _preprocessor,
            scale: 1,
            language: OcrRecognitionLanguage.English,
            retainBandRecognitionDetails: true);
        _confirmation = new WindowsOcrRecognizer(
            _preprocessor,
            scale: 1,
            language: OcrRecognitionLanguage.English,
            retainBandRecognitionDetails: true);

        // Loading the embedded recognition-only model is the expensive cold step. Start it away
        // from the UI thread and keep one session resident; normal frames never run inference.
        _paddleSessionTask = Task.Run(CreatePackagedPaddleSession);
    }

    public string RecognizerLanguageTag => _primary.RecognizerLanguageTag;

    public StructuredRuleOcrSupport StructuredRuleSupport =>
        StructuredRuleOcrSupport.ConfirmedStrictBatch;

    internal long BatchPrimaryPassCount => _batchPrimaryPassCount;

    internal long BatchLocalizedWorkUnitCount => _batchLocalizedWorkUnitCount;

    public ulong ComputeFrameFingerprint(CapturedFrame frame)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        ArgumentNullException.ThrowIfNull(frame);

        var fingerprint = _preprocessor.ComputeBlueTextFingerprint(frame);
        _prefetchedSourceFrame = frame;
        _prefetchedFingerprint = fingerprint;
        return fingerprint;
    }

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default) =>
        _primary.RecognizeAsync(frame, cancellationToken);

    public async Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(frame);
        ArgumentNullException.ThrowIfNull(target);

        await _recognitionGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            var fingerprint = ReferenceEquals(frame, _prefetchedSourceFrame)
                ? _prefetchedFingerprint
                : _preprocessor.ComputeBlueTextFingerprint(frame);
            _prefetchedSourceFrame = null;
            var targetKey = CreateTargetKey(target);

            if (_confirmedNoMatch is { } confirmed &&
                confirmed.Fingerprint == fingerprint &&
                string.Equals(confirmed.TargetKey, targetKey, StringComparison.Ordinal))
            {
                return confirmed.Result with
                {
                    PreprocessingElapsed = TimeSpan.Zero,
                    RecognitionElapsed = TimeSpan.Zero,
                    WasCached = true,
                    RequiresRescan = false,
                };
            }

            var confirmingSameFrame = _pendingNoMatch is { } pending &&
                                      pending.Fingerprint == fingerprint &&
                                      string.Equals(pending.TargetKey, targetKey, StringComparison.Ordinal);
            if (!confirmingSameFrame)
            {
                _pendingNoMatch = null;
                _confirmedNoMatch = null;
            }

            var windows = confirmingSameFrame ? _confirmation : _primary;
            var result = await windows
                .RecognizeAsync(frame, cancellationToken)
                .ConfigureAwait(false);
            if (HasStrictTarget(result, target))
            {
                _pendingNoMatch = null;
                _confirmedNoMatch = null;
                return result with { RequiresRescan = false };
            }

            var recoveryAlreadyAttempted = confirmingSameFrame &&
                                           _pendingNoMatch!.RecoveryAttempted;
            var recovery = recoveryAlreadyAttempted
                ? null
                : await TryRecoverLocalizedBandAsync(
                        result.Lines,
                        windows.LastRecognizedBands,
                        target,
                        cancellationToken)
                    .ConfigureAwait(false);
            if (recovery is not null)
            {
                _pendingNoMatch = null;
                _confirmedNoMatch = null;
                return result with
                {
                    RecognitionElapsed = result.RecognitionElapsed + recovery.Elapsed,
                    WasCached = false,
                    TargetAssistedMatch = recovery.Match,
                    RequiresRescan = false,
                };
            }

            if (!confirmingSameFrame)
            {
                // A changed frame is never accepted from one strict miss. AffixMonitor keeps the
                // click guard held and captures it again for the independent Windows engine.
                _pendingNoMatch = new PendingNoMatch(
                    fingerprint,
                    targetKey,
                    RecoveryAttempted: HasOneLocalizedLexicalCandidate(result.Lines, target));
                return result with { RequiresRescan = true };
            }

            _pendingNoMatch = null;
            var final = result with { RequiresRescan = false };
            _confirmedNoMatch = new ConfirmedNoMatch(fingerprint, targetKey, final);
            return final;
        }
        finally
        {
            _recognitionGate.Release();
        }
    }

    /// <summary>
    /// Runs one shared Windows OCR pass and checks every deduplicated target strictly in memory.
    /// A changed frame not accepted by the caller's full rule evaluation is confirmed by the
    /// independent Windows engine on the next scan. Up to two localized bands are verified by
    /// shared Paddle logits, and every assisted alternative keeps its physical band identity.
    /// </summary>
    public async Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        IReadOnlyList<FullLineAffixMatcher> targets,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(frame);
        var targetSet = NormalizeTargetSet(targets);
        if (targetSet.Count == 0)
        {
            return await _primary.RecognizeAsync(frame, cancellationToken).ConfigureAwait(false);
        }

        await _recognitionGate.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            var fingerprint = ReferenceEquals(frame, _prefetchedSourceFrame)
                ? _prefetchedFingerprint
                : _preprocessor.ComputeBlueTextFingerprint(frame);
            _prefetchedSourceFrame = null;
            var targetKey = CreateTargetSetKey(targetSet);

            // Never let a pending state from the legacy single-target route influence a batch.
            _pendingNoMatch = null;
            _confirmedNoMatch = null;

            if (_confirmedBatchNoMatch is { } confirmed &&
                confirmed.Fingerprint == fingerprint &&
                string.Equals(confirmed.TargetKey, targetKey, StringComparison.Ordinal))
            {
                _pendingBatchRecovery = null;
                return confirmed.Result with
                {
                    PreprocessingElapsed = TimeSpan.Zero,
                    RecognitionElapsed = TimeSpan.Zero,
                    WasCached = true,
                    RequiresRescan = false,
                };
            }

            var confirmingSameFrame = _pendingBatchNoMatch is { } pending &&
                                      pending.Fingerprint == fingerprint &&
                                      string.Equals(pending.TargetKey, targetKey, StringComparison.Ordinal);
            if (!confirmingSameFrame)
            {
                _pendingBatchNoMatch = null;
                _confirmedBatchNoMatch = null;
                _pendingBatchRecovery = null;
            }

            var windows = confirmingSameFrame ? _confirmation : _primary;
            var result = await windows
                .RecognizeAsync(frame, targetSet, cancellationToken)
                .ConfigureAwait(false);
            _batchPrimaryPassCount++;
            var hasStrictTarget = HasAnyStrictTarget(result.Lines, targetSet);
            var batchRecovery = confirmingSameFrame && _pendingBatchRecovery is not null
                ? _pendingBatchRecovery
                : await TryRecoverLocalizedBandsAsync(
                        result.Lines,
                        windows.LastRecognizedBands,
                        targetSet,
                        cancellationToken)
                    .ConfigureAwait(false);
            result = result with
            {
                RecognitionElapsed = result.RecognitionElapsed + batchRecovery.Elapsed,
                AssistedObservations = batchRecovery.Assisted,
                CandidateEvidence = SelectBatchCandidateEvidence(
                    confirmingSameFrame,
                    batchRecovery.Candidates),
                PhysicalLines = CreatePhysicalLineMap(
                    result.Lines,
                    windows.LastRecognizedBands),
            };
            hasStrictTarget |= batchRecovery.Assisted.Count > 0;

            if (!confirmingSameFrame)
            {
                // A lexical target can still fail a numeric constraint or an AtLeast group in
                // the caller. Keep the changed-frame guard held unless the caller's full rule
                // evaluation accepts this first result; if it does not, the next scan uses the
                // independent confirmation engine before a non-match is released.
                _pendingBatchNoMatch = new PendingNoMatch(
                    fingerprint,
                    targetKey,
                    RecoveryAttempted: batchRecovery.Assisted.Count > 0 ||
                                       batchRecovery.Candidates.Count > 0);
                // Keep unresolved target-like evidence in the pending confirmation state. It is
                // hidden from the progressive first result below, then surfaced on the final
                // independent pass so Guarded can yellow-pause instead of accepting a false
                // NoMatch. Assisted transcripts remain tied to the same physical band ids.
                _pendingBatchRecovery = batchRecovery;
                return result with
                {
                    TargetAssistedMatch = null,
                    RequiresRescan = true,
                };
            }

            _pendingBatchNoMatch = null;
            _pendingBatchRecovery = null;
            var final = result with
            {
                TargetAssistedMatch = null,
                RequiresRescan = false,
            };
            // A strict target can still fail a numeric constraint or AtLeast group in the caller.
            // Cache only a lexical non-match; a candidate frame must remain re-evaluable.
            _confirmedBatchNoMatch = hasStrictTarget
                ? null
                : new ConfirmedNoMatch(fingerprint, targetKey, final);
            return final;
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
            if (_disposed)
            {
                return;
            }

            _disposed = true;
            _prefetchedSourceFrame = null;
            _pendingNoMatch = null;
            _confirmedNoMatch = null;
            _pendingBatchNoMatch = null;
            _confirmedBatchNoMatch = null;
            _pendingBatchRecovery = null;
            _confirmation.Dispose();
            _primary.Dispose();
            if (_paddleSessionTask.IsCompletedSuccessfully)
            {
                _paddleSessionTask.Result.Dispose();
            }
            else
            {
                // Do not block the WPF thread while the optional verifier is still warming up.
                // This observes load failures and releases a successful native session later.
                _ = _paddleSessionTask.ContinueWith(
                    static task =>
                    {
                        if (task.IsCompletedSuccessfully)
                        {
                            task.Result.Dispose();
                        }
                        else
                        {
                            _ = task.Exception;
                        }
                    },
                    CancellationToken.None,
                    TaskContinuationOptions.ExecuteSynchronously,
                    TaskScheduler.Default);
            }
        }
        finally
        {
            _recognitionGate.Release();
        }
    }

    private async Task<LocalizedRecovery?> TryRecoverLocalizedBandAsync(
        IReadOnlyList<string> logicalLines,
        IReadOnlyList<WindowsRecognizedBand> recognizedBands,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken)
    {
        if (!HasOneLocalizedLexicalCandidate(logicalLines, target))
        {
            return null;
        }

        var matchingBands = recognizedBands
            .Where(item => !item.Frame.IsFallback &&
                           ContainsLocalizedLexicalCandidate(item.Lines, target))
            .GroupBy(item => new
            {
                item.Frame.ContentFingerprint,
                item.Frame.SourceTop,
                item.Frame.SourceBottom,
            })
            .Select(group => group.First())
            .Take(2)
            .ToArray();
        if (matchingBands.Length != 1)
        {
            return null;
        }

        PaddleCtcSession session;
        try
        {
            session = await _paddleSessionTask.WaitAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            throw;
        }
        catch
        {
            return null;
        }

        var watch = Stopwatch.StartNew();
        var recognized = PaddleTargetRecovery.RecognizePrimary(
            session,
            matchingBands[0].Frame,
            target);
        watch.Stop();
        // This remains a lexical verifier only. Numeric values are deliberately normalized away
        // by the product matcher in both games.
        var verifiedText = target.IsMatch(recognized.Primary.Text)
            ? recognized.Primary.Text
            : recognized.AssistanceKind == PaddleTargetAssistanceKind.CtcTargetSupport &&
              !string.IsNullOrWhiteSpace(recognized.AssistedText)
                ? recognized.AssistedText
                : null;
        if (verifiedText is null)
        {
            return null;
        }

        return new LocalizedRecovery(
            new LogicalAffixMatch(
                StartLineIndex: 0,
                PhysicalLineCount: 1,
                verifiedText,
                target.Template.Text),
            watch.Elapsed);
    }

    private async Task<BatchLocalizedRecovery> TryRecoverLocalizedBandsAsync(
        IReadOnlyList<string> logicalLines,
        IReadOnlyList<WindowsRecognizedBand> recognizedBands,
        IReadOnlyList<FullLineAffixMatcher> targets,
        CancellationToken cancellationToken)
    {
        var jobs = targets
            .Where(target => !target.TryFindMatch(logicalLines, out _))
            .SelectMany(target => recognizedBands
                .Where(item => !item.Frame.IsFallback &&
                               ContainsLocalizedLexicalCandidate(item.Lines, target))
                .Select(band => new BatchRecoveryJob(target, band)))
            .GroupBy(job => CreateBandId(job.Band.Frame), StringComparer.Ordinal)
            .Select(static group => new BatchRecoveryBandJob(
                group.First().Band,
                group.Select(static item => item.Target).ToArray()))
            .Take(2)
            .ToArray();
        if (jobs.Length == 0)
        {
            return BatchLocalizedRecovery.Empty;
        }

        PaddleCtcSession session;
        try
        {
            session = await _paddleSessionTask.WaitAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            throw;
        }
        catch
        {
            return BatchLocalizedRecovery.Empty;
        }

        var watch = Stopwatch.StartNew();
        var assisted = new List<OcrAssistedObservation>();
        var candidates = new List<OcrCandidateEvidence>();
        foreach (var job in jobs)
        {
            _batchLocalizedWorkUnitCount++;
            cancellationToken.ThrowIfCancellationRequested();
            var recognized = session.RecognizeBatch(job.Band.Frame, job.Targets);
            var bandId = CreateBandId(job.Band.Frame);
            foreach (var target in job.Targets)
            {
                var verified = target.IsMatch(recognized.Recognition.Text) ||
                               recognized.TargetSupports.TryGetValue(
                                   target.Template.Text,
                                   out var support) && support.StronglySupported;
                if (verified)
                {
                    assisted.Add(new OcrAssistedObservation(
                        bandId,
                        recognized.Recognition.Text,
                        target.Template.Text,
                        job.Band.Frame.SourceTop,
                        job.Band.Frame.SourceBottom));
                }
                else
                {
                    // Windows localized exactly one glyph-compatible physical row, but
                    // independent CTC did not verify this target. Guarded policy may pause;
                    // normal rule matching always ignores this evidence.
                    candidates.Add(new OcrCandidateEvidence(
                        bandId,
                        string.Join(' ', job.Band.Lines),
                        target.Template.Text,
                        OcrCandidateEvidenceKind.LocalizedLexicalCandidate,
                        job.Band.Frame.SourceTop,
                        job.Band.Frame.SourceBottom));
                }
            }
        }

        watch.Stop();
        return new BatchLocalizedRecovery(assisted, candidates, watch.Elapsed);
    }

    private static IReadOnlyList<OcrPhysicalLine> CreatePhysicalLineMap(
        IReadOnlyList<string> lines,
        IReadOnlyList<WindowsRecognizedBand> recognizedBands)
    {
        var result = new List<OcrPhysicalLine>();
        var searchStart = 0;
        foreach (var band in recognizedBands.Where(static item => !item.Frame.IsFallback))
        {
            foreach (var bandLine in band.Lines)
            {
                var lineIndex = FindLineIndex(lines, bandLine, searchStart);
                if (lineIndex < 0)
                {
                    continue;
                }

                result.Add(new OcrPhysicalLine(
                    lineIndex,
                    CreateBandId(band.Frame),
                    band.Frame.SourceTop,
                    band.Frame.SourceBottom));
                searchStart = lineIndex + 1;
            }
        }

        return result;
    }

    private static int FindLineIndex(
        IReadOnlyList<string> lines,
        string target,
        int start)
    {
        for (var index = start; index < lines.Count; index++)
        {
            if (string.Equals(lines[index], target, StringComparison.Ordinal))
            {
                return index;
            }
        }

        return -1;
    }

    private static string CreateBandId(PreparedFrame band) =>
        $"poe2:{band.ContentFingerprint:x16}:{band.SourceTop}:{band.SourceBottom}:{band.SourceLeft}:{band.SourceRight}";

    /// <summary>
    /// A progressive first pass keeps target-like evidence private while the independent Windows
    /// confirmation is pending. The confirmation pass must surface that preserved evidence so
    /// Guarded can pause instead of accepting an unresolved lexical candidate as a safe miss.
    /// </summary>
    internal static IReadOnlyList<OcrCandidateEvidence> SelectBatchCandidateEvidence(
        bool isConfirmation,
        IReadOnlyList<OcrCandidateEvidence> candidates) =>
        isConfirmation ? candidates : [];

    private static bool HasStrictTarget(OcrRecognitionResult result, FullLineAffixMatcher target) =>
        target.TryFindMatch(result.Lines, out _) ||
        result.TargetAssistedMatch is { } assisted &&
        string.Equals(assisted.CanonicalText, target.Template.Text, StringComparison.Ordinal);

    private static string CreateTargetKey(FullLineAffixMatcher target) =>
        $"{target.MaximumLineSpan}:{target.Template.Text}";

    internal static IReadOnlyList<FullLineAffixMatcher> NormalizeTargetSet(
        IReadOnlyList<FullLineAffixMatcher> targets)
    {
        ArgumentNullException.ThrowIfNull(targets);
        if (targets.Any(static target => target is null))
        {
            throw new ArgumentException("The structured target set contains a null target.", nameof(targets));
        }

        return targets
            .GroupBy(CreateTargetKey, StringComparer.Ordinal)
            .Select(static group => group.First())
            .OrderBy(CreateTargetKey, StringComparer.Ordinal)
            .ToArray();
    }

    internal static string CreateTargetSetKey(IReadOnlyList<FullLineAffixMatcher> targets)
    {
        ArgumentNullException.ThrowIfNull(targets);
        return string.Concat(targets.Select(target =>
        {
            var key = CreateTargetKey(target);
            return $"{key.Length}:{key};";
        }));
    }

    internal static bool HasAnyStrictTarget(
        IReadOnlyList<string> lines,
        IReadOnlyList<FullLineAffixMatcher> targets) =>
        targets.Any(target => target.TryFindMatch(lines, out _));

    private static bool HasOneLocalizedLexicalCandidate(
        IReadOnlyList<string> lines,
        FullLineAffixMatcher target)
    {
        var candidates = EnumerateLocalizedLexicalCandidates(lines, target)
            .Select(candidate => candidate.Text)
            .ToHashSet(StringComparer.Ordinal);
        return candidates.Count == 1;
    }

    private static bool ContainsLocalizedLexicalCandidate(
        IReadOnlyList<string> lines,
        FullLineAffixMatcher target) =>
        EnumerateLocalizedLexicalCandidates(lines, target).Any();

    private static IEnumerable<CanonicalAffix> EnumerateLocalizedLexicalCandidates(
        IReadOnlyList<string> lines,
        FullLineAffixMatcher target)
    {
        var expectedWords = target.Template.Tokens
            .Where(token => token.Kind == AffixTokenKind.Word)
            .Select(token => token.Text)
            .ToArray();
        for (var start = 0; start < lines.Count; start++)
        {
            if (string.IsNullOrWhiteSpace(lines[start]))
            {
                continue;
            }

            var combined = new StringBuilder();
            var maximumSpan = Math.Min(target.MaximumLineSpan, lines.Count - start);
            for (var span = 1; span <= maximumSpan; span++)
            {
                var line = lines[start + span - 1];
                if (string.IsNullOrWhiteSpace(line))
                {
                    break;
                }

                if (combined.Length > 0)
                {
                    combined.Append(' ');
                }

                combined.Append(line.Trim());
                var candidate = AffixCanonicalizer.Normalize(combined.ToString());
                var actualWords = candidate.Tokens
                    .Where(token => token.Kind == AffixTokenKind.Word)
                    .Select(token => token.Text);
                if (IsExactOrGlyphLocalizable(expectedWords, actualWords.ToArray()))
                {
                    yield return candidate;
                }
            }
        }
    }

    private static bool IsExactOrGlyphLocalizable(
        IReadOnlyList<string> expectedWords,
        IReadOnlyList<string> actualWords)
    {
        if (expectedWords.Count != actualWords.Count)
        {
            return false;
        }

        var substitutions = 0;
        for (var wordIndex = 0; wordIndex < expectedWords.Count; wordIndex++)
        {
            var expected = expectedWords[wordIndex];
            var actual = actualWords[wordIndex];
            if (string.Equals(expected, actual, StringComparison.Ordinal))
            {
                continue;
            }

            // Candidate localization is not acceptance. It merely chooses one physical band for
            // strict CTC verification. Keep it character-position preserving, allow no short-word
            // fuzziness and at most two changed glyphs over the complete sentence.
            if (expected.Length < 4 || expected.Length != actual.Length)
            {
                return false;
            }

            for (var characterIndex = 0; characterIndex < expected.Length; characterIndex++)
            {
                if (expected[characterIndex] == actual[characterIndex])
                {
                    continue;
                }

                substitutions++;
                if (substitutions > 2)
                {
                    return false;
                }
            }
        }

        return true;
    }

    private static PaddleCtcSession CreatePackagedPaddleSession()
    {
        var assets = PackagedPaddleOcrAssets.Load();
        return new PaddleCtcSession(
            assets.ModelBytes,
            assets.Dictionary,
            threads: 2,
            TextPolarity.BrightOnDark,
            inkMargin: 2,
            rightPadding: 8,
            allowLatinTargetSupport: true);
    }

    private sealed record PendingNoMatch(
        ulong Fingerprint,
        string TargetKey,
        bool RecoveryAttempted);

    private sealed record ConfirmedNoMatch(
        ulong Fingerprint,
        string TargetKey,
        OcrRecognitionResult Result);

    private sealed record LocalizedRecovery(LogicalAffixMatch Match, TimeSpan Elapsed);

    private sealed record BatchRecoveryJob(
        FullLineAffixMatcher Target,
        WindowsRecognizedBand Band);

    private sealed record BatchRecoveryBandJob(
        WindowsRecognizedBand Band,
        IReadOnlyList<FullLineAffixMatcher> Targets);

    private sealed record BatchLocalizedRecovery(
        IReadOnlyList<OcrAssistedObservation> Assisted,
        IReadOnlyList<OcrCandidateEvidence> Candidates,
        TimeSpan Elapsed)
    {
        public static BatchLocalizedRecovery Empty { get; } = new([], [], TimeSpan.Zero);
    }
}
