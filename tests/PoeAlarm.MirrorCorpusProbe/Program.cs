using System.Text.Json;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Rules;

const string DefaultCorpus = @"tests\fixtures\mirror-tier-composite-rules.json";

var corpusPath = Path.GetFullPath(args.Length == 0 ? DefaultCorpus : args[0]);
if (!File.Exists(corpusPath))
{
    Console.Error.WriteLine($"Corpus not found: {corpusPath}");
    return 2;
}

var options = new JsonSerializerOptions
{
    PropertyNameCaseInsensitive = true,
};
var corpus = JsonSerializer.Deserialize<MirrorCorpus>(
                 await File.ReadAllTextAsync(corpusPath),
                 options)
             ?? throw new InvalidDataException("Mirror-tier corpus is empty.");

var failures = new List<string>();
ValidateCorpus(corpus, failures);

var evaluated = 0;
var matchedAsExpected = 0;
var positiveCount = 0;
var negativeCount = 0;
var nearNegativeCount = 0;
var oneShortCount = 0;
var boundaryCount = 0;
var oneToOneCount = 0;
var identityContractCount = 0;

foreach (var scenario in corpus.Scenarios)
{
    CompiledRuleSet rules;
    try
    {
        rules = RuleCompiler.Compile(scenario.RuleSet);
    }
    catch (Exception exception)
    {
        failures.Add($"{scenario.Id}: rule compilation failed: {exception.Message}");
        continue;
    }

    foreach (var item in scenario.Cases)
    {
        evaluated++;
        positiveCount += item.ExpectedMatch ? 1 : 0;
        negativeCount += item.ExpectedMatch ? 0 : 1;
        nearNegativeCount += item.Tags.Contains("near-neighbour-negative", StringComparer.Ordinal) ? 1 : 0;
        oneShortCount += item.Tags.Contains("one-short-negative", StringComparer.Ordinal) ? 1 : 0;
        boundaryCount += item.Tags.Any(tag => tag.StartsWith("numeric-boundary", StringComparison.Ordinal)) ? 1 : 0;
        oneToOneCount += item.Tags.Contains("one-modifier-one-count", StringComparer.Ordinal) ? 1 : 0;

        var ocrResult = CreateOcrResult(item);
        var result = EvaluateRules(rules, ocrResult);
        if (item.Tags.Contains("one-modifier-one-count", StringComparer.Ordinal))
        {
            identityContractCount++;
            var withoutPrimaryIdentity = rules.Evaluate(
                ocrResult.Lines,
                ToAssistedObservations(ocrResult),
                []);
            if (!withoutPrimaryIdentity.IsMatch || result.IsMatch)
            {
                failures.Add(
                    $"{scenario.Id}/{item.Id}: identity-contract case is not discriminating; " +
                    $"expected match without primary physical identity and no match with identity, " +
                    $"got without={withoutPrimaryIdentity.IsMatch}, with={result.IsMatch}.");
            }
        }

        var actualGroup = result.MatchedGroupId;
        var groupMatches = item.ExpectedGroup is null ||
                           string.Equals(item.ExpectedGroup, actualGroup, StringComparison.Ordinal);
        if (result.IsMatch == item.ExpectedMatch && groupMatches)
        {
            matchedAsExpected++;
            Console.WriteLine($"PASS  {scenario.Id}/{item.Id}");
            continue;
        }

        failures.Add(
            $"{scenario.Id}/{item.Id}: expected match={item.ExpectedMatch}, " +
            $"group='{item.ExpectedGroup ?? "<none>"}'; got match={result.IsMatch}, " +
            $"group='{actualGroup ?? "<none>"}'. Evidence: {Describe(result)}");
        Console.WriteLine($"FAIL  {scenario.Id}/{item.Id}");
    }
}

Console.WriteLine();
Console.WriteLine("POE Alarm real mirror-tier composite-rule corpus probe");
Console.WriteLine($"Corpus:                   {corpusPath}");
Console.WriteLine($"Sources:                  {corpus.Sources.Count}");
Console.WriteLine($"Scenarios:                {corpus.Scenarios.Count}");
Console.WriteLine($"Actual mirror scenarios:  {corpus.Scenarios.Count(item => item.Basis == "actual-mirror-item")}");
Console.WriteLine($"Cases:                    {matchedAsExpected}/{evaluated}");
Console.WriteLine($"Positive / negative:      {positiveCount} / {negativeCount}");
Console.WriteLine($"Near-neighbour negatives: {nearNegativeCount}");
Console.WriteLine($"One-short negatives:      {oneShortCount}");
Console.WriteLine($"Numeric boundary cases:   {boundaryCount}");
Console.WriteLine($"One-modifier-one-count:   {oneToOneCount}");
Console.WriteLine($"OCR identity contracts:   {identityContractCount}");
Console.WriteLine();

if (failures.Count == 0)
{
    Console.WriteLine("PASS  all corpus cases and coverage invariants");
    return 0;
}

foreach (var failure in failures)
{
    Console.Error.WriteLine($"FAIL  {failure}");
}

return 1;

static void ValidateCorpus(MirrorCorpus corpus, ICollection<string> failures)
{
    if (corpus.Version != 1)
    {
        failures.Add($"Unsupported corpus version {corpus.Version}.");
    }

    if (!DateOnly.TryParse(corpus.GeneratedAt, out _))
    {
        failures.Add("generatedAt must be an ISO date.");
    }

    if (corpus.Sources.Count < 8)
    {
        failures.Add("At least eight independent source records are required.");
    }

    var duplicateSource = corpus.Sources
        .GroupBy(source => source.Id, StringComparer.Ordinal)
        .FirstOrDefault(group => group.Count() > 1);
    if (duplicateSource is not null)
    {
        failures.Add($"Duplicate source id '{duplicateSource.Key}'.");
    }

    var sourceById = corpus.Sources
        .GroupBy(source => source.Id, StringComparer.Ordinal)
        .ToDictionary(group => group.Key, group => group.First(), StringComparer.Ordinal);
    foreach (var source in corpus.Sources)
    {
        if (!Uri.TryCreate(source.Url, UriKind.Absolute, out var uri) || uri.Scheme != Uri.UriSchemeHttps)
        {
            failures.Add($"{source.Id}: source URL must be absolute HTTPS.");
        }

        if (!DateOnly.TryParse(source.AccessedAt, out _))
        {
            failures.Add($"{source.Id}: accessedAt must be an ISO date.");
        }

        if (source.Game is not ("POE1" or "POE2") || string.IsNullOrWhiteSpace(source.EquipmentSlot))
        {
            failures.Add($"{source.Id}: game and equipmentSlot are required.");
        }
    }

    var duplicateScenario = corpus.Scenarios
        .GroupBy(scenario => scenario.Id, StringComparer.Ordinal)
        .FirstOrDefault(group => group.Count() > 1);
    if (duplicateScenario is not null)
    {
        failures.Add($"Duplicate scenario id '{duplicateScenario.Key}'.");
    }

    foreach (var scenario in corpus.Scenarios)
    {
        if (scenario.Game is not ("POE1" or "POE2"))
        {
            failures.Add($"{scenario.Id}: game must be POE1 or POE2.");
        }

        if (scenario.SourceIds.Count == 0 || scenario.Cases.Count == 0)
        {
            failures.Add($"{scenario.Id}: sources and cases are required.");
        }

        foreach (var sourceId in scenario.SourceIds)
        {
            if (!sourceById.TryGetValue(sourceId, out var source))
            {
                failures.Add($"{scenario.Id}: unknown source '{sourceId}'.");
            }
            else if (!string.Equals(source.Game, scenario.Game, StringComparison.Ordinal))
            {
                failures.Add($"{scenario.Id}: source '{sourceId}' belongs to {source.Game}.");
            }
        }

        var duplicateCase = scenario.Cases
            .GroupBy(item => item.Id, StringComparer.Ordinal)
            .FirstOrDefault(group => group.Count() > 1);
        if (duplicateCase is not null)
        {
            failures.Add($"{scenario.Id}: duplicate case id '{duplicateCase.Key}'.");
        }

        foreach (var condition in scenario.RuleSet.Groups.SelectMany(group => group.Conditions))
        {
            if (condition.Template.Contains("fractured", StringComparison.OrdinalIgnoreCase) ||
                condition.Template.Contains("破裂", StringComparison.Ordinal))
            {
                failures.Add(
                    $"{scenario.Id}: fractured provenance is Alt-only and must not be a rule target.");
            }
        }

        if (scenario.Basis == "actual-mirror-item" &&
            !scenario.Cases.Any(item => item.ExpectedMatch &&
                                             item.Tags.Contains("actual-listing", StringComparer.Ordinal)))
        {
            failures.Add(
                $"{scenario.Id}: an actual mirror scenario needs an explicitly tagged listing case.");
        }
    }

    foreach (var game in new[] { "POE1", "POE2" })
    {
        var mirrorScenarios = corpus.Scenarios.Count(scenario =>
            scenario.Game == game && scenario.Basis == "actual-mirror-item");
        if (mirrorScenarios < 2)
        {
            failures.Add($"{game}: expected at least two actual mirror-item scenarios.");
        }
    }

    if (!corpus.Scenarios.Any(scenario => scenario.RuleSet.Groups.Count > 1))
    {
        failures.Add("No scenario exercises OR between acceptable results.");
    }

    if (!corpus.Scenarios.Any(scenario => scenario.RuleSet.Groups.Count > 1 &&
                                          scenario.Cases.Any(item =>
                                              item.ExpectedMatch && item.ExpectedGroup is not null &&
                                              item.ExpectedGroup == scenario.RuleSet.Groups[1].Name)))
    {
        failures.Add("No case proves that the second acceptable-result OR branch can win.");
    }

    if (!corpus.Scenarios.SelectMany(scenario => scenario.RuleSet.Groups)
            .Any(group => group.Mode == ResultGroupMode.AtLeast &&
                          group.RequiredCount == 2 && group.Conditions.Count == 4))
    {
        failures.Add("No scenario exercises AtLeast 2 of 4.");
    }

    if (!corpus.Scenarios.SelectMany(scenario => scenario.RuleSet.Groups)
            .Any(group => group.Mode == ResultGroupMode.All))
    {
        failures.Add("No scenario exercises All.");
    }

    var allCases = corpus.Scenarios.SelectMany(scenario => scenario.Cases).ToArray();
    RequireTag(allCases, "positive", failures);
    RequireTag(allCases, "near-neighbour-negative", failures);
    RequireTag(allCases, "one-short-negative", failures);
    RequireTag(allCases, "numeric-boundary-inclusive", failures);
    RequireTag(allCases, "numeric-boundary-below", failures);
    RequireTag(allCases, "one-modifier-one-count", failures);

    foreach (var item in allCases.Where(item =>
                 item.Tags.Contains("one-modifier-one-count", StringComparer.Ordinal)))
    {
        if (item.AssistedObservations.Count == 0 || item.PhysicalLines.Count == 0)
        {
            failures.Add(
                $"{item.Id}: one-modifier-one-count must exercise both assisted and primary " +
                "physical identity through OcrRecognitionResult.");
        }
    }

    if (!corpus.Limitations.Any(item =>
            item.Contains("Alt", StringComparison.OrdinalIgnoreCase) &&
            item.Contains("fractured", StringComparison.OrdinalIgnoreCase)))
    {
        failures.Add("Corpus must state the fractured/Alt observability limitation.");
    }
}

static OcrRecognitionResult CreateOcrResult(CorpusCase item) => new(
    item.Lines,
    TimeSpan.Zero,
    TimeSpan.Zero,
    AssistedObservations: item.AssistedObservations.Select(static observation =>
        new OcrAssistedObservation(
            observation.PhysicalBandId,
            observation.OriginalText,
            observation.CanonicalTarget,
            observation.SourceTop,
            observation.SourceBottom,
            observation.RelatedPhysicalBandIds)).ToArray(),
    PhysicalLines: item.PhysicalLines.Select(static line =>
        new OcrPhysicalLine(
            line.LineIndex,
            line.PhysicalBandId,
            line.SourceTop,
            line.SourceBottom)).ToArray());

static RuleEvaluationResult EvaluateRules(
    CompiledRuleSet rules,
    OcrRecognitionResult ocrResult) =>
    rules.Evaluate(
        ocrResult.Lines,
        ToAssistedObservations(ocrResult),
        ocrResult.BatchPhysicalLines.Select(static line =>
            new PhysicalLineIdentity(line.LineIndex, line.PhysicalBandId)).ToArray());

static AssistedModifierObservation[] ToAssistedObservations(
    OcrRecognitionResult ocrResult) =>
    ocrResult.BatchAssistedObservations.Select(static observation =>
        new AssistedModifierObservation(
            observation.PhysicalBandId,
            observation.OriginalText,
            observation.CanonicalTarget,
            observation.RelatedPhysicalBandIds)).ToArray();

static void RequireTag(
    IReadOnlyList<CorpusCase> cases,
    string tag,
    ICollection<string> failures)
{
    if (!cases.Any(item => item.Tags.Contains(tag, StringComparer.Ordinal)))
    {
        failures.Add($"Required coverage tag '{tag}' is missing.");
    }
}

static string Describe(RuleEvaluationResult result) => string.Join(
    " | ",
    result.Groups.Select(group =>
        $"{group.Name}={group.MatchedCount}/{group.RequiredCount} [" +
        string.Join(", ", group.Conditions.Select(condition =>
            $"{condition.Name}:{condition.IsMatched}")) + "]"));

internal sealed record MirrorCorpus
{
    public int Version { get; init; }

    public string GeneratedAt { get; init; } = string.Empty;

    public IReadOnlyList<string> Limitations { get; init; } = [];

    public IReadOnlyList<CorpusSource> Sources { get; init; } = [];

    public IReadOnlyList<CorpusScenario> Scenarios { get; init; } = [];
}

internal sealed record CorpusSource
{
    public string Id { get; init; } = string.Empty;

    public string Kind { get; init; } = string.Empty;

    public string Game { get; init; } = string.Empty;

    public string EquipmentSlot { get; init; } = string.Empty;

    public string Url { get; init; } = string.Empty;

    public string AccessedAt { get; init; } = string.Empty;

    public string Evidence { get; init; } = string.Empty;
}

internal sealed record CorpusScenario
{
    public string Id { get; init; } = string.Empty;

    public string Game { get; init; } = string.Empty;

    public string Locale { get; init; } = string.Empty;

    public string EquipmentSlot { get; init; } = string.Empty;

    public string Title { get; init; } = string.Empty;

    public string Basis { get; init; } = string.Empty;

    public IReadOnlyList<string> SourceIds { get; init; } = [];

    public IReadOnlyList<string> UnobservableFacts { get; init; } = [];

    public RuleSetDefinition RuleSet { get; init; } = new();

    public IReadOnlyList<CorpusCase> Cases { get; init; } = [];
}

internal sealed record CorpusCase
{
    public string Id { get; init; } = string.Empty;

    public bool ExpectedMatch { get; init; }

    public string? ExpectedGroup { get; init; }

    public IReadOnlyList<string> Tags { get; init; } = [];

    public IReadOnlyList<string> Lines { get; init; } = [];

    public IReadOnlyList<CorpusAssistedObservation> AssistedObservations { get; init; } = [];

    public IReadOnlyList<CorpusPhysicalLine> PhysicalLines { get; init; } = [];
}

internal sealed record CorpusAssistedObservation
{
    public string PhysicalBandId { get; init; } = string.Empty;

    public string OriginalText { get; init; } = string.Empty;

    public string CanonicalTarget { get; init; } = string.Empty;

    public int SourceTop { get; init; }

    public int SourceBottom { get; init; }

    public IReadOnlyList<string>? RelatedPhysicalBandIds { get; init; }
}

internal sealed record CorpusPhysicalLine
{
    public int LineIndex { get; init; }

    public string PhysicalBandId { get; init; } = string.Empty;

    public int SourceTop { get; init; }

    public int SourceBottom { get; init; }
}
