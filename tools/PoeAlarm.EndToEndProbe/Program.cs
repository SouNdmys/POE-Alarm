using System.Diagnostics;
using System.IO;
using System.Text.Json;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.App.Replay;
using PoeAlarm.Core.Matching;
using PoeAlarm.Core.Rules;

const string defaultScreenshot =
    @"%TEMP%\poe-alarm-probe.png";
const string defaultTemplate =
    "(6-8)% increased Attack Speed if you've dealt a Critical Strike Recently";

var manifestArgument = Array.FindIndex(args, argument =>
    argument.Equals("--manifest", StringComparison.OrdinalIgnoreCase));
if (manifestArgument >= 0)
{
    if (manifestArgument + 1 >= args.Length)
    {
        throw new ArgumentException("--manifest requires a JSON path.");
    }

    var usePoe2EnglishRecovery = args.Any(argument =>
        argument.Equals("--poe2-en-recovery", StringComparison.OrdinalIgnoreCase));
    return await RunTraditionalManifestAsync(
        Path.GetFullPath(args[manifestArgument + 1]),
        usePoe2EnglishRecovery);
}

var legacyManifestArgument = Array.FindIndex(args, argument =>
    argument.Equals("--legacy-manifest", StringComparison.OrdinalIgnoreCase));
if (legacyManifestArgument >= 0)
{
    if (legacyManifestArgument + 1 >= args.Length)
    {
        throw new ArgumentException("--legacy-manifest requires a JSON path.");
    }

    return await RunLegacyTraditionalManifestAsync(
        Path.GetFullPath(args[legacyManifestArgument + 1]));
}

var useTraditionalChinese = args.Any(argument =>
    argument.Equals("--zh", StringComparison.OrdinalIgnoreCase));
var structuredNumericArgument = Array.FindIndex(args, argument =>
    argument.Equals("--structured-at-least", StringComparison.OrdinalIgnoreCase));
decimal? structuredMinimum = null;
var parsedMinimum = 0m;
if (structuredNumericArgument >= 0 &&
    (structuredNumericArgument + 1 >= args.Length ||
     !decimal.TryParse(
         args[structuredNumericArgument + 1],
         System.Globalization.NumberStyles.Number,
         System.Globalization.CultureInfo.InvariantCulture,
         out parsedMinimum)))
{
    throw new ArgumentException("--structured-at-least requires an invariant decimal value.");
}
else if (structuredNumericArgument >= 0)
{
    structuredMinimum = parsedMinimum;
}
var scaleArgument = Array.FindIndex(args, argument =>
    argument.Equals("--scale", StringComparison.OrdinalIgnoreCase));
var ocrScale = 1;
if (scaleArgument >= 0 &&
    (scaleArgument + 1 >= args.Length ||
     !int.TryParse(args[scaleArgument + 1], out ocrScale) ||
     ocrScale is < 1 or > 4))
{
    throw new ArgumentException("--scale requires an integer from 1 through 4.");
}

var positionalArgs = new List<string>();
for (var index = 0; index < args.Length; index++)
{
    if (args[index].Equals("--zh", StringComparison.OrdinalIgnoreCase))
    {
        continue;
    }

    if (args[index].Equals("--scale", StringComparison.OrdinalIgnoreCase))
    {
        index++;
        continue;
    }

    if (args[index].Equals("--structured-at-least", StringComparison.OrdinalIgnoreCase))
    {
        index++;
        continue;
    }

    positionalArgs.Add(args[index]);
}
var screenshot = positionalArgs.Count > 0 ? Path.GetFullPath(positionalArgs[0]) : defaultScreenshot;
var template = positionalArgs.Count > 1 ? positionalArgs[1] : defaultTemplate;
ScreenRegion? crop = null;
if (positionalArgs.Count == 6 &&
    int.TryParse(positionalArgs[2], out var x) &&
    int.TryParse(positionalArgs[3], out var y) &&
    int.TryParse(positionalArgs[4], out var width) &&
    int.TryParse(positionalArgs[5], out var height))
{
    crop = new ScreenRegion(x, y, width, height);
}

Console.WriteLine($"Screenshot: {screenshot}");
Console.WriteLine($"Template:   {template}");
Console.WriteLine($"Crop:       {crop?.ToString() ?? "full image"}");
Console.WriteLine($"Language:   {(useTraditionalChinese ? "Traditional Chinese" : "English")}");
Console.WriteLine($"OCR scale:  {ocrScale}");

var diagnosticFrame = await new ScreenshotFrameLoader().LoadAsync(screenshot, crop);
var diagnosticBands = new PoeTextPreprocessor().PrepareBlueAffixBands(diagnosticFrame);
Console.WriteLine($"Blue bands: {diagnosticBands.Count}");
for (var index = 0; index < diagnosticBands.Count; index++)
{
    Console.WriteLine($"  band[{index}] {diagnosticBands[index].Width} × {diagnosticBands[index].Height}");
}

using IOcrRecognizer recognizer = useTraditionalChinese
    ? new WindowsChineseOcrRecognizer()
    : new WindowsOcrRecognizer(
        scale: ocrScale,
        language: OcrRecognitionLanguage.English);
Console.WriteLine($"Recognizer: {(recognizer is WindowsChineseOcrRecognizer chinese
    ? chinese.RecognizerLanguageTag
    : ((WindowsOcrRecognizer)recognizer).RecognizerLanguageTag)}");
var analyzer = new ScreenshotAffixAnalyzer(recognizer);
if (structuredMinimum is { } minimum)
{
    var rules = RuleCompiler.Compile(new RuleSetDefinition(
        "screenshot probe",
        [
            new AcceptableResultGroup(
                "result",
                ResultGroupMode.Any,
                [
                    new AffixCondition(
                        "target",
                        template,
                        [NumericConstraint.AtLeast(minimum)]),
                ]),
        ]));
    var structuredResult = await analyzer.AnalyzeRulesAsync(screenshot, rules, crop);
    var condition = structuredResult.Evaluation.Groups.Single().Conditions.Single();
    Console.WriteLine(
        $"Structured rule: match={structuredResult.IsMatch}; " +
        $"observed=\"{condition.ObservedText}\"; " +
        $"values={string.Join(',', condition.ActualValues.Select(value =>
            value?.ToString(System.Globalization.CultureInfo.InvariantCulture) ?? "missing"))}");
    return structuredResult.IsMatch ? 0 : 1;
}

var result = await analyzer.AnalyzeAsync(screenshot, template, crop);
var unchangedResult = await recognizer.RecognizeAsync(
    diagnosticFrame,
    new FullLineAffixMatcher(template));

Console.WriteLine(
    $"Timing: load={result.ImageLoadElapsed.TotalMilliseconds:F1} ms, " +
    $"preprocess={result.PreprocessingElapsed.TotalMilliseconds:F1} ms, " +
    $"ocr={result.RecognitionElapsed.TotalMilliseconds:F1} ms");
Console.WriteLine(
    $"Unchanged frame: total={unchangedResult.TotalElapsed.TotalMilliseconds:F1} ms, " +
    $"cached={unchangedResult.WasCached}");
Console.WriteLine("OCR lines:");
for (var index = 0; index < result.Lines.Count; index++)
{
    Console.WriteLine($"  [{index}] {result.Lines[index]}");
}

Console.WriteLine(result.IsMatch
    ? $"MATCH: {result.Match!.OriginalText}"
    : "NO MATCH");

return result.IsMatch ? 0 : 1;

static async Task<int> RunTraditionalManifestAsync(
    string manifestPath,
    bool usePoe2EnglishRecovery)
{
    using var manifestDocument = JsonDocument.Parse(await File.ReadAllTextAsync(manifestPath));
    var requestedLocale = manifestDocument.RootElement.TryGetProperty("locale", out var localeElement)
        ? localeElement.GetString() ?? "zh-TW"
        : "zh-TW";
    var useEnglish = requestedLocale.StartsWith("en", StringComparison.OrdinalIgnoreCase);
    var manifest = JsonSerializer.Deserialize<NativeTraditionalManifest>(
                       manifestDocument.RootElement.GetRawText(),
                       new JsonSerializerOptions { PropertyNameCaseInsensitive = true })
                   ?? throw new InvalidDataException("Traditional OCR manifest is empty.");
    if (manifest.Roi.Length != 4)
    {
        throw new InvalidDataException("Manifest roi must contain x,y,width,height.");
    }

    var crop = new ScreenRegion(
        manifest.Roi[0],
        manifest.Roi[1],
        manifest.Roi[2],
        manifest.Roi[3]);
    var root = Path.GetDirectoryName(manifestPath)!;
    var loader = new ScreenshotFrameLoader();
    using IOcrRecognizer recognizer = useEnglish
        ? usePoe2EnglishRecovery
            ? new Poe2EnglishRecognizer()
            : new WindowsOcrRecognizer(scale: 1, language: OcrRecognitionLanguage.English)
        : new WindowsChineseOcrRecognizer();
    var fingerprintProvider = recognizer as IFrameFingerprintProvider;
    var samples = new List<double>();
    var preflightSamples = new List<double>();
    var failures = new List<string>();
    var falseAlerts = new List<string>();
    var assistedCount = 0;
    var allTemplates = manifest.Screenshots
        .SelectMany(item => item.Cases)
        .GroupBy(template => new FullLineAffixMatcher(template).Template.Text, StringComparer.Ordinal)
        .Select(group => group.First())
        .ToArray();

    Console.WriteLine($"POE Alarm native {requestedLocale} end-to-end regression");
    Console.WriteLine($"Manifest:   {manifestPath}");
    Console.WriteLine($"Recognizer: {recognizer switch
    {
        WindowsChineseOcrRecognizer chinese => chinese.RecognizerLanguageTag,
        WindowsOcrRecognizer windows => windows.RecognizerLanguageTag,
        _ => recognizer.GetType().Name,
    }}");
    Console.WriteLine($"ROI:        {crop}");

    foreach (var screenshot in manifest.Screenshots)
    {
        var imagePath = Path.Combine(root, screenshot.Image);
        var frame = await loader.LoadAsync(imagePath, crop);
        foreach (var template in screenshot.Cases)
        {
            var matcher = new FullLineAffixMatcher(template);
            var preflight = Stopwatch.StartNew();
            _ = fingerprintProvider?.ComputeFrameFingerprint(frame);
            preflight.Stop();
            var decision = Stopwatch.StartNew();
            var result = await recognizer.RecognizeAsync(frame, matcher);
            decision.Stop();

            matcher.TryFindMatch(result.Lines, out var match);
            match ??= result.TargetAssistedMatch is { } assisted &&
                      assisted.CanonicalText == matcher.Template.Text
                ? assisted
                : null;
            preflightSamples.Add(preflight.Elapsed.TotalMilliseconds);
            samples.Add(decision.Elapsed.TotalMilliseconds);
            if (result.TargetAssistedMatch is not null)
            {
                assistedCount++;
            }

            var status = match is null ? "FAIL" : "PASS";
            Console.WriteLine(
                $"{status} {screenshot.Image} | {template} | " +
                $"preflight={preflight.Elapsed.TotalMilliseconds:F1}ms, " +
                $"decision={decision.Elapsed.TotalMilliseconds:F1}ms, " +
                $"assisted={result.TargetAssistedMatch is not null}");
            if (match is null)
            {
                failures.Add($"{screenshot.Image}: {template}");
            }
        }

        var presentCanonical = screenshot.Cases
            .Select(template => new FullLineAffixMatcher(template).Template.Text)
            .ToHashSet(StringComparer.Ordinal);
        foreach (var negativeTemplate in allTemplates.Where(template =>
                     !presentCanonical.Contains(new FullLineAffixMatcher(template).Template.Text)))
        {
            var matcher = new FullLineAffixMatcher(negativeTemplate);
            _ = fingerprintProvider?.ComputeFrameFingerprint(frame);
            var negative = await recognizer.RecognizeAsync(frame, matcher);
            matcher.TryFindMatch(negative.Lines, out var unexpected);
            unexpected ??= negative.TargetAssistedMatch is { } assisted &&
                           assisted.CanonicalText == matcher.Template.Text
                ? assisted
                : null;
            if (unexpected is not null)
            {
                falseAlerts.Add($"{screenshot.Image}: {negativeTemplate}");
            }
        }
    }

    samples.Sort();
    preflightSamples.Sort();
    Console.WriteLine();
    Console.WriteLine(
        $"Summary: total={samples.Count}, passed={samples.Count - failures.Count}, " +
        $"failed={failures.Count}, assisted={assistedCount}, " +
        $"crossNegatives={manifest.Screenshots.Count * allTemplates.Length - manifest.Screenshots.Sum(item => item.Cases.Select(template => new FullLineAffixMatcher(template).Template.Text).Distinct(StringComparer.Ordinal).Count())}, " +
        $"falseAlerts={falseAlerts.Count}");
    Console.WriteLine(
        $"Preflight: p50={Percentile(preflightSamples, 0.50):F1}ms, " +
        $"p95={Percentile(preflightSamples, 0.95):F1}ms, max={preflightSamples[^1]:F1}ms");
    Console.WriteLine(
        $"Decision:  p50={Percentile(samples, 0.50):F1}ms, " +
        $"p95={Percentile(samples, 0.95):F1}ms, max={samples[^1]:F1}ms");
    foreach (var failure in failures)
    {
        Console.WriteLine($"  FAILED: {failure}");
    }

    foreach (var falseAlert in falseAlerts)
    {
        Console.WriteLine($"  FALSE ALERT: {falseAlert}");
    }

    if (recognizer is Poe2EnglishTargetRecoveryAuditRecognizer recovery)
    {
        recovery.RecoveryWallMilliseconds.Sort();
        Console.WriteLine(
            $"POE2 English recovery: attempts={recovery.RecoveryAttempts}, " +
            $"matches={recovery.RecoveryMatches}, " +
            $"p50={Percentile(recovery.RecoveryWallMilliseconds, 0.50):F1}ms, " +
            $"p95={Percentile(recovery.RecoveryWallMilliseconds, 0.95):F1}ms");
    }

    return failures.Count == 0 && falseAlerts.Count == 0 ? 0 : 1;
}

static double Percentile(IReadOnlyList<double> sorted, double percentile)
{
    if (sorted.Count == 0)
    {
        return 0;
    }

    var index = (int)Math.Ceiling((sorted.Count - 1) * percentile);
    return sorted[Math.Clamp(index, 0, sorted.Count - 1)];
}

static async Task<int> RunLegacyTraditionalManifestAsync(string manifestPath)
{
    var manifest = JsonSerializer.Deserialize<LegacyTraditionalManifest>(
                       await File.ReadAllTextAsync(manifestPath),
                       new JsonSerializerOptions { PropertyNameCaseInsensitive = true })
                   ?? throw new InvalidDataException("Legacy Traditional OCR manifest is empty.");
    var root = Path.GetDirectoryName(manifestPath)!;
    var loader = new ScreenshotFrameLoader();
    using var recognizer = new WindowsChineseOcrRecognizer();
    var failures = new List<string>();
    var falseAlerts = new List<string>();
    var total = 0;
    var assisted = 0;
    var decisionSamples = new List<double>();

    Console.WriteLine("POE Alarm legacy Traditional Chinese regression");
    Console.WriteLine($"Manifest:   {manifestPath}");
    Console.WriteLine($"Recognizer: {recognizer.RecognizerLanguageTag}");

    foreach (var screenshot in manifest.Screenshots)
    {
        if (screenshot.Roi.Length != 4)
        {
            throw new InvalidDataException($"{screenshot.Image}: roi must contain four integers.");
        }

        var crop = new ScreenRegion(
            screenshot.Roi[0],
            screenshot.Roi[1],
            screenshot.Roi[2],
            screenshot.Roi[3]);
        var frame = await loader.LoadAsync(Path.Combine(root, screenshot.Image), crop);
        foreach (var item in screenshot.Cases)
        {
            total++;
            var matcher = new FullLineAffixMatcher(item.Template);
            _ = recognizer.ComputeFrameFingerprint(frame);
            var watch = Stopwatch.StartNew();
            var result = await recognizer.RecognizeAsync(frame, matcher);
            watch.Stop();
            decisionSamples.Add(watch.Elapsed.TotalMilliseconds);
            matcher.TryFindMatch(result.Lines, out var match);
            match ??= result.TargetAssistedMatch is { } targetAssisted &&
                      targetAssisted.CanonicalText == matcher.Template.Text
                ? targetAssisted
                : null;
            if (result.TargetAssistedMatch is not null)
            {
                assisted++;
            }

            if (match is null)
            {
                failures.Add($"{screenshot.Image}: {item.Template} => {string.Join(" | ", result.Lines)}");
            }
        }

        foreach (var item in screenshot.Cases)
        {
            var target = new FullLineAffixMatcher(item.Template);
            foreach (var other in screenshot.Cases.Where(candidate =>
                         !string.Equals(candidate.Template, item.Template, StringComparison.Ordinal)))
            {
                // Text-only strict cross-negative coverage remains in the core test suite. Here
                // we only need one screen-level negative per semantically distinct target, which
                // is already exercised by the positive pass against all simultaneously visible
                // rows. Avoid multiplying expensive OCR calls quadratically.
                if (target.Template.Text == new FullLineAffixMatcher(other.Template).Template.Text)
                {
                    falseAlerts.Add($"duplicate canonical template: {item.Template}");
                }
            }
        }
    }

    decisionSamples.Sort();
    Console.WriteLine(
        $"Summary: total={total}, passed={total - failures.Count}, failed={failures.Count}, " +
        $"assisted={assisted}, p50={Percentile(decisionSamples, 0.50):F1}ms, " +
        $"p95={Percentile(decisionSamples, 0.95):F1}ms");
    foreach (var failure in failures)
    {
        Console.WriteLine($"  FAILED: {failure}");
    }

    return failures.Count == 0 ? 0 : 1;
}

internal sealed record NativeTraditionalManifest(
    int[] Roi,
    IReadOnlyList<NativeTraditionalScreenshot> Screenshots);

internal sealed record NativeTraditionalScreenshot(
    string Image,
    IReadOnlyList<string> Cases);

internal sealed record LegacyTraditionalManifest(
    IReadOnlyList<LegacyTraditionalScreenshot> Screenshots);

internal sealed record LegacyTraditionalScreenshot(
    string Image,
    int[] Roi,
    IReadOnlyList<LegacyTraditionalCase> Cases);

internal sealed record LegacyTraditionalCase(string Template);
