using System.Text;

namespace PoeAlarm.Core.Matching;

/// <summary>
/// A compiled full-affix matcher. Every normalized token must occur in the same
/// position; this class never performs substring or keyword matching.
/// </summary>
public sealed class FullLineAffixMatcher
{
    public const int MaximumPhysicalLineSpan = 4;
    public const int MaximumSupportedPhysicalLineSpan = 8;

    private readonly AffixToken[] _templateTokens;
    private readonly int _maximumPhysicalLineSpan;

    public FullLineAffixMatcher(
        string template,
        int maximumPhysicalLineSpan = MaximumPhysicalLineSpan)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(template);
        if (maximumPhysicalLineSpan is < 1 or > MaximumSupportedPhysicalLineSpan)
        {
            throw new ArgumentOutOfRangeException(
                nameof(maximumPhysicalLineSpan),
                $"The line span must be between 1 and {MaximumSupportedPhysicalLineSpan}.");
        }

        SourceTemplate = template.Trim();
        _maximumPhysicalLineSpan = maximumPhysicalLineSpan;
        Template = AffixCanonicalizer.Normalize(template);
        if (!Template.HasWords)
        {
            throw new ArgumentException("An affix template must contain lexical text.", nameof(template));
        }

        _templateTokens = [.. Template.Tokens];
    }

    public string SourceTemplate { get; }

    public CanonicalAffix Template { get; }

    public int MaximumLineSpan => _maximumPhysicalLineSpan;

    public bool IsMatch(string? candidate)
    {
        var normalizedCandidate = AffixCanonicalizer.Normalize(candidate);
        return IsMatch(normalizedCandidate);
    }

    public bool IsMatch(CanonicalAffix candidate)
    {
        ArgumentNullException.ThrowIfNull(candidate);

        var expectedIndex = 0;
        var actualIndex = 0;
        while (true)
        {
            var expectedWordStart = expectedIndex;
            while (expectedIndex < _templateTokens.Length &&
                   _templateTokens[expectedIndex].Kind == AffixTokenKind.Word)
            {
                expectedIndex++;
            }

            var actualWordStart = actualIndex;
            while (actualIndex < candidate.Tokens.Count &&
                   candidate.Tokens[actualIndex].Kind == AffixTokenKind.Word)
            {
                actualIndex++;
            }

            if (!OcrWordComparer.AreRunsEquivalent(
                    _templateTokens,
                    expectedWordStart,
                    expectedIndex,
                    candidate.Tokens,
                    actualWordStart,
                    actualIndex))
            {
                return false;
            }

            var expectedEnded = expectedIndex == _templateTokens.Length;
            var actualEnded = actualIndex == candidate.Tokens.Count;
            if (expectedEnded || actualEnded)
            {
                return expectedEnded && actualEnded;
            }

            // Values remain structural even though their magnitudes are ignored.
            // In particular, a missing leading "<NUM> to" can never be recovered
            // by the word-boundary tolerance below.
            var expectedValue = _templateTokens[expectedIndex];
            var actualValue = candidate.Tokens[actualIndex];
            if (expectedValue.Kind != actualValue.Kind)
            {
                return false;
            }

            expectedIndex++;
            actualIndex++;
        }
    }

    /// <summary>
    /// Tests every adjacent OCR-line group up to the span configured for this matcher.
    /// Blank lines are logical boundaries and are never crossed.
    /// </summary>
    public bool TryFindMatch(
        IReadOnlyList<string> physicalLines,
        out LogicalAffixMatch? match) =>
        TryFindMatch(physicalLines, out match, _maximumPhysicalLineSpan);

    public bool TryFindMatch(
        IReadOnlyList<string> physicalLines,
        out LogicalAffixMatch? match,
        int maximumLineSpan)
    {
        ArgumentNullException.ThrowIfNull(physicalLines);
        if (maximumLineSpan is < 1 or > MaximumSupportedPhysicalLineSpan)
        {
            throw new ArgumentOutOfRangeException(
                nameof(maximumLineSpan),
                $"The line span must be between 1 and {MaximumSupportedPhysicalLineSpan}.");
        }

        for (var start = 0; start < physicalLines.Count; start++)
        {
            if (string.IsNullOrWhiteSpace(physicalLines[start]))
            {
                continue;
            }

            var candidate = new StringBuilder();
            var available = Math.Min(maximumLineSpan, physicalLines.Count - start);
            for (var lineCount = 1; lineCount <= available; lineCount++)
            {
                var line = physicalLines[start + lineCount - 1];
                if (string.IsNullOrWhiteSpace(line))
                {
                    break;
                }

                if (candidate.Length > 0)
                {
                    candidate.Append(' ');
                }

                candidate.Append(line.Trim());
                var candidateText = candidate.ToString();
                var canonicalCandidate = AffixCanonicalizer.Normalize(candidateText);
                if (IsMatch(canonicalCandidate))
                {
                    match = new LogicalAffixMatch(start, lineCount, candidateText, canonicalCandidate.Text);
                    return true;
                }
            }
        }

        match = null;
        return false;
    }

    private static class OcrWordComparer
    {
        public static bool AreRunsEquivalent(
            IReadOnlyList<AffixToken> expectedTokens,
            int expectedStart,
            int expectedEnd,
            IReadOnlyList<AffixToken> actualTokens,
            int actualStart,
            int actualEnd)
        {
            var expectedCount = expectedEnd - expectedStart;
            var actualCount = actualEnd - actualStart;

            if (expectedCount == actualCount)
            {
                var allWordsMatch = true;
                for (var offset = 0; offset < expectedCount; offset++)
                {
                    if (AreEquivalent(
                            expectedTokens[expectedStart + offset].Text,
                            actualTokens[actualStart + offset].Text))
                    {
                        continue;
                    }

                    allWordsMatch = false;
                    break;
                }

                if (allWordsMatch)
                {
                    return true;
                }
            }

            // This fallback is deliberately one-way: it only repairs word
            // boundaries that OCR removed. It cannot accept extra candidate words.
            if (expectedCount < 2 || actualCount == 0 || actualCount >= expectedCount)
            {
                return false;
            }

            return AreMergedRunsEquivalent(
                expectedTokens,
                expectedStart,
                expectedEnd,
                actualTokens,
                actualStart,
                actualEnd);
        }

        public static bool AreEquivalent(string expected, string actual)
        {
            if (string.Equals(expected, actual, StringComparison.Ordinal))
            {
                return true;
            }

            var left = CollapseApostrophesAndRn(expected);
            var right = CollapseApostrophesAndRn(actual);
            if (string.Equals(left, right, StringComparison.Ordinal))
            {
                return true;
            }

            // Avoid fuzzy semantic matching for short words (more/less, to/no, etc.).
            // Long words may differ only through a small set of glyphs OCR commonly
            // confuses. No arbitrary edit distance is used.
            if (left.Length < 4 || left.Length != right.Length)
            {
                return false;
            }

            var confusionCount = 0;
            for (var index = 0; index < left.Length; index++)
            {
                if (left[index] == right[index])
                {
                    continue;
                }

                if (!IsCommonGlyphConfusion(left[index], right[index]))
                {
                    return false;
                }

                confusionCount++;
                if (confusionCount > 2)
                {
                    return false;
                }
            }

            return confusionCount > 0;
        }

        private static bool AreMergedRunsEquivalent(
            IReadOnlyList<AffixToken> expectedTokens,
            int expectedStart,
            int expectedEnd,
            IReadOnlyList<AffixToken> actualTokens,
            int actualStart,
            int actualEnd)
        {
            var expectedCharacters = new List<ExpectedCharacter>();
            var expectedBoundaries = new HashSet<int>();
            for (var tokenIndex = expectedStart; tokenIndex < expectedEnd; tokenIndex++)
            {
                var word = CollapseApostrophesAndRn(expectedTokens[tokenIndex].Text);
                var wordIndex = tokenIndex - expectedStart;
                foreach (var character in word)
                {
                    expectedCharacters.Add(new ExpectedCharacter(character, wordIndex, word.Length));
                }

                if (tokenIndex + 1 < expectedEnd)
                {
                    expectedBoundaries.Add(expectedCharacters.Count);
                }
            }

            var actualCharacters = new List<ActualCharacter>();
            for (var tokenIndex = actualStart; tokenIndex < actualEnd; tokenIndex++)
            {
                var word = CollapseApostrophesAndRn(actualTokens[tokenIndex].Text);
                foreach (var character in word)
                {
                    actualCharacters.Add(new ActualCharacter(character, tokenIndex - actualStart));
                }
            }

            var memo = new Dictionary<(int Expected, int Actual, bool SkippedDigit, int Confusions), bool>();
            return Match(0, 0, skippedDigit: false, confusions: 0);

            bool Match(int expectedOffset, int actualOffset, bool skippedDigit, int confusions)
            {
                if (expectedOffset == expectedCharacters.Count || actualOffset == actualCharacters.Count)
                {
                    return expectedOffset == expectedCharacters.Count && actualOffset == actualCharacters.Count;
                }

                var state = (expectedOffset, actualOffset, skippedDigit, confusions);
                if (memo.TryGetValue(state, out var cached))
                {
                    return cached;
                }

                var expected = expectedCharacters[expectedOffset];
                var actual = actualCharacters[actualOffset];
                var nextConfusions = expectedOffset + 1 < expectedCharacters.Count &&
                                     expectedCharacters[expectedOffset + 1].WordIndex == expected.WordIndex
                    ? confusions
                    : 0;

                if (expected.Value == actual.Value &&
                    Match(expectedOffset + 1, actualOffset + 1, skippedDigit, nextConfusions))
                {
                    memo[state] = true;
                    return true;
                }

                if (expected.WordLength >= 4 &&
                    confusions < 2 &&
                    IsCommonGlyphConfusion(expected.Value, actual.Value))
                {
                    var confusionCountAfterMatch = expectedOffset + 1 < expectedCharacters.Count &&
                                                   expectedCharacters[expectedOffset + 1].WordIndex == expected.WordIndex
                        ? confusions + 1
                        : 0;
                    if (Match(
                            expectedOffset + 1,
                            actualOffset + 1,
                            skippedDigit,
                            confusionCountAfterMatch))
                    {
                        memo[state] = true;
                        return true;
                    }
                }

                // A single stray digit is accepted only when it is embedded between
                // letters in one OCR token and sits exactly at a template word
                // boundary. Letters are never inserted, removed, or reordered.
                if (!skippedDigit &&
                    expectedBoundaries.Contains(expectedOffset) &&
                    IsEmbeddedIsolatedDigit(actualCharacters, actualOffset) &&
                    Match(expectedOffset, actualOffset + 1, skippedDigit: true, confusions))
                {
                    memo[state] = true;
                    return true;
                }

                memo[state] = false;
                return false;
            }
        }

        private static bool IsEmbeddedIsolatedDigit(
            IReadOnlyList<ActualCharacter> characters,
            int index)
        {
            if (!char.IsDigit(characters[index].Value) || index == 0 || index + 1 >= characters.Count)
            {
                return false;
            }

            var tokenIndex = characters[index].TokenIndex;
            return characters[index - 1].TokenIndex == tokenIndex &&
                   characters[index + 1].TokenIndex == tokenIndex &&
                   char.IsLetter(characters[index - 1].Value) &&
                   char.IsLetter(characters[index + 1].Value);
        }

        private static string CollapseApostrophesAndRn(string value)
        {
            var withoutApostrophes = value.Replace("'", string.Empty, StringComparison.Ordinal);
            return withoutApostrophes.Replace("rn", "m", StringComparison.Ordinal);
        }

        private static bool IsCommonGlyphConfusion(char left, char right)
        {
            return InSameGroup(left, right, "il1|!")
                || InSameGroup(left, right, "o0")
                || InSameGroup(left, right, "s5")
                || InSameGroup(left, right, "b8");
        }

        private static bool InSameGroup(char left, char right, string group) =>
            group.Contains(left, StringComparison.Ordinal) && group.Contains(right, StringComparison.Ordinal);

        private readonly record struct ExpectedCharacter(char Value, int WordIndex, int WordLength);

        private readonly record struct ActualCharacter(char Value, int TokenIndex);
    }
}

public sealed record LogicalAffixMatch(
    int StartLineIndex,
    int PhysicalLineCount,
    string OriginalText,
    string CanonicalText);
