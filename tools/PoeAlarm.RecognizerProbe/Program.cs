using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Text;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.App.Replay;
using PoeAlarm.Core.Matching;
using PoeAlarm.RecognizerProbe;

const string DefaultScreenshot =
    @"%TEMP%\poe-alarm-probe.png";
const string DefaultModel =
    @"src\PoeAlarm.App\Assets\Ocr\PP-OCRv5_mobile_rec.onnx";
const string DefaultDictionary =
    @"src\PoeAlarm.App\Assets\Ocr\ppocrv5_dict.txt";

try
{
    var options = ProbeOptions.Parse(args, DefaultScreenshot, DefaultModel, DefaultDictionary);
    if (options.ShowHelp)
    {
        ProbeOptions.PrintHelp();
        return 0;
    }

    if (!string.IsNullOrWhiteSpace(options.ManifestPath))
    {
        return await ManifestBenchmark.RunAsync(options);
    }

    return await RunAsync(options);
}
catch (ArgumentException exception)
{
    Console.Error.WriteLine($"Argument error: {exception.Message}");
    Console.Error.WriteLine(exception.StackTrace);
    Console.Error.WriteLine("Run with --help for usage.");
    return 2;
}
catch (Exception exception)
{
    Console.Error.WriteLine($"Recognizer probe failed: {exception.GetType().Name}: {exception.Message}");
    Console.Error.WriteLine(exception.StackTrace);
    return 1;
}

static async Task<int> RunAsync(ProbeOptions options)
{
    var imagePath = options.SyntheticText is null
        ? RequireFile(options.ImagePath, "screenshot")
        : null;
    var modelPath = RequireFile(options.ModelPath, "ONNX model");
    var dictionaryPath = RequireFile(options.DictionaryPath, "character dictionary");

    Console.WriteLine("POE Alarm recognizer-only benchmark");
    Console.WriteLine(options.SyntheticText is null
        ? $"Image:      {imagePath}"
        : $"Synthetic:  \"{options.SyntheticText}\"; {options.SyntheticFontFamily} {options.SyntheticFontSize:F0}px");
    Console.WriteLine($"ROI:        {options.Region?.ToString() ?? "full image"}");
    Console.WriteLine($"Model:      {modelPath}");
    Console.WriteLine($"Dictionary: {dictionaryPath}");
    Console.WriteLine($"Threads:    {options.Threads}");
    Console.WriteLine($"Polarity:   {options.Polarity}");
    Console.WriteLine($"Ink margin: {options.InkMargin}px");
    Console.WriteLine($"Runs:       warmup={options.WarmupCount}, measured={options.RepeatCount}");

    var loadWatch = Stopwatch.StartNew();
    var frame = options.SyntheticText is null
        ? await new ScreenshotFrameLoader().LoadAsync(imagePath!, options.Region)
        : SyntheticTextFrameRenderer.Render(
            options.SyntheticText,
            options.SyntheticFontFamily,
            options.SyntheticFontSize);
    loadWatch.Stop();

    var preprocessor = new PoeTextPreprocessor(new PoeTextPreprocessorSettings(
        options.MinimumBlue,
        options.MinimumBlueDominance,
        options.MaximumWarmChannelDifference,
        options.MaskIntensityMode));
    var preprocessSamples = new List<double>(options.RepeatCount);
    IReadOnlyList<PreparedFrame> bands = [];
    for (var iteration = 0; iteration < options.RepeatCount; iteration++)
    {
        var watch = Stopwatch.StartNew();
        bands = preprocessor.PrepareBlueAffixBands(frame, options.Scale);
        watch.Stop();
        preprocessSamples.Add(watch.Elapsed.TotalMilliseconds);
    }

    if (bands.Count == 0)
    {
        throw new InvalidDataException("The production blue-text preprocessor found no candidate rows.");
    }

    var selectedBands = SelectBands(bands, options.BandIndex);
    Console.WriteLine($"Source:     {frame.Width} x {frame.Height}; load={loadWatch.Elapsed.TotalMilliseconds:F2} ms");
    Console.WriteLine($"Blue rows:  {bands.Count}; selected={string.Join(',', selectedBands.Select(item => item.Index))}");
    Console.WriteLine($"Preprocess: {BenchmarkDistribution.Create(preprocessSamples)}");
    foreach (var item in selectedBands)
    {
        Console.WriteLine(
            $"  band[{item.Index}] {item.Frame.Width} x {item.Frame.Height}; " +
            $"source-y={item.Frame.SourceTop}..{item.Frame.SourceBottom}; fingerprint={item.Frame.ContentFingerprint:X16}");
    }

    if (!string.IsNullOrWhiteSpace(options.DumpDirectory))
    {
        var dumpDirectory = Path.GetFullPath(options.DumpDirectory);
        Directory.CreateDirectory(dumpDirectory);
        foreach (var item in selectedBands)
        {
            var outputPath = Path.Combine(dumpDirectory, $"band-{item.Index:D2}.png");
            WritePreparedFrame(outputPath, item.Frame);
        }

        Console.WriteLine($"Band images: {dumpDirectory}");
    }

    var process = Process.GetCurrentProcess();
    process.Refresh();
    var memoryBefore = process.WorkingSet64;
    var privateBefore = process.PrivateMemorySize64;
    var cpuBefore = process.TotalProcessorTime;
    var allocatedBefore = GC.GetTotalAllocatedBytes(true);
    var collectionBefore = Enumerable.Range(0, GC.MaxGeneration + 1).Select(GC.CollectionCount).ToArray();

    using var recognizer = new PaddleCtcSession(
        modelPath,
        dictionaryPath,
        options.Threads,
        options.Polarity,
        options.InkMargin,
        options.RightPadding,
        options.MaximumSegmentWidth);
    Console.WriteLine();
    Console.WriteLine(
        $"Session:    create={recognizer.SessionCreationElapsed.TotalMilliseconds:F2} ms; " +
        $"input={recognizer.InputName}[{string.Join(',', recognizer.InputDimensions)}]; " +
        $"output={recognizer.OutputName}[{string.Join(',', recognizer.OutputDimensions)}]; " +
        $"dict={recognizer.DictionarySize}");

    var coldWatch = Stopwatch.StartNew();
    var coldResult = recognizer.Recognize(selectedBands[0].Frame);
    coldWatch.Stop();
    Console.WriteLine(
        $"Cold row:   total={coldWatch.Elapsed.TotalMilliseconds:F2} ms; " +
        $"tensor={coldResult.TensorPreparationElapsed.TotalMilliseconds:F2}, " +
        $"infer={coldResult.InferenceElapsed.TotalMilliseconds:F2}, " +
        $"decode={coldResult.DecodeElapsed.TotalMilliseconds:F2}; " +
        $"shape=1x3x48x{coldResult.TensorWidth}; text=\"{coldResult.Text}\"");

    for (var warmup = 0; warmup < options.WarmupCount; warmup++)
    {
        foreach (var item in selectedBands)
        {
            _ = recognizer.Recognize(item.Frame);
        }
    }

    var frameSamples = new List<double>(options.RepeatCount);
    var tensorSamples = new List<double>(options.RepeatCount * selectedBands.Count);
    var inferenceSamples = new List<double>(options.RepeatCount * selectedBands.Count);
    var decodeSamples = new List<double>(options.RepeatCount * selectedBands.Count);
    var rowTotalSamples = new List<double>(options.RepeatCount * selectedBands.Count);
    var lastResults = new CtcRecognition[selectedBands.Count];
    var measurementWatch = Stopwatch.StartNew();
    for (var iteration = 0; iteration < options.RepeatCount; iteration++)
    {
        var frameWatch = Stopwatch.StartNew();
        for (var band = 0; band < selectedBands.Count; band++)
        {
            var result = recognizer.Recognize(selectedBands[band].Frame);
            lastResults[band] = result;
            tensorSamples.Add(result.TensorPreparationElapsed.TotalMilliseconds);
            inferenceSamples.Add(result.InferenceElapsed.TotalMilliseconds);
            decodeSamples.Add(result.DecodeElapsed.TotalMilliseconds);
            rowTotalSamples.Add(result.TotalElapsed.TotalMilliseconds);
        }

        frameWatch.Stop();
        frameSamples.Add(frameWatch.Elapsed.TotalMilliseconds);
    }

    measurementWatch.Stop();
    process.Refresh();
    var cpuAfter = process.TotalProcessorTime;
    var allocatedAfter = GC.GetTotalAllocatedBytes(false);

    Console.WriteLine();
    Console.WriteLine("Warm path:");
    Console.WriteLine($"  tensor prep  {BenchmarkDistribution.Create(tensorSamples)}");
    Console.WriteLine($"  inference    {BenchmarkDistribution.Create(inferenceSamples)}");
    Console.WriteLine($"  CTC decode   {BenchmarkDistribution.Create(decodeSamples)}");
    Console.WriteLine($"  row total    {BenchmarkDistribution.Create(rowTotalSamples)}");
    Console.WriteLine($"  selected set {BenchmarkDistribution.Create(frameSamples)}");

    Console.WriteLine();
    Console.WriteLine("Predictions:");
    for (var index = 0; index < selectedBands.Count; index++)
    {
        var item = selectedBands[index];
        var result = lastResults[index];
        Console.WriteLine(
            $"  band[{item.Index}] confidence={result.MeanConfidence:P1}; " +
            $"shape=1x3x48x{result.TensorWidth} -> \"{result.Text}\"");
    }

    var expectedText = options.ExpectedText ?? options.SyntheticText;
    if (!string.IsNullOrWhiteSpace(expectedText))
    {
        var best = lastResults
            .Select((result, index) => new
            {
                selectedBands[index].Index,
                result.Text,
                Distance = CharacterErrorRate(expectedText, result.Text),
            })
            .OrderBy(item => item.Distance)
            .First();
        Console.WriteLine();
        Console.WriteLine($"Expected:    \"{expectedText}\"");
        Console.WriteLine($"Best band:   {best.Index}; CER={best.Distance:P2}; normalized=\"{NormalizeText(best.Text)}\"");
    }

    if (!string.IsNullOrWhiteSpace(options.TargetTemplate))
    {
        var matcher = new FullLineAffixMatcher(options.TargetTemplate);
        var predictions = lastResults.Select(result => result.Text).ToArray();
        var matched = matcher.TryFindMatch(predictions, out var match);
        Console.WriteLine();
        Console.WriteLine($"Template:    \"{options.TargetTemplate}\"");
        Console.WriteLine(matched
            ? $"Strict match: YES; bands={match!.StartLineIndex}..{match.StartLineIndex + match.PhysicalLineCount - 1}; " +
              $"canonical=\"{match.CanonicalText}\""
            : "Strict match: NO");

        if (!matched)
        {
            for (var index = 0; index < selectedBands.Count; index++)
            {
                var diagnostic = recognizer.Recognize(selectedBands[index].Frame, options.TargetTemplate).TargetSupport;
                if (diagnostic is null)
                {
                    continue;
                }

                Console.WriteLine(
                    $"  CTC band[{selectedBands[index].Index}]: compatible={diagnostic.ShapeCompatible}; " +
                    $"strong={diagnostic.StronglySupported}; {diagnostic.Reason}");
                foreach (var mismatch in diagnostic.Mismatches)
                {
                    Console.WriteLine(
                        $"    '{mismatch.Actual}' -> '{mismatch.Expected}': rank={mismatch.Rank}, " +
                        $"p={mismatch.Probability:P2}, top={mismatch.TopProbability:P2}, " +
                        $"ratio={mismatch.ProbabilityRatio:P1}, t={mismatch.EvidenceTimeStep}");
                }
            }
        }
    }

    process.Refresh();
    var wallSeconds = measurementWatch.Elapsed.TotalSeconds;
    var cpuSeconds = (cpuAfter - cpuBefore).TotalSeconds;
    var logicalProcessors = Environment.ProcessorCount;
    Console.WriteLine();
    Console.WriteLine("Resources (session + measured run):");
    Console.WriteLine(
        $"  working set delta={FormatBytes(process.WorkingSet64 - memoryBefore)}; " +
        $"private delta={FormatBytes(process.PrivateMemorySize64 - privateBefore)}");
    Console.WriteLine(
        $"  managed allocations={FormatBytes(allocatedAfter - allocatedBefore)}; " +
        $"GC={string.Join('/', collectionBefore.Select((before, generation) => GC.CollectionCount(generation) - before))}");
    Console.WriteLine(
        $"  CPU={cpuSeconds:F3}s / wall={wallSeconds:F3}s; " +
        $"core-equivalent={(wallSeconds <= 0 ? 0 : cpuSeconds / wallSeconds):P1}; " +
        $"machine-normalized={(wallSeconds <= 0 ? 0 : cpuSeconds / wallSeconds / logicalProcessors):P1}");

    return 0;
}

static string RequireFile(string path, string description)
{
    var fullPath = Path.GetFullPath(path);
    if (!File.Exists(fullPath))
    {
        throw new FileNotFoundException($"The {description} was not found: {fullPath}", fullPath);
    }

    return fullPath;
}

static IReadOnlyList<SelectedBand> SelectBands(IReadOnlyList<PreparedFrame> bands, int? bandIndex)
{
    if (!bandIndex.HasValue)
    {
        return bands.Select((frame, index) => new SelectedBand(index, frame)).ToArray();
    }

    if (bandIndex.Value < 0 || bandIndex.Value >= bands.Count)
    {
        throw new ArgumentOutOfRangeException(
            nameof(bandIndex),
            $"Band {bandIndex.Value} is outside the available range 0..{bands.Count - 1}.");
    }

    return [new SelectedBand(bandIndex.Value, bands[bandIndex.Value])];
}

static double CharacterErrorRate(string expected, string actual)
{
    var left = NormalizeText(expected).EnumerateRunes().ToArray();
    var right = NormalizeText(actual).EnumerateRunes().ToArray();
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

static string NormalizeText(string text) =>
    string.Concat(text
        .Normalize(NormalizationForm.FormKC)
        .ToUpperInvariant()
        .Where(character => !char.IsWhiteSpace(character)));

static string FormatBytes(long bytes)
{
    var sign = bytes < 0 ? "-" : string.Empty;
    var absolute = Math.Abs((double)bytes);
    if (absolute >= 1024 * 1024)
    {
        return $"{sign}{absolute / (1024 * 1024):F1} MiB";
    }

    return $"{sign}{absolute / 1024:F1} KiB";
}

static void WritePreparedFrame(string outputPath, PreparedFrame frame)
{
    var bitmap = BitmapSource.Create(
        frame.Width,
        frame.Height,
        96,
        96,
        PixelFormats.Bgra32,
        null,
        frame.BgraPixels,
        frame.Stride);
    var encoder = new PngBitmapEncoder();
    encoder.Frames.Add(BitmapFrame.Create(bitmap));
    using var stream = File.Create(outputPath);
    encoder.Save(stream);
}

internal readonly record struct SelectedBand(int Index, PreparedFrame Frame);

internal sealed record ProbeOptions(
    string ImagePath,
    ScreenRegion? Region,
    string ModelPath,
    string DictionaryPath,
    int Threads,
    int WarmupCount,
    int RepeatCount,
    int Scale,
    int? BandIndex,
    TextPolarity Polarity,
    int InkMargin,
    string? ExpectedText,
    string? TargetTemplate,
    string? SyntheticText,
    string SyntheticFontFamily,
    double SyntheticFontSize,
    string? DumpDirectory,
    string? ManifestPath,
    bool SkipCrossNegatives,
    int MinimumBlue,
    int MinimumBlueDominance,
    int MaximumWarmChannelDifference,
    BlueMaskIntensityMode MaskIntensityMode,
    int RightPadding,
    int MaximumSegmentWidth,
    bool ShowHelp)
{
    public static ProbeOptions Parse(
        string[] arguments,
        string defaultScreenshot,
        string defaultModel,
        string defaultDictionary)
    {
        var image = defaultScreenshot;
        ScreenRegion? region = null;
        var model = defaultModel;
        var dictionary = defaultDictionary;
        var threads = 2;
        var warmup = 10;
        var repeat = 100;
        var scale = 1;
        int? bandIndex = null;
        var polarity = TextPolarity.BrightOnDark;
        var inkMargin = 2;
        string? expected = null;
        string? target = null;
        string? synthetic = null;
        var syntheticFontFamily = "Microsoft JhengHei";
        var syntheticFontSize = 32d;
        string? dumpDirectory = null;
        string? manifestPath = null;
        var skipCrossNegatives = false;
        var minimumBlue = 100;
        var minimumBlueDominance = 14;
        var maximumWarmChannelDifference = 72;
        var maskIntensityMode = BlueMaskIntensityMode.Dominance;
        var rightPadding = 0;
        var maximumSegmentWidth = 0;
        var showHelp = false;

        for (var index = 0; index < arguments.Length; index++)
        {
            var argument = arguments[index];
            switch (argument.ToLowerInvariant())
            {
                case "-h":
                case "--help":
                    showHelp = true;
                    break;
                case "--image":
                    image = NextValue(arguments, ref index, argument);
                    break;
                case "--roi":
                    region = ParseRegion(NextValue(arguments, ref index, argument));
                    break;
                case "--model":
                    model = NextValue(arguments, ref index, argument);
                    break;
                case "--dict":
                    dictionary = NextValue(arguments, ref index, argument);
                    break;
                case "--threads":
                    threads = ParseInteger(arguments, ref index, argument, 1, 16);
                    break;
                case "--warmup":
                    warmup = ParseInteger(arguments, ref index, argument, 0, 1000);
                    break;
                case "--repeat":
                    repeat = ParseInteger(arguments, ref index, argument, 1, 10000);
                    break;
                case "--scale":
                    scale = ParseInteger(arguments, ref index, argument, 1, 4);
                    break;
                case "--band":
                    var band = NextValue(arguments, ref index, argument);
                    if (!band.Equals("all", StringComparison.OrdinalIgnoreCase))
                    {
                        if (!int.TryParse(band, NumberStyles.None, CultureInfo.InvariantCulture, out var parsedBand) || parsedBand < 0)
                        {
                            throw new ArgumentException("--band must be 'all' or a non-negative integer.");
                        }

                        bandIndex = parsedBand;
                    }

                    break;
                case "--polarity":
                    polarity = ParsePolarity(NextValue(arguments, ref index, argument));
                    break;
                case "--ink-margin":
                    inkMargin = ParseInteger(arguments, ref index, argument, 0, 8);
                    break;
                case "--expected":
                    expected = NextValue(arguments, ref index, argument);
                    break;
                case "--target":
                    target = NextValue(arguments, ref index, argument);
                    break;
                case "--synthetic":
                    synthetic = NextValue(arguments, ref index, argument);
                    break;
                case "--font-size":
                    var fontSize = NextValue(arguments, ref index, argument);
                    if (!double.TryParse(fontSize, NumberStyles.Float, CultureInfo.InvariantCulture, out syntheticFontSize) ||
                        !double.IsFinite(syntheticFontSize) || syntheticFontSize is < 10 or > 96)
                    {
                        throw new ArgumentException("--font-size must be a number from 10 through 96.");
                    }

                    break;
                case "--font":
                    syntheticFontFamily = NextValue(arguments, ref index, argument);
                    break;
                case "--dump-bands":
                    dumpDirectory = NextValue(arguments, ref index, argument);
                    break;
                case "--manifest":
                    manifestPath = NextValue(arguments, ref index, argument);
                    break;
                case "--skip-cross-negatives":
                    skipCrossNegatives = true;
                    break;
                case "--minimum-blue":
                    minimumBlue = ParseInteger(arguments, ref index, argument, 0, 255);
                    break;
                case "--minimum-blue-dominance":
                    minimumBlueDominance = ParseInteger(arguments, ref index, argument, 0, 255);
                    break;
                case "--maximum-warm-difference":
                    maximumWarmChannelDifference = ParseInteger(arguments, ref index, argument, 0, 255);
                    break;
                case "--mask-intensity":
                    maskIntensityMode = ParseMaskIntensity(NextValue(arguments, ref index, argument));
                    break;
                case "--right-padding":
                    rightPadding = ParseInteger(arguments, ref index, argument, 0, 128);
                    break;
                case "--max-segment-width":
                    maximumSegmentWidth = ParseInteger(arguments, ref index, argument, 0, 1600);
                    if (maximumSegmentWidth is > 0 and < 320)
                    {
                        throw new ArgumentException("--max-segment-width must be 0 or at least 320.");
                    }
                    break;
                default:
                    throw new ArgumentException($"Unknown option: {argument}");
            }
        }

        return new ProbeOptions(
            image,
            region,
            model,
            dictionary,
            threads,
            warmup,
            repeat,
            scale,
            bandIndex,
            polarity,
            inkMargin,
            expected,
            target,
            synthetic,
            syntheticFontFamily,
            syntheticFontSize,
            dumpDirectory,
            manifestPath,
            skipCrossNegatives,
            minimumBlue,
            minimumBlueDominance,
            maximumWarmChannelDifference,
            maskIntensityMode,
            rightPadding,
            maximumSegmentWidth,
            showHelp);
    }

    public static void PrintHelp()
    {
        Console.WriteLine("POE Alarm PaddleOCR recognizer-only benchmark");
        Console.WriteLine();
        Console.WriteLine("Usage:");
        Console.WriteLine("  PoeAlarm.RecognizerProbe [options]");
        Console.WriteLine();
        Console.WriteLine("Options:");
        Console.WriteLine("  --image path              PNG/JPEG screenshot");
        Console.WriteLine("  --roi x,y,width,height    Optional source crop");
        Console.WriteLine("  --model path              Converted PaddleOCR ONNX model");
        Console.WriteLine("  --dict path               PaddleOCR character dictionary");
        Console.WriteLine("  --threads 1..16           ONNX CPU intra-op threads (default: 2)");
        Console.WriteLine("  --warmup 0..1000          Warm-up passes (default: 10)");
        Console.WriteLine("  --repeat 1..10000         Measured passes (default: 100)");
        Console.WriteLine("  --scale 1..4              Production blue-mask scale (default: 1)");
        Console.WriteLine("  --band all|index          Benchmark all rows or one row (default: all)");
        Console.WriteLine("  --polarity bright|dark    Bright text/dark background or inverse");
        Console.WriteLine("  --ink-margin 0..8         Quiet pixels around detected glyphs (default: 2)");
        Console.WriteLine("  --expected text           Optional ground truth for CER reporting");
        Console.WriteLine("  --target template         PoEDB-style template for production strict matching");
        Console.WriteLine("  --synthetic text          Render a repeatable Traditional Chinese test line");
        Console.WriteLine("  --font family             Synthetic font family (default: Microsoft JhengHei)");
        Console.WriteLine("  --font-size 10..96        Synthetic text size (default: 32)");
        Console.WriteLine("  --dump-bands directory   Write the exact production mask rows as PNG files");
        Console.WriteLine("  --manifest path          Run a real-screenshot JSON regression suite");
        Console.WriteLine("  --skip-cross-negatives   Skip all other-row false-alert checks in a sweep");
        Console.WriteLine("  --minimum-blue 0..255    Blue-mask channel floor (default: 100)");
        Console.WriteLine("  --minimum-blue-dominance 0..255  Blue-over-warm floor (default: 14)");
        Console.WriteLine("  --maximum-warm-difference 0..255 Red/green difference ceiling (default: 72)");
        Console.WriteLine("  --mask-intensity mode     dominance|boosted|blue|binary (default: dominance)");
        Console.WriteLine("  --right-padding 0..128    Extra blank model pixels after text (default: 0)");
        Console.WriteLine("  --max-segment-width N     Split long lines at quiet columns; 0 disables (default: 0)");
        Console.WriteLine("  -h, --help                Show help");
    }

    private static string NextValue(string[] arguments, ref int index, string option)
    {
        if (++index >= arguments.Length)
        {
            throw new ArgumentException($"{option} requires a value.");
        }

        return arguments[index];
    }

    private static int ParseInteger(
        string[] arguments,
        ref int index,
        string option,
        int minimum,
        int maximum)
    {
        var value = NextValue(arguments, ref index, option);
        if (!int.TryParse(value, NumberStyles.None, CultureInfo.InvariantCulture, out var parsed) ||
            parsed < minimum || parsed > maximum)
        {
            throw new ArgumentException($"{option} must be an integer from {minimum} through {maximum}.");
        }

        return parsed;
    }

    private static ScreenRegion ParseRegion(string value)
    {
        var parts = value.Split(',', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);
        if (parts.Length != 4 ||
            parts.Any(part => !int.TryParse(part, NumberStyles.Integer, CultureInfo.InvariantCulture, out _)))
        {
            throw new ArgumentException("--roi must use x,y,width,height integer values.");
        }

        var region = new ScreenRegion(
            int.Parse(parts[0], CultureInfo.InvariantCulture),
            int.Parse(parts[1], CultureInfo.InvariantCulture),
            int.Parse(parts[2], CultureInfo.InvariantCulture),
            int.Parse(parts[3], CultureInfo.InvariantCulture));
        if (!region.IsValid || region.X < 0 || region.Y < 0)
        {
            throw new ArgumentException("--roi coordinates must be non-negative and dimensions positive.");
        }

        return region;
    }

    private static TextPolarity ParsePolarity(string value) => value.ToLowerInvariant() switch
    {
        "bright" or "bright-on-dark" => TextPolarity.BrightOnDark,
        "dark" or "dark-on-light" => TextPolarity.DarkOnLight,
        _ => throw new ArgumentException("--polarity must be 'bright' or 'dark'."),
    };

    private static BlueMaskIntensityMode ParseMaskIntensity(string value) => value.ToLowerInvariant() switch
    {
        "dominance" => BlueMaskIntensityMode.Dominance,
        "boosted" or "boosted-dominance" => BlueMaskIntensityMode.BoostedDominance,
        "blue" or "blue-channel" => BlueMaskIntensityMode.BlueChannel,
        "binary" => BlueMaskIntensityMode.Binary,
        _ => throw new ArgumentException(
            "--mask-intensity must be 'dominance', 'boosted', 'blue', or 'binary'."),
    };
}
