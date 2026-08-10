using System.IO;
using System.Runtime.InteropServices.WindowsRuntime;
using PoeAlarm.App.Capture;
using Windows.Graphics.Imaging;
using Windows.Storage;

namespace PoeAlarm.App.Replay;

/// <summary>
/// Loads a PNG or JPEG screenshot into the same tightly packed BGRA8 frame shape
/// used by live screen capture. Decoding and cropping happen in memory.
/// </summary>
public sealed class ScreenshotFrameLoader
{
    private const int BytesPerPixel = 4;

    private static readonly HashSet<string> SupportedExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".png",
        ".jpg",
        ".jpeg",
    };

    public async Task<CapturedFrame> LoadAsync(
        string imagePath,
        ScreenRegion? cropRegion = null,
        CancellationToken cancellationToken = default)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(imagePath);
        cancellationToken.ThrowIfCancellationRequested();

        var fullPath = Path.GetFullPath(imagePath);
        var extension = Path.GetExtension(fullPath);
        if (!SupportedExtensions.Contains(extension))
        {
            throw new NotSupportedException(
                $"Screenshot replay supports PNG and JPEG files only; received '{extension}'.");
        }

        if (!File.Exists(fullPath))
        {
            throw new FileNotFoundException($"Screenshot not found: {fullPath}", fullPath);
        }

        var file = await StorageFile
            .GetFileFromPathAsync(fullPath)
            .AsTask(cancellationToken)
            .ConfigureAwait(false);

        using var stream = await file
            .OpenAsync(FileAccessMode.Read)
            .AsTask(cancellationToken)
            .ConfigureAwait(false);

        var decoder = await BitmapDecoder
            .CreateAsync(stream)
            .AsTask(cancellationToken)
            .ConfigureAwait(false);

        var sourceWidth = ToIntDimension(decoder.PixelWidth, "width");
        var sourceHeight = ToIntDimension(decoder.PixelHeight, "height");
        var region = cropRegion ?? new ScreenRegion(0, 0, sourceWidth, sourceHeight);
        ValidateRegion(region, sourceWidth, sourceHeight);

        var transform = new BitmapTransform
        {
            ScaledWidth = decoder.PixelWidth,
            ScaledHeight = decoder.PixelHeight,
            InterpolationMode = BitmapInterpolationMode.NearestNeighbor,
            Bounds = new BitmapBounds
            {
                X = checked((uint)region.X),
                Y = checked((uint)region.Y),
                Width = checked((uint)region.Width),
                Height = checked((uint)region.Height),
            },
        };

        var pixelProvider = await decoder
            .GetPixelDataAsync(
                BitmapPixelFormat.Bgra8,
                BitmapAlphaMode.Premultiplied,
                transform,
                ExifOrientationMode.IgnoreExifOrientation,
                ColorManagementMode.DoNotColorManage)
            .AsTask(cancellationToken)
            .ConfigureAwait(false);

        cancellationToken.ThrowIfCancellationRequested();
        var pixels = pixelProvider.DetachPixelData();
        var stride = checked(region.Width * BytesPerPixel);
        var expectedByteCount = checked(stride * region.Height);
        if (pixels.Length != expectedByteCount)
        {
            throw new InvalidDataException(
                $"Decoded screenshot returned {pixels.Length} BGRA bytes; expected {expectedByteCount}.");
        }

        return new CapturedFrame(
            region.Width,
            region.Height,
            stride,
            pixels,
            DateTimeOffset.UtcNow);
    }

    private static int ToIntDimension(uint value, string dimensionName)
    {
        if (value is 0 or > int.MaxValue)
        {
            throw new InvalidDataException($"Screenshot {dimensionName} is invalid: {value}.");
        }

        return (int)value;
    }

    private static void ValidateRegion(ScreenRegion region, int imageWidth, int imageHeight)
    {
        if (!region.IsValid || region.X < 0 || region.Y < 0)
        {
            throw new ArgumentOutOfRangeException(
                nameof(region),
                region,
                "The crop region must have non-negative coordinates and positive dimensions.");
        }

        // Subtraction avoids overflow from X + Width when validating untrusted input.
        if (region.X >= imageWidth ||
            region.Y >= imageHeight ||
            region.Width > imageWidth - region.X ||
            region.Height > imageHeight - region.Y)
        {
            throw new ArgumentOutOfRangeException(
                nameof(region),
                region,
                $"The crop region lies outside the {imageWidth} × {imageHeight} screenshot.");
        }
    }
}
