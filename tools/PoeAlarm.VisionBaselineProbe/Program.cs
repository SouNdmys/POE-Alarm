using System.Diagnostics;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.App.Replay;

var options = Options.Parse(args);
var frame = options.ImagePath is null
    ? SyntheticTooltip(options.Width, options.Height)
    : await new ScreenshotFrameLoader().LoadAsync(options.ImagePath, options.Crop);
var preprocessor = new PoeTextPreprocessor();
var fingerprint = preprocessor.ComputeBlueTextFingerprint(frame);

for (var index = 0; index < options.Warmup; index++)
{
    _ = preprocessor.ComputeBlueTextFingerprint(frame);
    _ = preprocessor.PrepareBlueAffixBands(frame);
}

var fingerprintSamples = new long[options.Iterations];
var bandsSamples = new long[options.Iterations];
var observedBands = 0;
for (var index = 0; index < options.Iterations; index++)
{
    var started = Stopwatch.GetTimestamp();
    _ = preprocessor.ComputeBlueTextFingerprint(frame);
    fingerprintSamples[index] = Stopwatch.GetTimestamp() - started;

    started = Stopwatch.GetTimestamp();
    var bands = preprocessor.PrepareBlueAffixBands(frame);
    bandsSamples[index] = Stopwatch.GetTimestamp() - started;
    observedBands = bands.Count;
}

Array.Sort(fingerprintSamples);
Array.Sort(bandsSamples);
if (options.CsvPath is not null)
{
    await WriteCsvAsync(
        options.CsvPath,
        fingerprintSamples,
        bandsSamples,
        frame.Width,
        frame.Height,
        fingerprint);
}
Console.WriteLine(
    $".NET vision baseline: {options.Iterations} iterations, " +
    $"{frame.Width}x{frame.Height}, {observedBands} prepared bands, " +
    $"fingerprint={fingerprint:X16}");
PrintSamples("fingerprint", fingerprintSamples, frame.Width * frame.Height);
PrintSamples("mask+bands+BGRA crops", bandsSamples, frame.Width * frame.Height);
return;

static async Task WriteCsvAsync(
    string path,
    long[] fingerprintSamples,
    long[] bandsSamples,
    int width,
    int height,
    ulong fingerprint)
{
    var parent = Path.GetDirectoryName(path);
    if (!string.IsNullOrEmpty(parent))
    {
        Directory.CreateDirectory(parent);
    }
    await using var writer = new StreamWriter(path, false, new System.Text.UTF8Encoding(false));
    await writer.WriteLineAsync("implementation,iteration,metric,nanoseconds,width,height,fingerprint");
    for (var index = 0; index < fingerprintSamples.Length; index++)
    {
        var fingerprintNanoseconds = (long)Math.Round(
            fingerprintSamples[index] * 1_000_000_000.0 / Stopwatch.Frequency);
        var bandsNanoseconds = (long)Math.Round(
            bandsSamples[index] * 1_000_000_000.0 / Stopwatch.Frequency);
        await writer.WriteLineAsync(
            $"dotnet,{index},fingerprint,{fingerprintNanoseconds},{width},{height},{fingerprint:X16}");
        await writer.WriteLineAsync(
            $"dotnet,{index},mask+bands+BGRA crops,{bandsNanoseconds},{width},{height},{fingerprint:X16}");
    }
}

static void PrintSamples(string label, long[] samples, int pixels)
{
    static double Microseconds(long ticks) =>
        ticks * 1_000_000.0 / Stopwatch.Frequency;
    var meanTicks = samples.Average(value => (double)value);
    var At = (double ratio) => samples[(int)Math.Round((samples.Length - 1) * ratio)];
    Console.WriteLine(
        $"{label}: mean={Microseconds((long)meanTicks):F3} us " +
        $"p50={Microseconds((long)At(0.50)):F3} us " +
        $"p95={Microseconds((long)At(0.95)):F3} us " +
        $"p99={Microseconds((long)At(0.99)):F3} us " +
        $"({Microseconds((long)meanTicks) * 1000.0 / pixels:F3} ns/pixel mean)");
}

static CapturedFrame SyntheticTooltip(int width, int height)
{
    var stride = checked(width * 4);
    var pixels = new byte[checked(stride * height)];
    for (var y = 0; y < height; y++)
    {
        for (var x = 0; x < width; x++)
        {
            var pixel = (y * stride) + (x * 4);
            pixels[pixel] = (byte)((x * 5 + y * 3) % 96);
            pixels[pixel + 1] = (byte)((x * 2 + y * 7) % 80);
            pixels[pixel + 2] = (byte)((x * 3 + y * 2) % 80);
            pixels[pixel + 3] = 255;
        }
    }
    for (var line = 0; line < 8; line++)
    {
        var top = 70 + (line * 42);
        for (var y = top; y < top + 18 && y < height; y++)
        {
            for (var x = 80; x < Math.Min(620, width); x += 5)
            {
                var pixel = (y * stride) + (x * 4);
                pixels[pixel] = 210;
                pixels[pixel + 1] = 125;
                pixels[pixel + 2] = 110;
            }
        }
    }
    return new CapturedFrame(width, height, stride, pixels, DateTimeOffset.UtcNow);
}

internal sealed record Options(
    int Width,
    int Height,
    int Iterations,
    int Warmup,
    string? ImagePath,
    ScreenRegion? Crop,
    string? CsvPath)
{
    public static Options Parse(string[] args)
    {
        var width = 720;
        var height = 520;
        var iterations = 500;
        var warmup = 20;
        string? imagePath = null;
        ScreenRegion? crop = null;
        string? csvPath = null;
        for (var index = 0; index < args.Length; index++)
        {
            if (args[index].Equals("--image", StringComparison.OrdinalIgnoreCase))
            {
                imagePath = Path.GetFullPath(Take(args, ref index));
                continue;
            }
            if (args[index].Equals("--csv", StringComparison.OrdinalIgnoreCase))
            {
                csvPath = Path.GetFullPath(Take(args, ref index));
                continue;
            }
            if (args[index].Equals("--crop", StringComparison.OrdinalIgnoreCase))
            {
                var x = ParseAny(args, ref index, "crop X");
                var y = ParseAny(args, ref index, "crop Y");
                var cropWidth = ParsePositive(args, ref index, "crop width");
                var cropHeight = ParsePositive(args, ref index, "crop height");
                crop = new ScreenRegion(x, y, cropWidth, cropHeight);
                continue;
            }
            var argument = args[index];
            var value = ParsePositive(args, ref index, argument);
            switch (argument)
            {
                case "--width": width = value; break;
                case "--height": height = value; break;
                case "--iterations": iterations = value; break;
                case "--warmup": warmup = value; break;
                default: throw new ArgumentException($"Unknown argument: {args[index]}");
            }
        }
        if (width < 640 || height < 400)
        {
            throw new ArgumentOutOfRangeException(nameof(args), "Width must be at least 640 and height at least 400.");
        }
        return new Options(width, height, iterations, warmup, imagePath, crop, csvPath);
    }

    private static string Take(string[] args, ref int index)
    {
        index++;
        return index < args.Length
            ? args[index]
            : throw new ArgumentException("Missing command-line value.");
    }

    private static int ParseAny(string[] args, ref int index, string name) =>
        int.TryParse(Take(args, ref index), out var value)
            ? value
            : throw new ArgumentException($"{name} requires an integer");

    private static int ParsePositive(string[] args, ref int index, string name)
    {
        var value = ParseAny(args, ref index, name);
        return value > 0
            ? value
            : throw new ArgumentException($"{name} requires a positive integer");
    }
}
