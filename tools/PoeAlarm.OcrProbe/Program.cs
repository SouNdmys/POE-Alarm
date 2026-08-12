using System.Diagnostics;
using System.Globalization;
using Windows.Foundation;
using Windows.Globalization;
using Windows.Graphics.Imaging;
using Windows.Media.Ocr;
using Windows.Storage;

const string DefaultScreenshot =
    @"%TEMP%\poe-alarm-probe.png";

try
{
    var options = ProbeOptions.Parse(args, DefaultScreenshot);
    if (options.ShowHelp)
    {
        ProbeOptions.PrintHelp();
        return 0;
    }

    return await RunAsync(options);
}
catch (ArgumentException exception)
{
    Console.Error.WriteLine($"Argument error: {exception.Message}");
    Console.Error.WriteLine("Run with --help for usage.");
    return 2;
}
catch (FileNotFoundException exception)
{
    Console.Error.WriteLine(exception.Message);
    return 3;
}
catch (Exception exception)
{
    Console.Error.WriteLine($"OCR probe failed: {exception.GetType().Name}: {exception.Message}");
    Console.Error.WriteLine(exception.StackTrace);
    return 1;
}

static async Task<int> RunAsync(ProbeOptions options)
{
    var fullPath = Path.GetFullPath(options.ImagePath);
    if (!File.Exists(fullPath))
    {
        throw new FileNotFoundException($"Image not found: {fullPath}", fullPath);
    }

    Console.WriteLine($"Image: {fullPath}");

    var loadWatch = Stopwatch.StartNew();
    var file = await StorageFile.GetFileFromPathAsync(fullPath);
    using var stream = await file.OpenAsync(FileAccessMode.Read);
    var decoder = await BitmapDecoder.CreateAsync(stream);
    loadWatch.Stop();

    Console.WriteLine($"Source: {decoder.PixelWidth} x {decoder.PixelHeight} px");
    Console.WriteLine($"Container load: {loadWatch.Elapsed.TotalMilliseconds:F1} ms");

    var sourceBounds = options.Region ?? new PixelRegion(0, 0, decoder.PixelWidth, decoder.PixelHeight);
    sourceBounds.Validate(decoder.PixelWidth, decoder.PixelHeight);

    var outputWidth = checked((uint)Math.Round(sourceBounds.Width * options.Scale));
    var outputHeight = checked((uint)Math.Round(sourceBounds.Height * options.Scale));
    if (outputWidth == 0 || outputHeight == 0)
    {
        throw new ArgumentException("ROI and scale result in an empty image.");
    }

    var language = new Language(options.LanguageTag);
    var engine = OcrEngine.TryCreateFromLanguage(language);
    if (engine is null)
    {
        var installed = string.Join(", ", OcrEngine.AvailableRecognizerLanguages.Select(item => item.LanguageTag));
        throw new InvalidOperationException(
            $"No Windows OCR engine is available for '{options.LanguageTag}'. Installed OCR languages: {installed}");
    }

    if (outputWidth > OcrEngine.MaxImageDimension || outputHeight > OcrEngine.MaxImageDimension)
    {
        throw new ArgumentException(
            $"Processed image would be {outputWidth} x {outputHeight}; Windows OCR allows at most " +
            $"{OcrEngine.MaxImageDimension} px on either axis. Reduce the ROI or scale.");
    }

    Console.WriteLine(
        $"ROI: x={sourceBounds.X}, y={sourceBounds.Y}, w={sourceBounds.Width}, h={sourceBounds.Height}; " +
        $"scale={options.Scale.ToString("0.###", CultureInfo.InvariantCulture)}; " +
        $"output={outputWidth} x {outputHeight}");
    Console.WriteLine($"OCR language: {engine.RecognizerLanguage.LanguageTag}");
    Console.WriteLine($"Pixel mode: {options.PixelMode.ToString().ToLowerInvariant()}");

    var transform = CreateTransform(decoder.PixelWidth, decoder.PixelHeight, sourceBounds, options.Scale);
    var decodeWatch = Stopwatch.StartNew();
    using var bitmap = await DecodeAsync(decoder, transform, options.PixelMode);
    decodeWatch.Stop();

    Console.WriteLine(
        $"Decoded: {bitmap.PixelWidth} x {bitmap.PixelHeight} {bitmap.BitmapPixelFormat}; " +
        $"decode/crop/scale={decodeWatch.Elapsed.TotalMilliseconds:F1} ms");

    for (var run = 1; run <= options.RepeatCount; run++)
    {
        var ocrWatch = Stopwatch.StartNew();
        var result = await engine.RecognizeAsync(bitmap);
        ocrWatch.Stop();

        Console.WriteLine();
        Console.WriteLine($"Run {run}/{options.RepeatCount}: OCR={ocrWatch.Elapsed.TotalMilliseconds:F1} ms; " +
                          $"lines={result.Lines.Count}; angle={FormatAngle(result.TextAngle)}");

        if (result.Lines.Count == 0)
        {
            Console.WriteLine("  (no text recognized)");
            continue;
        }

        for (var index = 0; index < result.Lines.Count; index++)
        {
            var line = result.Lines[index];
            var bounds = GetLineBounds(line);
            Console.WriteLine(
                $"  [{index:00}] ({bounds.X:F0},{bounds.Y:F0},{bounds.Width:F0},{bounds.Height:F0}) {line.Text}");
        }
    }

    return 0;
}

static BitmapTransform CreateTransform(
    uint sourceWidth,
    uint sourceHeight,
    PixelRegion sourceBounds,
    double scale)
{
    var scaledWidth = checked((uint)Math.Round(sourceWidth * scale));
    var scaledHeight = checked((uint)Math.Round(sourceHeight * scale));
    var scaledX = checked((uint)Math.Round(sourceBounds.X * scale));
    var scaledY = checked((uint)Math.Round(sourceBounds.Y * scale));
    var scaledRegionWidth = checked((uint)Math.Round(sourceBounds.Width * scale));
    var scaledRegionHeight = checked((uint)Math.Round(sourceBounds.Height * scale));

    return new BitmapTransform
    {
        ScaledWidth = scaledWidth,
        ScaledHeight = scaledHeight,
        InterpolationMode = scale >= 1 ? BitmapInterpolationMode.Cubic : BitmapInterpolationMode.Fant,
        Bounds = new BitmapBounds
        {
            X = scaledX,
            Y = scaledY,
            Width = scaledRegionWidth,
            Height = scaledRegionHeight,
        },
    };
}

static async Task<SoftwareBitmap> DecodeAsync(
    BitmapDecoder decoder,
    BitmapTransform transform,
    PixelMode pixelMode)
{
    var decoded = await decoder.GetSoftwareBitmapAsync(
        BitmapPixelFormat.Bgra8,
        BitmapAlphaMode.Premultiplied,
        transform,
        ExifOrientationMode.IgnoreExifOrientation,
        ColorManagementMode.DoNotColorManage);

    if (pixelMode == PixelMode.Bgra)
    {
        return decoded;
    }

    try
    {
        return SoftwareBitmap.Convert(decoded, BitmapPixelFormat.Gray8, BitmapAlphaMode.Ignore);
    }
    finally
    {
        decoded.Dispose();
    }
}

static Rect GetLineBounds(OcrLine line)
{
    if (line.Words.Count == 0)
    {
        return new Rect();
    }

    var left = line.Words.Min(word => word.BoundingRect.Left);
    var top = line.Words.Min(word => word.BoundingRect.Top);
    var right = line.Words.Max(word => word.BoundingRect.Right);
    var bottom = line.Words.Max(word => word.BoundingRect.Bottom);
    return new Rect(left, top, right - left, bottom - top);
}

static string FormatAngle(double? angle) =>
    angle.HasValue
        ? angle.Value.ToString("F2", CultureInfo.InvariantCulture)
        : "n/a";

internal enum PixelMode
{
    Gray,
    Bgra,
}

internal readonly record struct PixelRegion(uint X, uint Y, uint Width, uint Height)
{
    public void Validate(uint imageWidth, uint imageHeight)
    {
        if (Width == 0 || Height == 0)
        {
            throw new ArgumentException("ROI width and height must both be greater than zero.");
        }

        if (X >= imageWidth || Y >= imageHeight || Width > imageWidth - X || Height > imageHeight - Y)
        {
            throw new ArgumentException(
                $"ROI ({X},{Y},{Width},{Height}) is outside the {imageWidth} x {imageHeight} source image.");
        }
    }

    public static PixelRegion Parse(string value)
    {
        var parts = value.Split(',', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);
        if (parts.Length != 4 || parts.Any(part => !uint.TryParse(part, NumberStyles.None, CultureInfo.InvariantCulture, out _)))
        {
            throw new ArgumentException("ROI must use x,y,width,height with non-negative integer values.");
        }

        return new PixelRegion(
            uint.Parse(parts[0], CultureInfo.InvariantCulture),
            uint.Parse(parts[1], CultureInfo.InvariantCulture),
            uint.Parse(parts[2], CultureInfo.InvariantCulture),
            uint.Parse(parts[3], CultureInfo.InvariantCulture));
    }
}

internal sealed record ProbeOptions(
    string ImagePath,
    PixelRegion? Region,
    double Scale,
    string LanguageTag,
    PixelMode PixelMode,
    int RepeatCount,
    bool ShowHelp)
{
    public static ProbeOptions Parse(string[] arguments, string defaultScreenshot)
    {
        string? imagePath = null;
        PixelRegion? region = null;
        var scale = 1d;
        var language = "en-US";
        var pixelMode = PixelMode.Gray;
        var repeat = 1;
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
                case "--roi":
                    region = PixelRegion.Parse(NextValue(arguments, ref index, "--roi"));
                    break;
                case "--scale":
                    var scaleText = NextValue(arguments, ref index, "--scale");
                    if (!double.TryParse(scaleText, NumberStyles.Float, CultureInfo.InvariantCulture, out scale) ||
                        !double.IsFinite(scale) || scale <= 0 || scale > 8)
                    {
                        throw new ArgumentException("Scale must be a number greater than 0 and no greater than 8.");
                    }

                    break;
                case "--language":
                    language = NextValue(arguments, ref index, "--language");
                    break;
                case "--pixel-mode":
                    pixelMode = ParsePixelMode(NextValue(arguments, ref index, "--pixel-mode"));
                    break;
                case "--repeat":
                    var repeatText = NextValue(arguments, ref index, "--repeat");
                    if (!int.TryParse(repeatText, NumberStyles.None, CultureInfo.InvariantCulture, out repeat) ||
                        repeat is < 1 or > 100)
                    {
                        throw new ArgumentException("Repeat count must be an integer from 1 through 100.");
                    }

                    break;
                default:
                    if (argument.StartsWith('-'))
                    {
                        throw new ArgumentException($"Unknown option: {argument}");
                    }

                    if (imagePath is not null)
                    {
                        throw new ArgumentException("Only one image path may be supplied.");
                    }

                    imagePath = argument;
                    break;
            }
        }

        imagePath ??= defaultScreenshot;
        return new ProbeOptions(imagePath, region, scale, language, pixelMode, repeat, showHelp);
    }

    public static void PrintHelp()
    {
        Console.WriteLine("POE Alarm Windows OCR probe");
        Console.WriteLine();
        Console.WriteLine("Usage:");
        Console.WriteLine("  PoeAlarm.OcrProbe [image-path] [options]");
        Console.WriteLine();
        Console.WriteLine("Options:");
        Console.WriteLine("  --roi x,y,w,h       Crop in source-image pixels (default: full image)");
        Console.WriteLine("  --scale number      Resize before OCR, 0 < scale <= 8 (default: 1)");
        Console.WriteLine("  --language tag      Windows OCR language tag (default: en-US)");
        Console.WriteLine("  --pixel-mode mode   gray or bgra (default: gray)");
        Console.WriteLine("  --repeat count      OCR the prepared bitmap repeatedly (default: 1)");
        Console.WriteLine("  -h, --help          Show this help");
        Console.WriteLine();
        Console.WriteLine("With no image path, the original user-provided POE screenshot is used if it still exists.");
    }

    private static string NextValue(string[] arguments, ref int index, string option)
    {
        if (++index >= arguments.Length)
        {
            throw new ArgumentException($"{option} requires a value.");
        }

        return arguments[index];
    }

    private static PixelMode ParsePixelMode(string value) => value.ToLowerInvariant() switch
    {
        "gray" or "gray8" => PixelMode.Gray,
        "bgra" or "bgra8" => PixelMode.Bgra,
        _ => throw new ArgumentException("Pixel mode must be 'gray' or 'bgra'."),
    };
}
