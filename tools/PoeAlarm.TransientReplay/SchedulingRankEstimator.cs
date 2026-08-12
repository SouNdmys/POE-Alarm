using System.Text;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.TransientReplay;

/// <summary>Mirrors the production target-width ordering for benchmark case classification.</summary>
internal static class SchedulingRankEstimator
{
    private static readonly PoeTextPreprocessorSettings Settings = new(
        MinimumBlue: 100,
        MinimumBlueDominance: 14,
        MaximumWarmChannelDifference: 72,
        IntensityMode: BlueMaskIntensityMode.Dominance);

    public static int EstimateMinimumRank(
        CapturedFrame frame,
        FullLineAffixMatcher target,
        IReadOnlyList<int> targetBandIndices)
    {
        var targetIndexes = targetBandIndices.ToHashSet();
        var bands = new PoeTextPreprocessor(Settings).PrepareBlueAffixBands(frame, scale: 1);
        var ranked = bands
            .Select((band, index) => new
            {
                Index = index,
                Distance = TargetWidthDistance(band, target, frame.Width),
            })
            .OrderBy(item => item.Distance)
            .ThenBy(item => item.Index)
            .Select((item, zeroBasedRank) => new { item.Index, Rank = zeroBasedRank + 1 })
            .Where(item => targetIndexes.Contains(item.Index))
            .Select(item => item.Rank)
            .ToArray();
        return ranked.Length == 0
            ? throw new InvalidOperationException("Target band is absent from scheduling rank input.")
            : ranked.Min();
    }

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
}
