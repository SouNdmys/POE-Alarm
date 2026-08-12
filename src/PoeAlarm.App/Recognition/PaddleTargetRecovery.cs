using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Recognition;

internal static class PaddleTargetRecovery
{
    private const int ShortSegmentWidth = 320;
    private const int LongSegmentWidth = 400;

    public static PaddleTargetRecognition Recognize(
        PaddleCtcSession session,
        PreparedFrame frame,
        FullLineAffixMatcher? target)
    {
        var primary = RecognizePrimary(session, frame, target);
        return target is null
            ? primary
            : CompleteSegmentation(session, frame, target, primary);
    }

    public static PaddleTargetRecognition RecognizePrimary(
        PaddleCtcSession session,
        PreparedFrame frame,
        FullLineAffixMatcher? target)
    {
        var primary = session.Recognize(frame, target?.SourceTemplate);
        if (target is null || target.IsMatch(primary.Text))
        {
            return new PaddleTargetRecognition(
                primary,
                AssistedText: null,
                PaddleTargetAssistanceKind.None);
        }

        if (primary.TargetSupport?.StronglySupported == true)
        {
            return new PaddleTargetRecognition(
                primary,
                primary.Text,
                PaddleTargetAssistanceKind.CtcTargetSupport);
        }

        return new PaddleTargetRecognition(
            primary,
            AssistedText: null,
            PaddleTargetAssistanceKind.None);
    }

    public static PaddleTargetRecognition CompleteSegmentation(
        PaddleCtcSession session,
        PreparedFrame frame,
        FullLineAffixMatcher target,
        PaddleTargetRecognition preliminary)
    {
        var work = CreateSegmentationWork(preliminary, target);
        while (!work.IsComplete)
        {
            work = CompleteNextSegmentation(session, frame, target, work);
        }

        return work.Recognition;
    }

    /// <summary>
    /// Creates a deterministic recovery plan without running another model inference.  Live
    /// monitoring advances this plan one entry at a time so a similar target-absent row cannot
    /// monopolize the capture loop with every segmentation fallback in one scan.
    /// </summary>
    public static PaddleTargetRecoveryWork CreateSegmentationWork(
        PaddleTargetRecognition preliminary,
        FullLineAffixMatcher target)
    {
        ArgumentNullException.ThrowIfNull(preliminary);
        ArgumentNullException.ThrowIfNull(target);

        if (preliminary.AssistanceKind == PaddleTargetAssistanceKind.SegmentedStrict ||
            target.IsMatch(preliminary.Primary.Text))
        {
            return PaddleTargetRecoveryWork.Completed(preliminary);
        }

        var primary = preliminary.Primary;
        // Strong target-conditioned CTC evidence is exactly the case where an independent,
        // strictly matched segmented transcript is valuable.  It must still remain merely an
        // assisted candidate when segmentation does not produce an exact result.
        if (preliminary.AssistanceKind != PaddleTargetAssistanceKind.CtcTargetSupport &&
            !CouldBenefitFromSegmentation(primary, target))
        {
            return PaddleTargetRecoveryWork.Completed(preliminary);
        }

        // Very long, low-confidence rows are the one case where the wider segmentation has
        // consistently recovered the missing tail. Trying 320 first used to add another full
        // inference (roughly 30-40 ms) before the useful 400-wide pass.
        var preferLongSegments =
            primary.TensorWidth >= 800 && primary.MeanConfidence < 0.82;
        var widths = new List<int>(capacity: 2);
        if (preferLongSegments && primary.TensorWidth > LongSegmentWidth)
        {
            widths.Add(LongSegmentWidth);
        }

        if (primary.TensorWidth > ShortSegmentWidth)
        {
            widths.Add(ShortSegmentWidth);
        }

        if (!preferLongSegments &&
            primary.TensorWidth > LongSegmentWidth &&
            (primary.TensorWidth >= 720 || primary.MeanConfidence < 0.75))
        {
            widths.Add(LongSegmentWidth);
        }

        return widths.Count == 0
            ? PaddleTargetRecoveryWork.Completed(preliminary)
            : new PaddleTargetRecoveryWork(preliminary, widths.ToArray(), NextWidthIndex: 0);
    }

    /// <summary>Runs at most one configured segmentation width.</summary>
    public static PaddleTargetRecoveryWork CompleteNextSegmentation(
        PaddleCtcSession session,
        PreparedFrame frame,
        FullLineAffixMatcher target,
        PaddleTargetRecoveryWork work)
    {
        ArgumentNullException.ThrowIfNull(session);
        ArgumentNullException.ThrowIfNull(frame);
        ArgumentNullException.ThrowIfNull(target);
        ArgumentNullException.ThrowIfNull(work);

        if (work.IsComplete)
        {
            return work;
        }

        var maximumWidth = work.SegmentWidths[work.NextWidthIndex];
        var segmented = session.Recognize(
            frame,
            target.SourceTemplate,
            maximumSegmentWidthOverride: maximumWidth);
        var nextIndex = work.NextWidthIndex + 1;
        if (!target.IsMatch(segmented.Text))
        {
            return work with { NextWidthIndex = nextIndex };
        }

        return work with
        {
            Recognition = new PaddleTargetRecognition(
                work.Recognition.Primary,
                segmented.Text,
                PaddleTargetAssistanceKind.SegmentedStrict),
            NextWidthIndex = work.SegmentWidths.Count,
        };
    }

    private static bool CouldBenefitFromSegmentation(
        CtcRecognition recognition,
        FullLineAffixMatcher target)
    {
        if (recognition.TensorWidth >= 800 && recognition.MeanConfidence < 0.82)
        {
            return true;
        }

        var actual = AffixCanonicalizer.Normalize(recognition.Text).Tokens;
        var expected = target.Template.Tokens;
        var allowance = Math.Clamp(expected.Count / 8, 2, 4);
        if (Math.Abs(expected.Count - actual.Count) > allowance)
        {
            return false;
        }

        return TokenEditDistance(expected, actual, allowance) <= allowance;
    }

    private static int TokenEditDistance(
        IReadOnlyList<AffixToken> expected,
        IReadOnlyList<AffixToken> actual,
        int stopAfter)
    {
        var previous = Enumerable.Range(0, actual.Count + 1).ToArray();
        var current = new int[actual.Count + 1];
        for (var row = 1; row <= expected.Count; row++)
        {
            current[0] = row;
            var rowMinimum = current[0];
            for (var column = 1; column <= actual.Count; column++)
            {
                var substitution = expected[row - 1].Equals(actual[column - 1]) ? 0 : 1;
                current[column] = Math.Min(
                    Math.Min(current[column - 1] + 1, previous[column] + 1),
                    previous[column - 1] + substitution);
                rowMinimum = Math.Min(rowMinimum, current[column]);
            }

            if (rowMinimum > stopAfter)
            {
                return rowMinimum;
            }

            (previous, current) = (current, previous);
        }

        return previous[^1];
    }
}

internal sealed record PaddleTargetRecognition(
    CtcRecognition Primary,
    string? AssistedText,
    PaddleTargetAssistanceKind AssistanceKind);

internal sealed record PaddleTargetRecoveryWork(
    PaddleTargetRecognition Recognition,
    IReadOnlyList<int> SegmentWidths,
    int NextWidthIndex)
{
    public bool IsComplete => NextWidthIndex >= SegmentWidths.Count;

    public static PaddleTargetRecoveryWork Completed(PaddleTargetRecognition recognition) =>
        new(recognition, [], NextWidthIndex: 0);
}

internal enum PaddleTargetAssistanceKind
{
    None,
    CtcTargetSupport,
    SegmentedStrict,
}
