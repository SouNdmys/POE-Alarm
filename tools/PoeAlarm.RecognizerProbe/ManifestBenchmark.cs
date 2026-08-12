using System.Diagnostics;
using System.IO;
using System.Text;
using System.Text.Json;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.App.Replay;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.RecognizerProbe;

internal static class ManifestBenchmark
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true,
        ReadCommentHandling = JsonCommentHandling.Skip,
    };

    public static async Task<int> RunAsync(ProbeOptions options)
    {
        var manifestPath = RequireFile(options.ManifestPath!, "OCR regression manifest");
        var modelPath = RequireFile(options.ModelPath, "ONNX model");
        var dictionaryPath = RequireFile(options.DictionaryPath, "character dictionary");
        var manifest = JsonSerializer.Deserialize<TraditionalOcrManifest>(
                           await File.ReadAllTextAsync(manifestPath, Encoding.UTF8),
                           JsonOptions)
                       ?? throw new InvalidDataException("The OCR regression manifest is empty.");
        if (manifest.Screenshots.Count == 0)
        {
            throw new InvalidDataException("The OCR regression manifest contains no screenshots.");
        }

        Console.WriteLine("POE Alarm Traditional Chinese screenshot regression");
        Console.WriteLine($"Manifest:   {manifestPath}");
        Console.WriteLine($"Model:      {modelPath}");
        Console.WriteLine($"Dictionary: {dictionaryPath}");
        Console.WriteLine($"Threads:    {options.Threads}");
        Console.WriteLine($"Ink margin: {options.InkMargin}px");

        using var recognizer = new PaddleCtcSession(
            modelPath,
            dictionaryPath,
            options.Threads,
            options.Polarity,
            options.InkMargin,
            options.RightPadding,
            options.MaximumSegmentWidth);
        Console.WriteLine(
            $"Session:    create={recognizer.SessionCreationElapsed.TotalMilliseconds:F2} ms; " +
            $"input=[{string.Join(',', recognizer.InputDimensions)}]; " +
            $"output=[{string.Join(',', recognizer.OutputDimensions)}]; dict={recognizer.DictionarySize}");

        var root = Path.GetDirectoryName(manifestPath)!;
        var loader = new ScreenshotFrameLoader();
        var preprocessor = new PoeTextPreprocessor(new PoeTextPreprocessorSettings(
            options.MinimumBlue,
            options.MinimumBlueDominance,
            options.MaximumWarmChannelDifference,
            options.MaskIntensityMode));
        var allCases = new List<MeasuredCase>();
        var falseAlerts = new List<FalseAlert>();
        var productionFailures = new List<string>();
        var rowLatency = new List<double>();
        var productionChangedWallLatency = new List<double>();
        var productionChangedOcrLatency = new List<double>();
        var productionSliceWallLatency = new List<double>();
        var productionDecisionSlices = new List<double>();
        var productionCachedWallLatency = new List<double>();
        var suiteWatch = Stopwatch.StartNew();
        using var productionRecognizer = new PaddleOcrRecognizer(
            modelPath,
            dictionaryPath,
            options.Threads,
            new PoeTextPreprocessor(new PoeTextPreprocessorSettings(
                options.MinimumBlue,
                options.MinimumBlueDominance,
                options.MaximumWarmChannelDifference,
                options.MaskIntensityMode)));

        foreach (var screenshot in manifest.Screenshots)
        {
            ValidateScreenshot(screenshot);
            var imagePath = RequireFile(Path.Combine(root, screenshot.Image), "manifest screenshot");
            var region = new ScreenRegion(
                screenshot.Roi[0],
                screenshot.Roi[1],
                screenshot.Roi[2],
                screenshot.Roi[3]);
            var frame = await loader.LoadAsync(imagePath, region);
            var bands = preprocessor.PrepareBlueAffixBands(frame, manifest.Preprocessing.Scale);
            if (bands.Count != screenshot.ExpectedBandCount)
            {
                throw new InvalidDataException(
                    $"{screenshot.Image}: expected {screenshot.ExpectedBandCount} bands, found {bands.Count}.");
            }

            // One warm-up keeps cold session/JIT cost out of the real row-latency distribution.
            if (screenshot.Cases.Count > 0)
            {
                _ = recognizer.Recognize(bands[screenshot.Cases[0].Band]);
            }

            foreach (var item in screenshot.Cases)
            {
                PaddleTargetRecognition recovered;
                try
                {
                    var rowWatch = Stopwatch.StartNew();
                    recovered = PaddleTargetRecovery.Recognize(
                        recognizer,
                        bands[item.Band],
                        new FullLineAffixMatcher(item.Template));
                    rowWatch.Stop();
                    rowLatency.Add(rowWatch.Elapsed.TotalMilliseconds);
                }
                catch (Exception exception)
                {
                    throw new InvalidDataException(
                        $"{screenshot.Image}: positive case {item.Id} failed on band {item.Band}.",
                        exception);
                }
                var result = recovered.Primary;
                var strict = new FullLineAffixMatcher(item.Template).IsMatch(result.Text);
                var assisted = strict || recovered.AssistedText is not null;
                allCases.Add(new MeasuredCase(
                    item.Id,
                    item.Text,
                    item.Template,
                    result.Text,
                    CharacterErrorRate(item.Text, result.Text),
                    strict,
                    assisted,
                    recovered.AssistedText is not null
                        ? result.TargetSupport?.Reason ?? "segmented strict target recovery"
                        : result.TargetSupport?.Reason));
            }

            // Every target must reject every other detected blue row. This catches the dangerous
            // failure mode where target-aware recovery becomes permissive enough to alert on a
            // semantically different affix with a similar prefix.
            foreach (var item in options.SkipCrossNegatives
                         ? Array.Empty<TraditionalOcrCase>()
                         : screenshot.Cases)
            {
                var matcher = new FullLineAffixMatcher(item.Template);
                for (var band = 0; band < bands.Count; band++)
                {
                    if (band == item.Band)
                    {
                        continue;
                    }

                    PaddleTargetRecognition recovered;
                    try
                    {
                        recovered = PaddleTargetRecovery.Recognize(
                            recognizer,
                            bands[band],
                            matcher);
                    }
                    catch (Exception exception)
                    {
                        throw new InvalidDataException(
                            $"{screenshot.Image}: cross-negative target {item.Id} failed on band {band}.",
                            exception);
                    }
                    var result = recovered.Primary;
                    var strict = matcher.IsMatch(result.Text);
                    var assisted = recovered.AssistedText is not null;
                    if (strict || assisted)
                    {
                        falseAlerts.Add(new FalseAlert(
                            screenshot.Image,
                            item.Id,
                            band,
                            result.Text,
                            strict ? "strict" : "target-assisted"));
                    }
                }
            }

            // Exercise the actual recognizer used by live monitoring, including band assembly,
            // explicit target-assisted results, target-keyed caches, and the async worker path.
            foreach (var item in screenshot.Cases)
            {
                var matcher = new FullLineAffixMatcher(item.Template);
                var productionWatch = Stopwatch.StartNew();
                var totalOcrElapsed = TimeSpan.Zero;
                var sliceCount = 0;
                var firstSliceWasCached = false;
                OcrRecognitionResult? production = null;
                LogicalAffixMatch? productionMatch = null;
                for (; sliceCount < 128; sliceCount++)
                {
                    var sliceWatch = Stopwatch.StartNew();
                    production = await productionRecognizer.RecognizeAsync(frame, matcher);
                    sliceWatch.Stop();
                    productionSliceWallLatency.Add(sliceWatch.Elapsed.TotalMilliseconds);
                    totalOcrElapsed += production.TotalElapsed;
                    if (sliceCount == 0)
                    {
                        firstSliceWasCached = production.WasCached;
                    }

                    matcher.TryFindMatch(production.Lines, out productionMatch);
                    productionMatch ??= production.TargetAssistedMatch is { } assisted &&
                                        assisted.CanonicalText == matcher.Template.Text
                        ? assisted
                        : null;
                    if (productionMatch is not null || !production.RequiresRescan)
                    {
                        sliceCount++;
                        break;
                    }
                }

                productionWatch.Stop();
                productionChangedWallLatency.Add(productionWatch.Elapsed.TotalMilliseconds);
                productionChangedOcrLatency.Add(totalOcrElapsed.TotalMilliseconds);
                productionDecisionSlices.Add(sliceCount);
                if (productionMatch is null)
                {
                    productionFailures.Add($"{screenshot.Image}/{item.Id}: target was not detected");
                }

                if (firstSliceWasCached)
                {
                    productionFailures.Add($"{screenshot.Image}/{item.Id}: first target scan reused a stale cache");
                }

                productionWatch.Restart();
                var cached = await productionRecognizer.RecognizeAsync(frame, matcher);
                productionWatch.Stop();
                productionCachedWallLatency.Add(productionWatch.Elapsed.TotalMilliseconds);
                if (!cached.WasCached)
                {
                    productionFailures.Add($"{screenshot.Image}/{item.Id}: identical target scan was not cached");
                }
            }
        }

        suiteWatch.Stop();
        foreach (var item in allCases)
        {
            var marker = item.StrictMatch ? "PASS" : item.AssistedMatch ? "HELP" : "MISS";
            Console.WriteLine(
                $"{marker,-4} {item.Id,-42} CER={item.Cer,6:P1}  \"{item.Prediction}\"");
            if (!item.AssistedMatch && !string.IsNullOrWhiteSpace(item.AssistanceReason))
            {
                Console.WriteLine($"     target evidence: {item.AssistanceReason}");
            }
        }

        var strictCount = allCases.Count(item => item.StrictMatch);
        var assistedCount = allCases.Count(item => item.AssistedMatch);
        var exactCount = allCases.Count(item => item.Cer == 0);
        var meanCer = allCases.Average(item => item.Cer);
        Console.WriteLine();
        Console.WriteLine(
            $"Positive rows: exact={exactCount}/{allCases.Count}; canonical={strictCount}/{allCases.Count}; " +
            $"target-assisted={assistedCount}/{allCases.Count}; mean CER={meanCer:P2}");
        Console.WriteLine($"Row latency:  {BenchmarkDistribution.Create(rowLatency)}");
        Console.WriteLine($"False alerts: {falseAlerts.Count}");
        foreach (var alert in falseAlerts)
        {
            Console.WriteLine(
                $"  {alert.Screenshot} target={alert.TargetId} band={alert.Band} " +
                $"via={alert.Route}: \"{alert.Prediction}\"");
        }

        Console.WriteLine($"Production E2E failures: {productionFailures.Count}");
        foreach (var failure in productionFailures)
        {
            Console.WriteLine($"  {failure}");
        }
        Console.WriteLine(
            $"Production changed frame wall: {BenchmarkDistribution.Create(productionChangedWallLatency)}");
        Console.WriteLine(
            $"Production changed frame OCR:  {BenchmarkDistribution.Create(productionChangedOcrLatency)}");
        Console.WriteLine(
            $"Production slice wall:         {BenchmarkDistribution.Create(productionSliceWallLatency)}");
        Console.WriteLine(
            $"Production decision slices:    {BenchmarkDistribution.Create(productionDecisionSlices)}");
        Console.WriteLine(
            $"Production cached frame wall:  {BenchmarkDistribution.Create(productionCachedWallLatency)}");

        Console.WriteLine($"Suite wall:   {suiteWatch.Elapsed.TotalSeconds:F2}s");
        return assistedCount == allCases.Count &&
               falseAlerts.Count == 0 &&
               productionFailures.Count == 0
            ? 0
            : 3;
    }

    private static void ValidateScreenshot(TraditionalOcrScreenshot screenshot)
    {
        if (screenshot.Roi.Length != 4 || screenshot.Roi[2] <= 0 || screenshot.Roi[3] <= 0)
        {
            throw new InvalidDataException($"{screenshot.Image}: roi must contain x,y,width,height.");
        }

        if (screenshot.Cases.Any(item => item.Band < 0 || item.Band >= screenshot.ExpectedBandCount))
        {
            throw new InvalidDataException($"{screenshot.Image}: a case has an invalid band index.");
        }
    }

    private static string RequireFile(string path, string description)
    {
        var fullPath = Path.GetFullPath(path);
        if (!File.Exists(fullPath))
        {
            throw new FileNotFoundException($"The {description} was not found: {fullPath}", fullPath);
        }

        return fullPath;
    }

    private static double CharacterErrorRate(string expected, string actual)
    {
        var left = Normalize(expected).EnumerateRunes().ToArray();
        var right = Normalize(actual).EnumerateRunes().ToArray();
        if (left.Length == 0)
        {
            return right.Length == 0 ? 0 : 1;
        }

        var previous = Enumerable.Range(0, right.Length + 1).ToArray();
        var current = new int[right.Length + 1];
        for (var row = 1; row <= left.Length; row++)
        {
            current[0] = row;
            for (var column = 1; column <= right.Length; column++)
            {
                var substitution = left[row - 1] == right[column - 1] ? 0 : 1;
                current[column] = Math.Min(
                    Math.Min(current[column - 1] + 1, previous[column] + 1),
                    previous[column - 1] + substitution);
            }

            (previous, current) = (current, previous);
        }

        return previous[^1] / (double)left.Length;
    }

    private static string Normalize(string text) =>
        string.Concat(text
            .Normalize(NormalizationForm.FormKC)
            .ToUpperInvariant()
            .Where(character => !char.IsWhiteSpace(character)));

    private sealed record MeasuredCase(
        string Id,
        string Expected,
        string Template,
        string Prediction,
        double Cer,
        bool StrictMatch,
        bool AssistedMatch,
        string? AssistanceReason);

    private sealed record FalseAlert(
        string Screenshot,
        string TargetId,
        int Band,
        string Prediction,
        string Route);
}

internal sealed record TraditionalOcrManifest(
    int Version,
    TraditionalOcrPreprocessing Preprocessing,
    IReadOnlyList<TraditionalOcrScreenshot> Screenshots);

internal sealed record TraditionalOcrPreprocessing(int Scale);

internal sealed record TraditionalOcrScreenshot(
    string Image,
    int[] Roi,
    int ExpectedBandCount,
    IReadOnlyList<int> IgnoredBands,
    IReadOnlyList<TraditionalOcrCase> Cases);

internal sealed record TraditionalOcrCase(
    string Id,
    int Band,
    string Text,
    string Template,
    string Category);
