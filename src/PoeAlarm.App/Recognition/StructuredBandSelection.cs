using PoeAlarm.Core.Matching;

namespace PoeAlarm.App.Recognition;

/// <summary>
/// Keeps the full-ROI fallback conservative in structured recognition. Canonically identical
/// fallback text is an alternate view of a regular band and must not become a second modifier;
/// unique fallback text remains available when it is the only readable view of an oversized row.
/// </summary>
internal static class StructuredBandSelection
{
    public static IReadOnlyList<int> SelectIncludedIndices(
        IReadOnlyList<PreparedFrame> bands,
        Func<int, string?> recognizedText)
    {
        ArgumentNullException.ThrowIfNull(bands);
        ArgumentNullException.ThrowIfNull(recognizedText);

        var regularCanonical = Enumerable.Range(0, bands.Count)
            .Where(index => !bands[index].IsFallback)
            .Select(index => AffixCanonicalizer.Normalize(recognizedText(index)).Text)
            .Where(static text => !string.IsNullOrWhiteSpace(text))
            .ToHashSet(StringComparer.Ordinal);
        return Enumerable.Range(0, bands.Count)
            .Where(index => !string.IsNullOrWhiteSpace(recognizedText(index)))
            .Where(index => !bands[index].IsFallback ||
                            !regularCanonical.Contains(
                                AffixCanonicalizer.Normalize(recognizedText(index)).Text))
            .ToArray();
    }
}
