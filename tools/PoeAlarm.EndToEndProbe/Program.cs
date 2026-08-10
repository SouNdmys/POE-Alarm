using System.IO;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Recognition;
using PoeAlarm.App.Replay;

const string defaultScreenshot =
    @"%TEMP%\poe-alarm-probe.png";
const string defaultTemplate =
    "(6-8)% increased Attack Speed if you've dealt a Critical Strike Recently";

var screenshot = args.Length > 0 ? Path.GetFullPath(args[0]) : defaultScreenshot;
var template = args.Length > 1 ? args[1] : defaultTemplate;
ScreenRegion? crop = null;
if (args.Length == 6 &&
    int.TryParse(args[2], out var x) &&
    int.TryParse(args[3], out var y) &&
    int.TryParse(args[4], out var width) &&
    int.TryParse(args[5], out var height))
{
    crop = new ScreenRegion(x, y, width, height);
}

Console.WriteLine($"Screenshot: {screenshot}");
Console.WriteLine($"Template:   {template}");
Console.WriteLine($"Crop:       {crop?.ToString() ?? "full image"}");

var diagnosticFrame = await new ScreenshotFrameLoader().LoadAsync(screenshot, crop);
var diagnosticBands = new PoeTextPreprocessor().PrepareBlueAffixBands(diagnosticFrame);
Console.WriteLine($"Blue bands: {diagnosticBands.Count}");
for (var index = 0; index < diagnosticBands.Count; index++)
{
    Console.WriteLine($"  band[{index}] {diagnosticBands[index].Width} × {diagnosticBands[index].Height}");
}

using var recognizer = new WindowsOcrRecognizer(scale: 1);
var analyzer = new ScreenshotAffixAnalyzer(recognizer);
var result = await analyzer.AnalyzeAsync(screenshot, template, crop);
var unchangedResult = await recognizer.RecognizeAsync(diagnosticFrame);

Console.WriteLine(
    $"Timing: load={result.ImageLoadElapsed.TotalMilliseconds:F1} ms, " +
    $"preprocess={result.PreprocessingElapsed.TotalMilliseconds:F1} ms, " +
    $"ocr={result.RecognitionElapsed.TotalMilliseconds:F1} ms");
Console.WriteLine(
    $"Unchanged frame: total={unchangedResult.TotalElapsed.TotalMilliseconds:F1} ms, " +
    $"cached={unchangedResult.WasCached}");
Console.WriteLine("OCR lines:");
for (var index = 0; index < result.Lines.Count; index++)
{
    Console.WriteLine($"  [{index}] {result.Lines[index]}");
}

Console.WriteLine(result.IsMatch
    ? $"MATCH: {result.Match!.OriginalText}"
    : "NO MATCH");

return result.IsMatch ? 0 : 1;
