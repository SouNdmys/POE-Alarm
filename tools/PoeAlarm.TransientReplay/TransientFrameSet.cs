using System.IO;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;

namespace PoeAlarm.TransientReplay;

/// <summary>
/// Produces cache-cold variants of a real tooltip ROI. The absent frame replaces the target
/// row with another real blue affix row from the same tooltip. Each capture then removes two
/// inconsequential edge pixels per row so consecutive rolls retain the same OCR workload but
/// cannot take the production recognizer's identical-band cache shortcut.
/// </summary>
internal sealed class TransientFrameSet
{
    private static readonly PoeTextPreprocessorSettings Settings = new(
        MinimumBlue: 100,
        MinimumBlueDominance: 14,
        MaximumWarmChannelDifference: 72,
        IntensityMode: BlueMaskIntensityMode.Dominance);

    private readonly FrameVariantSource _present;
    private readonly FrameVariantSource _absent;
    private readonly FingerprintMode _fingerprintMode;
    private CapturedFrame? _stablePresent;
    private CapturedFrame? _stableAbsent;
    private int _variantId;

    private TransientFrameSet(
        FrameVariantSource present,
        FrameVariantSource absent,
        FingerprintMode fingerprintMode)
    {
        _present = present;
        _absent = absent;
        _fingerprintMode = fingerprintMode;
        BeginSequence();
    }

    public int Width => _present.BaseFrame.Width;

    public int Height => _present.BaseFrame.Height;

    public static TransientFrameSet Create(
        CapturedFrame source,
        IReadOnlyList<int> targetBandIndices,
        FingerprintMode fingerprintMode)
    {
        var preprocessor = new PoeTextPreprocessor(Settings);
        var bands = preprocessor.PrepareBlueAffixBands(source, scale: 1).ToArray();
        if (targetBandIndices.Count == 0 ||
            targetBandIndices.Any(index => index < 0 || index >= bands.Length))
        {
            throw new InvalidDataException(
                $"A target band is outside detected range 0..{bands.Length - 1}.");
        }

        var absent = ReplaceTargetsWithDonor(source, bands, targetBandIndices);
        var absentBands = new PoeTextPreprocessor(Settings)
            .PrepareBlueAffixBands(absent, scale: 1)
            .ToArray();
        if (absentBands.Length == 0)
        {
            throw new InvalidDataException("Absent-frame construction removed every OCR band.");
        }

        return new TransientFrameSet(
            FrameVariantSource.Create(source, bands),
            FrameVariantSource.Create(absent, absentBands),
            fingerprintMode);
    }

    public CapturedFrame Next(bool targetPresent)
    {
        if (_fingerprintMode == FingerprintMode.StableState)
        {
            var stable = targetPresent ? _stablePresent : _stableAbsent;
            return (stable ?? throw new InvalidOperationException("Replay sequence is not initialized.")) with
            {
                CapturedAt = DateTimeOffset.UtcNow,
            };
        }

        var source = targetPresent ? _present : _absent;
        var id = Interlocked.Increment(ref _variantId);
        return source.CreateVariant(id);
    }

    public void BeginSequence()
    {
        if (_fingerprintMode != FingerprintMode.StableState)
        {
            return;
        }

        // Each trial gets a fresh pair of fingerprints, while every capture within its absent
        // or present state remains byte-identical. This validates row-slice continuation without
        // allowing completed band caches from an earlier phase trial to shortcut the next one.
        _stableAbsent = _absent.CreateVariant(Interlocked.Increment(ref _variantId));
        _stablePresent = _present.CreateVariant(Interlocked.Increment(ref _variantId));
    }

    private static CapturedFrame ReplaceTargetsWithDonor(
        CapturedFrame source,
        IReadOnlyList<PreparedFrame> bands,
        IReadOnlyList<int> targetBandIndices)
    {
        var targetIndexes = targetBandIndices.ToHashSet();
        var target = bands[targetBandIndices[0]];
        var donor = bands
            .Select((band, index) => new { Band = band, Index = index })
            .Where(item => !targetIndexes.Contains(item.Index) && !item.Band.IsFallback)
            .OrderBy(item => Math.Abs(item.Band.Width - target.Width))
            .ThenBy(item => Math.Abs(
                (item.Band.SourceBottom - item.Band.SourceTop) -
                (target.SourceBottom - target.SourceTop)))
            .Select(item => item.Band)
            .FirstOrDefault();

        var pixels = (byte[])source.BgraPixels.Clone();
        foreach (var targetIndex in targetBandIndices)
        {
            var targetBand = bands[targetIndex];
            ClearBluePixels(
                pixels,
                source.Width,
                source.Stride,
                targetBand.SourceTop,
                targetBand.SourceBottom);
        }

        if (donor is not null)
        {
            var targetCenter = (target.SourceTop + target.SourceBottom) / 2;
            var donorCenter = (donor.SourceTop + donor.SourceBottom) / 2;
            var shiftY = targetCenter - donorCenter;
            for (var sourceY = donor.SourceTop; sourceY <= donor.SourceBottom; sourceY++)
            {
                var targetY = sourceY + shiftY;
                if (targetY < target.SourceTop || targetY > target.SourceBottom ||
                    targetY < 0 || targetY >= source.Height)
                {
                    continue;
                }

                for (var x = 0; x < source.Width; x++)
                {
                    var sourcePixel = (sourceY * source.Stride) + (x * 4);
                    if (!IsBlueAffixPixel(source.BgraPixels, sourcePixel))
                    {
                        continue;
                    }

                    var targetPixel = (targetY * source.Stride) + (x * 4);
                    pixels[targetPixel] = source.BgraPixels[sourcePixel];
                    pixels[targetPixel + 1] = source.BgraPixels[sourcePixel + 1];
                    pixels[targetPixel + 2] = source.BgraPixels[sourcePixel + 2];
                    pixels[targetPixel + 3] = source.BgraPixels[sourcePixel + 3];
                }
            }
        }
        else
        {
            // A one-row ROI has no safe donor. Mirror its real glyph mask; this preserves row
            // height and ink density while destroying the target sentence ordering.
            var original = source.BgraPixels;
            for (var y = target.SourceTop; y <= target.SourceBottom; y++)
            {
                for (var x = 0; x < source.Width; x++)
                {
                    var sourceX = source.Width - 1 - x;
                    var sourcePixel = (y * source.Stride) + (sourceX * 4);
                    if (!IsBlueAffixPixel(original, sourcePixel))
                    {
                        continue;
                    }

                    var targetPixel = (y * source.Stride) + (x * 4);
                    pixels[targetPixel] = original[sourcePixel];
                    pixels[targetPixel + 1] = original[sourcePixel + 1];
                    pixels[targetPixel + 2] = original[sourcePixel + 2];
                    pixels[targetPixel + 3] = original[sourcePixel + 3];
                }
            }
        }

        return source with
        {
            BgraPixels = pixels,
            CapturedAt = DateTimeOffset.UtcNow,
        };
    }

    private static void ClearBluePixels(
        byte[] pixels,
        int width,
        int stride,
        int firstRow,
        int lastRow)
    {
        for (var y = Math.Max(0, firstRow); y <= lastRow; y++)
        {
            for (var x = 0; x < width; x++)
            {
                var pixel = (y * stride) + (x * 4);
                if (IsBlueAffixPixel(pixels, pixel))
                {
                    Neutralize(pixels, pixel);
                }
            }
        }
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
        // Only the blue mask reaches PaddleOCR. Neutral black therefore removes an ink pixel
        // deterministically without introducing another UI colour into the candidate mask.
        pixels[pixel] = 0;
        pixels[pixel + 1] = 0;
        pixels[pixel + 2] = 0;
    }

    private sealed record FrameVariantSource(
        CapturedFrame BaseFrame,
        IReadOnlyList<int[]> MutablePixels)
    {
        public static FrameVariantSource Create(
            CapturedFrame frame,
            IReadOnlyList<PreparedFrame> bands)
        {
            var rows = new List<int[]>();
            foreach (var band in bands.Where(item => !item.IsFallback))
            {
                var candidates = new List<int>();
                for (var y = band.SourceTop; y <= band.SourceBottom; y++)
                {
                    for (var x = 0; x < frame.Width; x++)
                    {
                        var pixel = (y * frame.Stride) + (x * 4);
                        if (IsBlueAffixPixel(frame.BgraPixels, pixel))
                        {
                            candidates.Add(pixel);
                        }
                    }
                }

                if (candidates.Count > 0)
                {
                    rows.Add(candidates.ToArray());
                }
            }

            if (rows.Count == 0)
            {
                throw new InvalidDataException("Variant source contains no blue OCR pixels.");
            }

            return new FrameVariantSource(frame, rows);
        }

        public CapturedFrame CreateVariant(int id)
        {
            var pixels = (byte[])BaseFrame.BgraPixels.Clone();
            for (var rowIndex = 0; rowIndex < MutablePixels.Count; rowIndex++)
            {
                var candidates = MutablePixels[rowIndex];
                var first = Math.Abs((id * 17) + (rowIndex * 31)) % candidates.Length;
                var second = Math.Abs((id / Math.Max(1, candidates.Length)) +
                                      (rowIndex * 47) +
                                      (candidates.Length / 2)) % candidates.Length;
                Neutralize(pixels, candidates[first]);
                Neutralize(pixels, candidates[second]);
            }

            return BaseFrame with
            {
                BgraPixels = pixels,
                CapturedAt = DateTimeOffset.UtcNow,
            };
        }
    }
}
