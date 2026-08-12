using System.Diagnostics;
using System.IO;
using System.Security.Cryptography;
using System.Text.Json;

namespace PoeAlarm.App.Recognition;

internal static class PaddleOcrSelfTest
{
    private const string ExpectedModelSha256 =
        "DA72DC72CA4DC220DF0DFDE68C1DEDC31C58D3E76A25871122E5056227D50092";
    private const string ExpectedDictionarySha256 =
        "D1979E9F794C464C0D2E0B70A7FE14DD978E9DC644C0E71F14158CDF8342AF1B";

    public static int Run(string reportPath)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(reportPath);
        var watch = Stopwatch.StartNew();
        OcrSelfTestReport report;
        try
        {
            var assets = PackagedPaddleOcrAssets.Load();
            var modelHash = Convert.ToHexString(SHA256.HashData(assets.ModelBytes));
            var dictionaryHash = Convert.ToHexString(SHA256.HashData(assets.DictionaryBytes));
            Assert(modelHash == ExpectedModelSha256, "Packaged model SHA-256 does not match the tested asset.");
            Assert(dictionaryHash == ExpectedDictionarySha256, "Packaged dictionary SHA-256 does not match the tested asset.");
            Assert(assets.Dictionary.Count == 18_383, "Packaged dictionary must contain 18,383 entries.");
            Assert(assets.Dictionary[0] == "　", "Packaged dictionary first entry changed.");
            Assert(assets.Dictionary[^1] == "🕧", "Packaged dictionary last entry changed.");

            using var session = new PaddleCtcSession(
                assets.ModelBytes,
                assets.Dictionary,
                threads: 2,
                TextPolarity.BrightOnDark,
                inkMargin: 2);
            Assert(session.InputDimensions.SequenceEqual([-1, 3, 48, -1]),
                $"Unexpected model input shape [{string.Join(',', session.InputDimensions)}].");
            Assert(session.OutputDimensions.Count == 3 && session.OutputDimensions[^1] == 18_385,
                $"Unexpected model output shape [{string.Join(',', session.OutputDimensions)}].");

            var pixels = new byte[64 * 48 * 4];
            for (var pixel = 3; pixel < pixels.Length; pixel += 4)
            {
                pixels[pixel] = byte.MaxValue;
            }

            var inference = session.Recognize(new PreparedFrame(
                64,
                48,
                64 * 4,
                pixels,
                pixels.Length,
                ContentFingerprint: 0));
            watch.Stop();
            report = new OcrSelfTestReport(
                Success: true,
                Version: typeof(PaddleOcrSelfTest).Assembly.GetName().Version?.ToString() ?? "unknown",
                ModelBytes: assets.ModelBytes.Length,
                ModelSha256: modelHash,
                DictionaryBytes: assets.DictionaryBytes.Length,
                DictionarySha256: dictionaryHash,
                DictionaryEntries: assets.Dictionary.Count,
                SessionCreationMilliseconds: session.SessionCreationElapsed.TotalMilliseconds,
                InferenceMilliseconds: inference.TotalElapsed.TotalMilliseconds,
                TotalMilliseconds: watch.Elapsed.TotalMilliseconds,
                Error: null);
        }
        catch (Exception exception)
        {
            watch.Stop();
            report = new OcrSelfTestReport(
                Success: false,
                Version: typeof(PaddleOcrSelfTest).Assembly.GetName().Version?.ToString() ?? "unknown",
                ModelBytes: 0,
                ModelSha256: string.Empty,
                DictionaryBytes: 0,
                DictionarySha256: string.Empty,
                DictionaryEntries: 0,
                SessionCreationMilliseconds: 0,
                InferenceMilliseconds: 0,
                TotalMilliseconds: watch.Elapsed.TotalMilliseconds,
                Error: $"{exception.GetType().Name}: {exception.Message}");
        }

        var resolvedPath = Path.GetFullPath(reportPath);
        Directory.CreateDirectory(Path.GetDirectoryName(resolvedPath)!);
        File.WriteAllText(
            resolvedPath,
            JsonSerializer.Serialize(report, new JsonSerializerOptions { WriteIndented = true }));
        return report.Success ? 0 : 1;
    }

    private static void Assert(bool condition, string message)
    {
        if (!condition)
        {
            throw new InvalidDataException(message);
        }
    }

    private sealed record OcrSelfTestReport(
        bool Success,
        string Version,
        int ModelBytes,
        string ModelSha256,
        int DictionaryBytes,
        string DictionarySha256,
        int DictionaryEntries,
        double SessionCreationMilliseconds,
        double InferenceMilliseconds,
        double TotalMilliseconds,
        string? Error);
}
