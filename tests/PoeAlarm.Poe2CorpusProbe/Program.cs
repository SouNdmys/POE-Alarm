using System.Text.Json;
using System.Text.RegularExpressions;
using PoeAlarm.Core.Matching;

const string DefaultCorpus = @"tests\screenshots\poe2\poe2db-affix-corpus.json";
const int MaximumPhysicalLineSpan = FullLineAffixMatcher.MaximumSupportedPhysicalLineSpan;
string[] Locales = ["en-US", "zh-TW"];

var corpusPath = Path.GetFullPath(args.Length == 0 ? DefaultCorpus : args[0]);
if (!File.Exists(corpusPath))
{
    Console.Error.WriteLine($"Corpus not found: {corpusPath}");
    return 2;
}

var corpus = JsonSerializer.Deserialize<Corpus>(
                 await File.ReadAllTextAsync(corpusPath),
                 new JsonSerializerOptions { PropertyNameCaseInsensitive = true })
             ?? throw new InvalidDataException("POE2 corpus is empty.");

var hardFailures = new List<string>();
ValidateSchema(corpus, hardFailures);

Console.WriteLine("POE Alarm POE2 bilingual production-matcher corpus probe");
Console.WriteLine($"Corpus: {corpusPath}");
Console.WriteLine($"Cases:  {corpus.Cases.Count}");
Console.WriteLine();

var positiveCount = 0;
var positiveFailures = new List<string>();
var missingLineCount = 0;
var missingLineFalseMatches = new List<string>();
var numericTemplateCount = 0;
var numericTemplateCompatible = 0;
var numericTemplateFailures = new List<string>();
var rangeMaterializationCount = 0;
var rangeMaterializationFailures = new List<string>();
var physicalVariantCount = 0;
var physicalVariantFailures = new List<string>();

foreach (var item in corpus.Cases)
{
    foreach (var locale in Locales)
    {
        if (!TryGetLines(item.DisplayLines, locale, out var displayLines) ||
            !TryGetLines(item.NumericTemplates, locale, out var numericLines))
        {
            continue;
        }

        var displayMatcher = CreateMatcher(displayLines);
        positiveCount++;
        if (!displayMatcher.TryFindMatch(displayLines, out _))
        {
            positiveFailures.Add($"{locale}/{item.Id}: full display line did not match itself");
        }

        for (var omitted = 0; omitted < displayLines.Count; omitted++)
        {
            missingLineCount++;
            var incomplete = displayLines.Where((_, index) => index != omitted).ToArray();
            if (displayMatcher.TryFindMatch(incomplete, out var unexpected))
            {
                missingLineFalseMatches.Add(
                    $"{locale}/{item.Id}: omitted line {omitted + 1}, matched '{unexpected!.OriginalText}'");
            }
        }

        numericTemplateCount++;
        var numericMatcher = CreateMatcher(numericLines);
        if (numericMatcher.TryFindMatch(displayLines, out _))
        {
            numericTemplateCompatible++;
        }
        else
        {
            numericTemplateFailures.Add(
                $"{locale}/{item.Id}: {numericMatcher.Template.Text} != {displayMatcher.Template.Text}");
        }

        var materializedRanges = MaterializeRollRanges(displayLines);
        if (!materializedRanges.SequenceEqual(displayLines, StringComparer.Ordinal))
        {
            rangeMaterializationCount++;
            if (!displayMatcher.TryFindMatch(materializedRanges, out _))
            {
                rangeMaterializationFailures.Add(
                    $"{locale}/{item.Id}: concrete values no longer match the PoE2DB range template");
            }
        }

        if (item.PhysicalLineVariants is not null &&
            item.PhysicalLineVariants.TryGetValue(locale, out var variants))
        {
            foreach (var variant in variants)
            {
                physicalVariantCount++;
                if (!displayMatcher.TryFindMatch(variant, out _))
                {
                    physicalVariantFailures.Add(
                        $"{locale}/{item.Id}: {variant.Count}-line physical wrap did not match");
                }
            }
        }
    }
}

Console.WriteLine("Hard invariants");
Console.WriteLine(
    $"  full-display positives: {positiveCount - positiveFailures.Count}/{positiveCount}");
Console.WriteLine(
    $"  one-line-omitted negatives: {missingLineCount - missingLineFalseMatches.Count}/{missingLineCount}");
Console.WriteLine(
    $"  numeric-template compatibility (diagnostic): {numericTemplateCompatible}/{numericTemplateCount}");
Console.WriteLine(
    $"  roll-range concrete-value positives: {rangeMaterializationCount - rangeMaterializationFailures.Count}/{rangeMaterializationCount}");
Console.WriteLine(
    $"  physical-wrap positives (max {MaximumPhysicalLineSpan} lines): {physicalVariantCount - physicalVariantFailures.Count}/{physicalVariantCount}");
Console.WriteLine();

var pairAudits = new List<PairAudit>();
var fullTemplatePairAudits = new List<PairAudit>();
foreach (var locale in Locales)
{
    for (var leftIndex = 0; leftIndex < corpus.Cases.Count; leftIndex++)
    {
        var left = corpus.Cases[leftIndex];
        if (!TryGetLines(left.DisplayLines, locale, out var leftLines))
        {
            continue;
        }

        var leftMatcher = CreateMatcher(leftLines);
        for (var rightIndex = leftIndex + 1; rightIndex < corpus.Cases.Count; rightIndex++)
        {
            var right = corpus.Cases[rightIndex];
            if (!TryGetLines(right.DisplayLines, locale, out var rightLines))
            {
                continue;
            }

            var rightMatcher = CreateMatcher(rightLines);
            var leftMatchesRight = leftMatcher.TryFindMatch(rightLines, out _);
            var rightMatchesLeft = rightMatcher.TryFindMatch(leftLines, out _);
            var leftMatchesFullRight = leftMatcher.IsMatch(string.Join(' ', rightLines));
            var rightMatchesFullLeft = rightMatcher.IsMatch(string.Join(' ', leftLines));
            if (leftMatchesFullRight || rightMatchesFullLeft)
            {
                fullTemplatePairAudits.Add(new PairAudit(
                    locale,
                    left,
                    right,
                    leftMatchesFullRight,
                    rightMatchesFullLeft,
                    leftMatcher.Template.Text == rightMatcher.Template.Text));
            }

            if (!leftMatchesRight && !rightMatchesLeft)
            {
                continue;
            }

            pairAudits.Add(new PairAudit(
                locale,
                left,
                right,
                leftMatchesRight,
                rightMatchesLeft,
                leftMatcher.Template.Text == rightMatcher.Template.Text));
        }
    }
}

var directedCollisions = pairAudits.Sum(pair =>
    (pair.LeftMatchesRight ? 1 : 0) + (pair.RightMatchesLeft ? 1 : 0));
var waystonePairs = pairAudits.Where(pair =>
    pair.Left.Category.Equals("waystone", StringComparison.OrdinalIgnoreCase) ||
    pair.Right.Category.Equals("waystone", StringComparison.OrdinalIgnoreCase)).ToArray();

Console.WriteLine("Pairwise cross-affix collision audit");
Console.WriteLine(
    $"  unordered pairs checked: {LocalPairCount(corpus.Cases.Count) * Locales.Length:N0}");
Console.WriteLine(
    $"  colliding unordered pairs: {pairAudits.Count:N0}; directed matches: {directedCollisions:N0}");
Console.WriteLine(
    $"  full-template collisions: {fullTemplatePairAudits.Count:N0} unordered / " +
    $"{fullTemplatePairAudits.Sum(pair => (pair.LeftMatchesRight ? 1 : 0) + (pair.RightMatchesLeft ? 1 : 0)):N0} directed");
Console.WriteLine($"  canonical-identical pairs: {pairAudits.Count(pair => pair.CanonicalIdentical):N0}");
Console.WriteLine($"  pairs involving Waystones: {waystonePairs.Length:N0}");
foreach (var group in pairAudits
             .GroupBy(pair => pair.Locale)
             .OrderBy(group => group.Key, StringComparer.Ordinal))
{
    Console.WriteLine($"  {group.Key}: {group.Count():N0} unordered / " +
                      $"{group.Sum(pair => (pair.LeftMatchesRight ? 1 : 0) + (pair.RightMatchesLeft ? 1 : 0)):N0} directed");
}
Console.WriteLine();

foreach (var failure in hardFailures
             .Concat(positiveFailures)
             .Concat(missingLineFalseMatches)
             .Concat(rangeMaterializationFailures)
             .Concat(physicalVariantFailures)
             .Take(20))
{
    Console.WriteLine($"FAIL {failure}");
}
if (numericTemplateFailures.Count > 0)
{
    Console.WriteLine(
        $"NOTE {numericTemplateFailures.Count} numericTemplates are not directly consumable by the current matcher; " +
        "the product path uses the concrete PoE2DB display text pasted by the user.");
    foreach (var failure in numericTemplateFailures.Take(5))
    {
        Console.WriteLine($"  {failure}");
    }
}

var failures = hardFailures.Count +
               positiveFailures.Count +
               missingLineFalseMatches.Count +
               rangeMaterializationFailures.Count +
               physicalVariantFailures.Count;
Console.WriteLine();
Console.WriteLine(failures == 0
    ? "Validation: PASS (cross-affix collisions are diagnostic findings, not hidden failures)."
    : $"Validation: FAIL ({failures} hard failures)." );
return failures == 0 ? 0 : 1;

static FullLineAffixMatcher CreateMatcher(IReadOnlyList<string> lines) =>
    new(
        string.Join(' ', lines),
        MaximumPhysicalLineSpan);

static IReadOnlyList<string> MaterializeRollRanges(IReadOnlyList<string> lines) =>
    lines.Select(line => Regex.Replace(
        line,
        @"\(\s*([+-]?\d+(?:[\.,]\d+)?)\s*[—–-]\s*[+-]?\d+(?:[\.,]\d+)?\s*\)",
        "$1",
        RegexOptions.CultureInvariant)).ToArray();

static bool TryGetLines(
    IReadOnlyDictionary<string, IReadOnlyList<string>> localized,
    string locale,
    out IReadOnlyList<string> lines)
{
    if (localized.TryGetValue(locale, out var value) && value.Count > 0)
    {
        lines = value;
        return true;
    }

    lines = [];
    return false;
}

void ValidateSchema(Corpus corpus, List<string> failures)
{
    if (corpus.Cases.Count == 0)
    {
        failures.Add("corpus contains no cases");
        return;
    }

    var duplicateIds = corpus.Cases
        .GroupBy(item => item.Id, StringComparer.Ordinal)
        .Where(group => group.Count() > 1)
        .Select(group => group.Key)
        .ToArray();
    if (duplicateIds.Length > 0)
    {
        failures.Add($"duplicate ids: {string.Join(", ", duplicateIds)}");
    }

    foreach (var item in corpus.Cases)
    {
        foreach (var locale in Locales)
        {
            if (!TryGetLines(item.DisplayLines, locale, out var display))
            {
                failures.Add($"{item.Id}: missing displayLines[{locale}]");
                continue;
            }

            if (!TryGetLines(item.NumericTemplates, locale, out var templates))
            {
                failures.Add($"{item.Id}: missing numericTemplates[{locale}]");
                continue;
            }

            if (display.Count != templates.Count)
            {
                failures.Add(
                    $"{item.Id}/{locale}: display/template line count differs ({display.Count}/{templates.Count})");
            }

            if (display.Count > MaximumPhysicalLineSpan)
            {
                failures.Add(
                    $"{item.Id}/{locale}: {display.Count} display lines exceed production span " +
                    MaximumPhysicalLineSpan);
            }

            if (item.PhysicalLineVariants is null ||
                !item.PhysicalLineVariants.TryGetValue(locale, out var variants))
            {
                continue;
            }

            foreach (var variant in variants)
            {
                if (variant.Count is < 1 or > MaximumPhysicalLineSpan)
                {
                    failures.Add(
                        $"{item.Id}/{locale}: physical variant has {variant.Count} lines; " +
                        $"expected 1..{MaximumPhysicalLineSpan}");
                }
            }
        }
    }
}

static long LocalPairCount(int count) => (long)count * (count - 1) / 2;

internal sealed record Corpus(
    IReadOnlyList<CorpusCase> Cases);

internal sealed record CorpusCase(
    string Id,
    string PairKey,
    string Category,
    IReadOnlyDictionary<string, IReadOnlyList<string>> DisplayLines,
    IReadOnlyDictionary<string, IReadOnlyList<string>> NumericTemplates,
    IReadOnlyList<string> RiskTags,
    IReadOnlyDictionary<string, IReadOnlyList<IReadOnlyList<string>>>? PhysicalLineVariants = null);

internal sealed record PairAudit(
    string Locale,
    CorpusCase Left,
    CorpusCase Right,
    bool LeftMatchesRight,
    bool RightMatchesLeft,
    bool CanonicalIdentical);
