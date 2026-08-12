using System.IO;
using System.Text.Json;

namespace PoeAlarm.TransientReplay;

internal sealed record ReplayManifest(
    int Version,
    IReadOnlyList<ReplayScreenshot> Screenshots)
{
    public static ReplayManifest Load(string path)
    {
        var manifest = JsonSerializer.Deserialize<ReplayManifest>(
            File.ReadAllText(path),
            new JsonSerializerOptions { PropertyNameCaseInsensitive = true });
        return manifest ?? throw new InvalidDataException($"Cannot parse replay manifest: {path}");
    }
}

internal sealed record ReplayScreenshot(
    string Image,
    int[] Roi,
    int ExpectedBandCount,
    IReadOnlyList<int> IgnoredBands,
    IReadOnlyList<ReplayCase> Cases);

internal sealed record ReplayCase(
    string Id,
    int Band,
    string Text,
    string Template,
    string Category);

internal sealed record ReplayScenario(
    string ManifestPath,
    ReplayScreenshot Screenshot,
    ReplayCase Case)
{
    public string ImagePath => Path.GetFullPath(
        Path.Combine(Path.GetDirectoryName(ManifestPath)!, Screenshot.Image));
}
