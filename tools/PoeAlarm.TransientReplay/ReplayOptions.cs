using System.Globalization;
using System.IO;

namespace PoeAlarm.TransientReplay;

internal sealed record ReplayOptions(
    string ManifestPath,
    IReadOnlySet<string> CaseIds,
    bool AllCases,
    string ModelPath,
    string DictionaryPath,
    IReadOnlyList<int> DurationsMs,
    int Trials,
    int LeadMinimumMs,
    int LeadMaximumMs,
    int TailMs,
    int Seed,
    ReplayMode Mode,
    FingerprintMode Fingerprints,
    RollModel RollModel,
    ReplayRecognizer Recognizer,
    TraceSpec? Trace,
    IReadOnlySet<string> WrappedCaseIds,
    string? CsvPath,
    bool ShowHelp)
{
    public static ReplayOptions Parse(string[] arguments)
    {
        var manifest = @"tests\screenshots\traditional-ocr-cases.json";
        var caseIds = new HashSet<string>(StringComparer.OrdinalIgnoreCase)
        {
            "weapon-attack-speed",
        };
        var caseWasSpecified = false;
        var allCases = false;
        var model = @"src\PoeAlarm.App\Assets\Ocr\PP-OCRv5_mobile_rec.onnx";
        var dictionary = @"src\PoeAlarm.App\Assets\Ocr\ppocrv5_dict.txt";
        IReadOnlyList<int> durations = [30, 40, 50, 60, 80, 100];
        var trials = 20;
        var leadMinimum = 15;
        var leadMaximum = 240;
        var tail = 450;
        var seed = 20260811;
        var mode = ReplayMode.Sliced;
        var fingerprints = FingerprintMode.StableState;
        var rollModel = RollModel.Guarded;
        var recognizer = ReplayRecognizer.Paddle;
        TraceSpec? trace = null;
        var wrappedCaseIds = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        string? csv = null;
        var showHelp = false;

        for (var index = 0; index < arguments.Length; index++)
        {
            var option = arguments[index].ToLowerInvariant();
            switch (option)
            {
                case "-h":
                case "--help":
                    showHelp = true;
                    break;
                case "--manifest":
                    manifest = Next(arguments, ref index, option);
                    break;
                case "--case":
                    if (!caseWasSpecified)
                    {
                        caseIds.Clear();
                        caseWasSpecified = true;
                    }

                    caseIds.Add(Next(arguments, ref index, option));
                    break;
                case "--all-cases":
                    allCases = true;
                    caseIds.Clear();
                    break;
                case "--model":
                    model = Next(arguments, ref index, option);
                    break;
                case "--dict":
                    dictionary = Next(arguments, ref index, option);
                    break;
                case "--durations":
                    durations = ParseDurations(Next(arguments, ref index, option));
                    break;
                case "--trials":
                    trials = ParseInt(arguments, ref index, option, 1, 1000);
                    break;
                case "--lead-min":
                    leadMinimum = ParseInt(arguments, ref index, option, 0, 5000);
                    break;
                case "--lead-max":
                    leadMaximum = ParseInt(arguments, ref index, option, 1, 5000);
                    break;
                case "--tail":
                    tail = ParseInt(arguments, ref index, option, 50, 10000);
                    break;
                case "--seed":
                    seed = ParseInt(arguments, ref index, option, 0, int.MaxValue);
                    break;
                case "--mode":
                    mode = ParseMode(Next(arguments, ref index, option));
                    break;
                case "--fingerprints":
                    fingerprints = ParseFingerprints(Next(arguments, ref index, option));
                    break;
                case "--roll-model":
                    rollModel = ParseRollModel(Next(arguments, ref index, option));
                    break;
                case "--recognizer":
                    recognizer = ParseRecognizer(Next(arguments, ref index, option));
                    break;
                case "--trace":
                    trace = ParseTrace(Next(arguments, ref index, option));
                    break;
                case "--wrap-case":
                    wrappedCaseIds.Add(Next(arguments, ref index, option));
                    break;
                case "--csv":
                    csv = Next(arguments, ref index, option);
                    break;
                default:
                    throw new ArgumentException($"Unknown option: {arguments[index]}");
            }
        }

        if (leadMaximum <= leadMinimum)
        {
            throw new ArgumentException("--lead-max must be greater than --lead-min.");
        }

        return new ReplayOptions(
            Path.GetFullPath(manifest),
            caseIds,
            allCases,
            Path.GetFullPath(model),
            Path.GetFullPath(dictionary),
            durations,
            trials,
            leadMinimum,
            leadMaximum,
            tail,
            seed,
            mode,
            fingerprints,
            rollModel,
            recognizer,
            trace,
            wrappedCaseIds,
            csv is null ? null : Path.GetFullPath(csv),
            showHelp);
    }

    public static void PrintHelp()
    {
        Console.WriteLine("POE Alarm transient target-frame replay benchmark");
        Console.WriteLine();
        Console.WriteLine("Replays the production AffixMonitor loop against a real screenshot ROI:");
        Console.WriteLine("target absent -> target visible for N ms -> target absent.");
        Console.WriteLine();
        Console.WriteLine("Options:");
        Console.WriteLine("  --manifest path          Screenshot regression manifest");
        Console.WriteLine("  --case id                Case to replay; repeat for multiple cases");
        Console.WriteLine("  --all-cases              Replay every case in the manifest (slow)");
        Console.WriteLine("  --durations list         Comma-separated dwell times (default: 30,40,50,60,80,100)");
        Console.WriteLine("  --trials N               Phase samples per dwell time (default: 20)");
        Console.WriteLine("  --lead-min N             Earliest target onset after monitor start (default: 15 ms)");
        Console.WriteLine("  --lead-max N             Latest target onset after monitor start (default: 240 ms)");
        Console.WriteLine("  --tail N                 Post-target observation tail (default: 450 ms)");
        Console.WriteLine("  --seed N                 Deterministic phase-order seed (default: 20260811)");
        Console.WriteLine("  --mode value             baseline|sliced|both (default: sliced)");
        Console.WriteLine("  --fingerprints value     stable-state|cold-each-capture (default: stable-state)");
        Console.WriteLine("  --roll-model value       guarded|fixed-dwell (default: guarded)");
        Console.WriteLine("  --recognizer value       paddle|windows-en|windows-zh (default: paddle)");
        Console.WriteLine("  --trace dwell:trial      Print one trial's capture/guard/OCR timeline");
        Console.WriteLine("  --wrap-case id           Split a real target row into two adjacent physical rows");
        Console.WriteLine("  --model path             PP-OCRv5 ONNX model");
        Console.WriteLine("  --dict path              PP-OCRv5 character dictionary");
        Console.WriteLine("  --csv path               Optional per-trial CSV output");
        Console.WriteLine("  -h, --help               Show help");
    }

    private static string Next(string[] arguments, ref int index, string option)
    {
        if (++index >= arguments.Length)
        {
            throw new ArgumentException($"{option} requires a value.");
        }

        return arguments[index];
    }

    private static int ParseInt(
        string[] arguments,
        ref int index,
        string option,
        int minimum,
        int maximum)
    {
        var text = Next(arguments, ref index, option);
        if (!int.TryParse(text, NumberStyles.None, CultureInfo.InvariantCulture, out var value) ||
            value < minimum || value > maximum)
        {
            throw new ArgumentException($"{option} must be from {minimum} through {maximum}.");
        }

        return value;
    }

    private static IReadOnlyList<int> ParseDurations(string value)
    {
        var parsed = value
            .Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .Select(item => int.TryParse(item, NumberStyles.None, CultureInfo.InvariantCulture, out var duration)
                ? duration
                : -1)
            .ToArray();
        if (parsed.Length == 0 || parsed.Any(duration => duration is < 1 or > 5000))
        {
            throw new ArgumentException("--durations must contain comma-separated values from 1 through 5000 ms.");
        }

        return parsed.Distinct().Order().ToArray();
    }

    private static ReplayMode ParseMode(string value) => value.ToLowerInvariant() switch
    {
        "baseline" => ReplayMode.Baseline,
        "sliced" => ReplayMode.Sliced,
        "both" => ReplayMode.Both,
        _ => throw new ArgumentException("--mode must be baseline, sliced, or both."),
    };

    private static FingerprintMode ParseFingerprints(string value) => value.ToLowerInvariant() switch
    {
        "stable-state" => FingerprintMode.StableState,
        "cold-each-capture" => FingerprintMode.ColdEachCapture,
        _ => throw new ArgumentException(
            "--fingerprints must be stable-state or cold-each-capture."),
    };

    private static RollModel ParseRollModel(string value) => value.ToLowerInvariant() switch
    {
        "guarded" => RollModel.Guarded,
        "fixed-dwell" => RollModel.FixedDwell,
        _ => throw new ArgumentException("--roll-model must be guarded or fixed-dwell."),
    };

    private static ReplayRecognizer ParseRecognizer(string value) => value.ToLowerInvariant() switch
    {
        "paddle" => ReplayRecognizer.Paddle,
        "windows-en" => ReplayRecognizer.WindowsEnglish,
        "windows-zh" => ReplayRecognizer.WindowsChinese,
        _ => throw new ArgumentException("--recognizer must be paddle, windows-en, or windows-zh."),
    };

    private static TraceSpec ParseTrace(string value)
    {
        var pieces = value.Split(':', StringSplitOptions.TrimEntries);
        if (pieces.Length != 2 ||
            !int.TryParse(pieces[0], NumberStyles.None, CultureInfo.InvariantCulture, out var dwell) ||
            !int.TryParse(pieces[1], NumberStyles.None, CultureInfo.InvariantCulture, out var trial) ||
            dwell < 1 || trial < 0)
        {
            throw new ArgumentException("--trace must use dwell:trial, for example 100:0.");
        }

        return new TraceSpec(dwell, trial);
    }
}

internal enum ReplayRecognizer
{
    Paddle,
    WindowsEnglish,
    WindowsChinese,
}

internal enum ReplayMode
{
    Baseline,
    Sliced,
    Both,
}

internal enum FingerprintMode
{
    StableState,
    ColdEachCapture,
}

internal enum RollModel
{
    Guarded,
    FixedDwell,
}

internal sealed record TraceSpec(int DwellMs, int Trial);
