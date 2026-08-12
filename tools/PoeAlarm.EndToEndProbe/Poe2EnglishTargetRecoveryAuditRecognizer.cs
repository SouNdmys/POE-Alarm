using System.Diagnostics;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;

/// <summary>
/// Test-only prototype for the narrow POE2 English recovery route.
///
/// Windows OCR remains authoritative. Paddle is consulted only when Windows found exactly one
/// logical line with the target's complete lexical word sequence but failed the strict match
/// because the numeric structure was lost. Paddle must then independently produce a normal
/// strict/target-supported full-line match; lexical similarity is never accepted as a hit.
/// </summary>
internal sealed class Poe2EnglishTargetRecoveryAuditRecognizer : IOcrRecognizer, IFrameFingerprintProvider
{
    private readonly WindowsOcrRecognizer _windows = new(
        scale: 1,
        language: OcrRecognitionLanguage.English);
    private PaddleOcrRecognizer? _paddle;

    public int RecoveryAttempts { get; private set; }

    public int RecoveryMatches { get; private set; }

    public List<double> RecoveryWallMilliseconds { get; } = [];

    public ulong ComputeFrameFingerprint(CapturedFrame frame) =>
        new PoeTextPreprocessor().ComputeBlueTextFingerprint(frame);

    public Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        CancellationToken cancellationToken = default) =>
        _windows.RecognizeAsync(frame, cancellationToken);

    public async Task<OcrRecognitionResult> RecognizeAsync(
        CapturedFrame frame,
        FullLineAffixMatcher target,
        CancellationToken cancellationToken = default)
    {
        var primary = await _windows
            .RecognizeAsync(frame, cancellationToken)
            .ConfigureAwait(false);
        if (target.TryFindMatch(primary.Lines, out _))
        {
            return primary;
        }

        // Candidate localization only. This deliberately requires exact lexical words; it does
        // not use edit distance or the shared matcher's glyph-confusion tolerance. Duplicate
        // fallback transcripts are collapsed by normalized text before uniqueness is checked.
        if (!HasOneExactLexicalCandidate(primary.Lines, target))
        {
            return primary;
        }

        RecoveryAttempts++;
        _paddle ??= new PaddleOcrRecognizer();
        var watch = Stopwatch.StartNew();
        LogicalAffixMatch? verified = null;
        var totalPreprocessing = primary.PreprocessingElapsed;
        var totalRecognition = primary.RecognitionElapsed;
        for (var slice = 0; slice < 128; slice++)
        {
            var fallback = await _paddle
                .RecognizeAsync(frame, target, cancellationToken)
                .ConfigureAwait(false);
            totalPreprocessing += fallback.PreprocessingElapsed;
            totalRecognition += fallback.RecognitionElapsed;
            target.TryFindMatch(fallback.Lines, out verified);
            verified ??= fallback.TargetAssistedMatch is { } assisted &&
                         string.Equals(
                             assisted.CanonicalText,
                             target.Template.Text,
                             StringComparison.Ordinal)
                ? assisted
                : null;
            if (verified is not null || !fallback.RequiresRescan)
            {
                break;
            }
        }

        watch.Stop();
        RecoveryWallMilliseconds.Add(watch.Elapsed.TotalMilliseconds);
        if (verified is not null)
        {
            RecoveryMatches++;
        }

        return primary with
        {
            PreprocessingElapsed = totalPreprocessing,
            RecognitionElapsed = totalRecognition,
            WasCached = false,
            TargetAssistedMatch = verified,
            RequiresRescan = false,
        };
    }

    public void Dispose()
    {
        _paddle?.Dispose();
        _windows.Dispose();
    }

    private static bool HasOneExactLexicalCandidate(
        IReadOnlyList<string> lines,
        FullLineAffixMatcher target)
    {
        var expectedWords = target.Template.Tokens
            .Where(token => token.Kind == AffixTokenKind.Word)
            .Select(token => token.Text)
            .ToArray();
        var candidates = new HashSet<string>(StringComparer.Ordinal);
        for (var start = 0; start < lines.Count; start++)
        {
            if (string.IsNullOrWhiteSpace(lines[start]))
            {
                continue;
            }

            var combined = string.Empty;
            var available = Math.Min(FullLineAffixMatcher.MaximumPhysicalLineSpan, lines.Count - start);
            for (var span = 1; span <= available; span++)
            {
                var line = lines[start + span - 1];
                if (string.IsNullOrWhiteSpace(line))
                {
                    break;
                }

                combined = combined.Length == 0 ? line.Trim() : $"{combined} {line.Trim()}";
                var candidate = AffixCanonicalizer.Normalize(combined);
                var actualWords = candidate.Tokens
                    .Where(token => token.Kind == AffixTokenKind.Word)
                    .Select(token => token.Text);
                if (expectedWords.SequenceEqual(actualWords, StringComparer.Ordinal))
                {
                    candidates.Add(candidate.Text);
                }
            }
        }

        return candidates.Count == 1;
    }
}
