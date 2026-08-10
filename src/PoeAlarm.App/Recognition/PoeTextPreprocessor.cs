using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Recognition;

/// <summary>
/// Isolates POE's blue affix text from the translucent tooltip and animated game scene.
/// Numeric values and letters use the same mask; matching decides which parts are significant.
/// </summary>
public sealed class PoeTextPreprocessor
{
    private const int BytesPerPixel = 4;
    private readonly List<byte[]> _bandBuffers = [];
    private byte[] _combinedBuffer = [];
    private byte[] _maskBuffer = [];
    private int[] _rowInkBuffer = [];

    /// <summary>
    /// Finds individual physical text rows. Small row images keep Windows OCR latency low even
    /// when affixes move vertically because preceding text wraps differently.
    /// </summary>
    public IReadOnlyList<PreparedFrame> PrepareBlueAffixBands(CapturedFrame source, int scale = 1)
    {
        ArgumentNullException.ThrowIfNull(source);
        ValidateScale(scale);

        var mask = BuildBlueMask(source, out var fingerprint);
        var rowInk = GetRowInkBuffer(source.Height);
        Array.Clear(rowInk, 0, source.Height);
        for (var y = 0; y < source.Height; y++)
        {
            var row = y * source.Width;
            var count = 0;
            for (var x = 0; x < source.Width; x++)
            {
                if (mask[row + x] != 0)
                {
                    count++;
                }
            }

            rowInk[y] = count;
        }

        var bands = FindRowBands(rowInk);
        var prepared = new List<PreparedFrame>(bands.Count);
        var oversizedBandFound = false;
        foreach (var band in bands)
        {
            var bandHeight = band.End - band.Start + 1;
            if (bandHeight > 48)
            {
                // Blue UI art can vertically connect an otherwise valid text row. Never
                // silently discard it: retain a bounded crop and schedule a full-ROI fallback.
                oversizedBandFound = true;
                if (bandHeight > 96)
                {
                    continue;
                }
            }

            var minimumX = source.Width;
            var maximumX = -1;
            for (var y = band.Start; y <= band.End; y++)
            {
                var row = y * source.Width;
                for (var x = 0; x < source.Width; x++)
                {
                    if (mask[row + x] == 0)
                    {
                        continue;
                    }

                    minimumX = Math.Min(minimumX, x);
                    maximumX = Math.Max(maximumX, x);
                }
            }

            if (maximumX < minimumX || maximumX - minimumX < 24)
            {
                continue;
            }

            const int horizontalMargin = 10;
            // Windows OCR is noticeably more reliable with roughly one text-height of
            // quiet context above and below a POE line (about a 50 px crop at 1080p).
            const int verticalMargin = 14;
            minimumX = Math.Max(0, minimumX - horizontalMargin);
            maximumX = Math.Min(source.Width - 1, maximumX + horizontalMargin);
            var minimumY = Math.Max(0, band.Start - verticalMargin);
            var maximumY = Math.Min(source.Height - 1, band.End + verticalMargin);

            prepared.Add(CreatePreparedFrame(
                source,
                mask,
                minimumX,
                minimumY,
                maximumX,
                maximumY,
                scale,
                fingerprint ^ (ulong)band.Start,
                prepared.Count,
                band.Start,
                band.End));
        }

        if (oversizedBandFound)
        {
            prepared.Add(CreatePreparedFrame(
                source,
                mask,
                0,
                0,
                source.Width - 1,
                source.Height - 1,
                scale,
                fingerprint ^ 0x9E3779B97F4A7C15UL,
                bufferSlot: -1,
                logicalTop: 0,
                logicalBottom: source.Height - 1,
                isFallback: true));
        }

        return prepared;
    }

    /// <summary>Creates one combined mask for diagnostics or a conservative OCR fallback.</summary>
    public PreparedFrame PrepareBlueAffixText(CapturedFrame source, int scale = 1)
    {
        ArgumentNullException.ThrowIfNull(source);
        ValidateScale(scale);

        var mask = BuildBlueMask(source, out var fingerprint);
        var minimumX = source.Width;
        var minimumY = source.Height;
        var maximumX = -1;
        var maximumY = -1;

        for (var y = 0; y < source.Height; y++)
        {
            var row = y * source.Width;
            for (var x = 0; x < source.Width; x++)
            {
                if (mask[row + x] == 0)
                {
                    continue;
                }

                minimumX = Math.Min(minimumX, x);
                minimumY = Math.Min(minimumY, y);
                maximumX = Math.Max(maximumX, x);
                maximumY = Math.Max(maximumY, y);
            }
        }

        if (maximumX < minimumX || maximumY < minimumY)
        {
            return new PreparedFrame(1, 1, BytesPerPixel, [0, 0, 0, 255], BytesPerPixel, fingerprint);
        }

        const int margin = 8;
        return CreatePreparedFrame(
            source,
            mask,
            Math.Max(0, minimumX - margin),
            Math.Max(0, minimumY - margin),
            Math.Min(source.Width - 1, maximumX + margin),
            Math.Min(source.Height - 1, maximumY + margin),
            scale,
            fingerprint,
            bufferSlot: -1,
            logicalTop: minimumY,
            logicalBottom: maximumY);
    }

    private static List<RowBand> FindRowBands(IReadOnlyList<int> rowInk)
    {
        const int minimumInkPerRow = 3;
        const int toleratedBlankRows = 2;
        const int minimumBandHeight = 5;

        var bands = new List<RowBand>();
        var start = -1;
        var lastInk = -1;

        for (var y = 0; y < rowInk.Count; y++)
        {
            if (rowInk[y] >= minimumInkPerRow)
            {
                start = start < 0 ? y : start;
                lastInk = y;
                continue;
            }

            if (start < 0 || y - lastInk <= toleratedBlankRows)
            {
                continue;
            }

            AddBandIfTextLike(bands, start, lastInk, minimumBandHeight);
            start = -1;
            lastInk = -1;
        }

        if (start >= 0)
        {
            AddBandIfTextLike(bands, start, lastInk, minimumBandHeight);
        }

        return bands;
    }

    private static void AddBandIfTextLike(
        ICollection<RowBand> bands,
        int start,
        int end,
        int minimumHeight)
    {
        var height = end - start + 1;
        if (height >= minimumHeight)
        {
            bands.Add(new RowBand(start, end));
        }
    }

    private byte[] BuildBlueMask(CapturedFrame source, out ulong fingerprint)
    {
        var pixelCount = checked(source.Width * source.Height);
        if (_maskBuffer.Length < pixelCount)
        {
            _maskBuffer = new byte[pixelCount];
        }

        var mask = _maskBuffer;
        Array.Clear(mask, 0, pixelCount);
        fingerprint = 14695981039346656037UL;

        for (var y = 0; y < source.Height; y++)
        {
            var sourceRow = y * source.Stride;
            var maskRow = y * source.Width;

            for (var x = 0; x < source.Width; x++)
            {
                var pixel = sourceRow + (x * BytesPerPixel);
                var blue = source.BgraPixels[pixel];
                var green = source.BgraPixels[pixel + 1];
                var red = source.BgraPixels[pixel + 2];

                var dominance = blue - Math.Max(red, green);
                var balancedWarmChannels = Math.Abs(red - green) <= 72;
                if (blue < 105 || dominance < 18 || !balancedWarmChannels)
                {
                    continue;
                }

                var intensity = (byte)Math.Clamp(72 + (dominance * 3), 0, 255);
                mask[maskRow + x] = intensity;
                fingerprint ^= (ulong)((x * 397) ^ y ^ (intensity >> 5));
                fingerprint *= 1099511628211UL;
            }
        }

        return mask;
    }

    private PreparedFrame CreatePreparedFrame(
        CapturedFrame source,
        byte[] mask,
        int minimumX,
        int minimumY,
        int maximumX,
        int maximumY,
        int scale,
        ulong fingerprint,
        int bufferSlot,
        int logicalTop,
        int logicalBottom,
        bool isFallback = false)
    {
        var croppedWidth = maximumX - minimumX + 1;
        var croppedHeight = maximumY - minimumY + 1;
        var outputWidth = checked(croppedWidth * scale);
        var outputHeight = checked(croppedHeight * scale);
        var outputStride = checked(outputWidth * BytesPerPixel);
        var byteLength = checked(outputStride * outputHeight);
        var output = GetOutputBuffer(bufferSlot, byteLength);

        for (var outputY = 0; outputY < outputHeight; outputY++)
        {
            var sourceY = minimumY + (outputY / scale);
            var outputRow = outputY * outputStride;

            for (var outputX = 0; outputX < outputWidth; outputX++)
            {
                var sourceX = minimumX + (outputX / scale);
                var outputPixel = outputRow + (outputX * BytesPerPixel);
                var intensity = mask[(sourceY * source.Width) + sourceX];

                // The tooltip is translucent, so inventory stack counts and icons can sit
                // directly behind an affix (for example an unrelated white "2" between
                // WITH and DAGGER). Feed OCR only the detected blue glyphs instead of the
                // original scene. Keeping the mask intensity preserves anti-aliased edges.
                output[outputPixel] = intensity;
                output[outputPixel + 1] = intensity;
                output[outputPixel + 2] = intensity;
                output[outputPixel + 3] = 255;
            }
        }

        return new PreparedFrame(
            outputWidth,
            outputHeight,
            outputStride,
            output,
            byteLength,
            fingerprint,
            logicalTop,
            logicalBottom,
            isFallback);
    }

    private int[] GetRowInkBuffer(int requiredLength)
    {
        if (_rowInkBuffer.Length < requiredLength)
        {
            _rowInkBuffer = new int[requiredLength];
        }

        return _rowInkBuffer;
    }

    private byte[] GetOutputBuffer(int slot, int requiredLength)
    {
        if (slot < 0)
        {
            if (_combinedBuffer.Length < requiredLength)
            {
                _combinedBuffer = new byte[requiredLength];
            }

            return _combinedBuffer;
        }

        while (_bandBuffers.Count <= slot)
        {
            _bandBuffers.Add([]);
        }

        if (_bandBuffers[slot].Length < requiredLength)
        {
            _bandBuffers[slot] = new byte[requiredLength];
        }

        return _bandBuffers[slot];
    }

    private static void ValidateScale(int scale)
    {
        if (scale is < 1 or > 4)
        {
            throw new ArgumentOutOfRangeException(nameof(scale), "OCR scale must be between 1 and 4.");
        }
    }

    private readonly record struct RowBand(int Start, int End);
}
