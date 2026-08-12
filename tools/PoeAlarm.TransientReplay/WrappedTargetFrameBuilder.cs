using System.IO;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;

namespace PoeAlarm.TransientReplay;

/// <summary>
/// Derives a real-pixel wrapped target from one manifest row. Glyph pixels before a quiet
/// inter-character column remain on the first row; the remainder moves to an immediately
/// adjacent second row. Rows below it shift down intact. No text is rendered or synthesized.
/// </summary>
internal static class WrappedTargetFrameBuilder
{
    private static readonly PoeTextPreprocessorSettings Settings = new(
        MinimumBlue: 100,
        MinimumBlueDominance: 14,
        MaximumWarmChannelDifference: 72,
        IntensityMode: BlueMaskIntensityMode.Dominance);

    public static WrappedTargetFrame Create(CapturedFrame source, int targetBandIndex)
    {
        var originalBands = new PoeTextPreprocessor(Settings)
            .PrepareBlueAffixBands(source, scale: 1)
            .ToArray();
        if (targetBandIndex < 0 || targetBandIndex >= originalBands.Length)
        {
            throw new InvalidDataException(
                $"Wrapped target band {targetBandIndex} is outside 0..{originalBands.Length - 1}.");
        }

        var target = originalBands[targetBandIndex];
        var inkColumns = CollectInkColumns(source, target);
        var inkIndexes = inkColumns
            .Select((hasInk, x) => new { hasInk, x })
            .Where(item => item.hasInk)
            .Select(item => item.x)
            .ToArray();
        if (inkIndexes.Length < 2)
        {
            throw new InvalidDataException("Wrapped target has too little blue ink to split.");
        }

        var minimumX = inkIndexes[0];
        var maximumX = inkIndexes[^1];
        var minimumFilledWidth = (int)Math.Ceiling(source.Width * 0.70) - 20;
        var earliestSplit = Math.Max(
            minimumX + minimumFilledWidth,
            minimumX + (int)((maximumX - minimumX) * 0.60));
        var preferredSplit = minimumX + (int)((maximumX - minimumX) * 0.72);
        var latestSplit = minimumX + (int)((maximumX - minimumX) * 0.84);
        var splitX = Enumerable.Range(
                Math.Max(minimumX + 1, earliestSplit),
                Math.Max(0, Math.Min(maximumX - 1, latestSplit) -
                            Math.Max(minimumX + 1, earliestSplit) + 1))
            .Where(x => !inkColumns[x])
            .OrderBy(x => Math.Abs(x - preferredSplit))
            .FirstOrDefault(-1);
        if (splitX < 0)
        {
            throw new InvalidDataException(
                "Could not find a quiet inter-character column that leaves a full-width first wrapped row.");
        }

        var rowHeight = target.SourceBottom - target.SourceTop + 1;
        const int blankGap = 4;
        var insertionHeight = rowHeight + blankGap;
        var newHeight = checked(source.Height + insertionHeight);
        var newStride = source.Stride;
        var pixels = new byte[checked(newStride * newHeight)];
        for (var pixel = 3; pixel < pixels.Length; pixel += 4)
        {
            pixels[pixel] = byte.MaxValue;
        }

        var rowsThroughTarget = target.SourceBottom + 1;
        Buffer.BlockCopy(source.BgraPixels, 0, pixels, 0, rowsThroughTarget * source.Stride);
        var remainingRows = source.Height - rowsThroughTarget;
        Buffer.BlockCopy(
            source.BgraPixels,
            rowsThroughTarget * source.Stride,
            pixels,
            (rowsThroughTarget + insertionHeight) * newStride,
            remainingRows * source.Stride);

        for (var y = target.SourceTop; y <= target.SourceBottom; y++)
        {
            for (var x = splitX + 1; x < source.Width; x++)
            {
                var originalPixel = (y * source.Stride) + (x * 4);
                if (!IsBlueAffixPixel(source.BgraPixels, originalPixel))
                {
                    continue;
                }

                Neutralize(pixels, originalPixel);
                var wrappedY = y + insertionHeight;
                var wrappedPixel = (wrappedY * newStride) + (x * 4);
                pixels[wrappedPixel] = source.BgraPixels[originalPixel];
                pixels[wrappedPixel + 1] = source.BgraPixels[originalPixel + 1];
                pixels[wrappedPixel + 2] = source.BgraPixels[originalPixel + 2];
                pixels[wrappedPixel + 3] = source.BgraPixels[originalPixel + 3];
            }
        }

        var wrapped = new CapturedFrame(
            source.Width,
            newHeight,
            newStride,
            pixels,
            DateTimeOffset.UtcNow);
        var wrappedBands = new PoeTextPreprocessor(Settings)
            .PrepareBlueAffixBands(wrapped, scale: 1)
            .ToArray();
        var firstIndex = FindBand(wrappedBands, target.SourceTop, "first");
        var secondIndex = FindBand(
            wrappedBands,
            target.SourceTop + insertionHeight,
            "second");
        if (secondIndex != firstIndex + 1)
        {
            throw new InvalidDataException(
                $"Wrapped target rows are not adjacent: {firstIndex} then {secondIndex}.");
        }

        return new WrappedTargetFrame(wrapped, [firstIndex, secondIndex], splitX);
    }

    private static bool[] CollectInkColumns(CapturedFrame frame, PreparedFrame band)
    {
        var result = new bool[frame.Width];
        for (var y = band.SourceTop; y <= band.SourceBottom; y++)
        {
            for (var x = 0; x < frame.Width; x++)
            {
                result[x] |= IsBlueAffixPixel(frame.BgraPixels, (y * frame.Stride) + (x * 4));
            }
        }

        return result;
    }

    private static int FindBand(
        IReadOnlyList<PreparedFrame> bands,
        int expectedTop,
        string label)
    {
        var match = bands
            .Select((band, index) => new { band, index })
            .Where(item => !item.band.IsFallback)
            .OrderBy(item => Math.Abs(item.band.SourceTop - expectedTop))
            .FirstOrDefault();
        if (match is null || Math.Abs(match.band.SourceTop - expectedTop) > 2)
        {
            throw new InvalidDataException($"Could not locate the {label} wrapped target row.");
        }

        return match.index;
    }

    private static bool IsBlueAffixPixel(byte[] pixels, int pixel)
    {
        var blue = pixels[pixel];
        var green = pixels[pixel + 1];
        var red = pixels[pixel + 2];
        return blue >= Settings.MinimumBlue &&
               blue - Math.Max(red, green) >= Settings.MinimumBlueDominance &&
               Math.Abs(red - green) <= Settings.MaximumWarmChannelDifference;
    }

    private static void Neutralize(byte[] pixels, int pixel)
    {
        pixels[pixel] = 0;
        pixels[pixel + 1] = 0;
        pixels[pixel + 2] = 0;
    }
}

internal sealed record WrappedTargetFrame(
    CapturedFrame Frame,
    IReadOnlyList<int> TargetBandIndices,
    int SplitX);
