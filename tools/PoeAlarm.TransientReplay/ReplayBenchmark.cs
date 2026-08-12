using System.Globalization;
using System.IO;
using System.Text;
using PoeAlarm.App.Monitoring;
using PoeAlarm.App.Recognition;
using PoeAlarm.App.Replay;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.TransientReplay;

internal static class ReplayBenchmark
{
    public static async Task<int> RunAsync(ReplayOptions options)
    {
        RequireFile(options.ManifestPath, "manifest");
        if (options.Recognizer == ReplayRecognizer.Paddle)
        {
            RequireFile(options.ModelPath, "ONNX model");
            RequireFile(options.DictionaryPath, "character dictionary");
        }

        var manifest = ReplayManifest.Load(options.ManifestPath);
        var scenarios = SelectScenarios(options, manifest);
        Console.WriteLine("POE Alarm transient target-frame replay");
        Console.WriteLine($"Manifest:   {options.ManifestPath}");
        Console.WriteLine($"Scenarios:  {scenarios.Count}");
        Console.WriteLine($"Dwell:      {string.Join(',', options.DurationsMs)} ms");
        Console.WriteLine($"Trials:     {options.Trials} per dwell/scenario");
        Console.WriteLine($"Mode:       {options.Mode}");
        Console.WriteLine($"Fingerprint: {options.Fingerprints}");
        Console.WriteLine($"Roll model: {options.RollModel}");
        Console.WriteLine($"Recognizer: {options.Recognizer}");
        Console.WriteLine(
            $"Target lead: stratified {options.LeadMinimumMs}..{options.LeadMaximumMs} ms; seed={options.Seed}");
        Console.WriteLine(options.Fingerprints == FingerprintMode.StableState
            ? "Cache:      stable fingerprint within each absent/present state (continuation validation)"
            : "Cache:      cold each capture (two-pixel real-frame mutation stress mode)");
        Console.WriteLine();

        var results = new List<TrialResult>();
        foreach (var scenario in scenarios)
        {
            if (options.Mode is ReplayMode.Baseline or ReplayMode.Both)
            {
                results.AddRange(await RunScenarioAsync(
                    options,
                    scenario,
                    "baseline-0.4.1-upper-bound",
                    exhaustive: true));
            }

            if (options.Mode is ReplayMode.Sliced or ReplayMode.Both)
            {
                results.AddRange(await RunScenarioAsync(options, scenario, "sliced", exhaustive: false));
            }
        }

        PrintSummary(results);
        if (options.CsvPath is not null)
        {
            WriteCsv(options.CsvPath, results);
            Console.WriteLine();
            Console.WriteLine($"Per-trial CSV: {options.CsvPath}");
        }

        var thresholdFailures = EvaluateThresholds(options, results);
        Console.WriteLine();
        Console.WriteLine(thresholdFailures.Count == 0
            ? "Acceptance: PASS"
            : $"Acceptance: FAIL ({thresholdFailures.Count})");
        foreach (var failure in thresholdFailures)
        {
            Console.WriteLine($"  {failure}");
        }

        return thresholdFailures.Count == 0 ? 0 : 3;
    }

    private static async Task<IReadOnlyList<TrialResult>> RunScenarioAsync(
        ReplayOptions options,
        ReplayScenario scenario,
        string mode,
        bool exhaustive)
    {
        if (scenario.Screenshot.Roi.Length != 4)
        {
            throw new InvalidDataException($"{scenario.Case.Id}: ROI must contain x,y,width,height.");
        }

        var roi = scenario.Screenshot.Roi;
        var sourceFrame = await new ScreenshotFrameLoader().LoadAsync(
            scenario.ImagePath,
            new PoeAlarm.App.Capture.ScreenRegion(roi[0], roi[1], roi[2], roi[3]));
        var frame = sourceFrame;
        IReadOnlyList<int> targetBandIndices = [scenario.Case.Band];
        int? wrappedSplitX = null;
        if (options.WrappedCaseIds.Contains(scenario.Case.Id))
        {
            var wrapped = WrappedTargetFrameBuilder.Create(sourceFrame, scenario.Case.Band);
            frame = wrapped.Frame;
            targetBandIndices = wrapped.TargetBandIndices;
            wrappedSplitX = wrapped.SplitX;
        }

        var frameSet = TransientFrameSet.Create(
            frame,
            targetBandIndices,
            options.Fingerprints);
        IReplayCapture capture = options.RollModel == RollModel.Guarded
            ? new GuardedRollCapture(frameSet)
            : new TimedSequenceCapture(frameSet);
        var matcher = new FullLineAffixMatcher(scenario.Case.Template);
        var schedulingRank = SchedulingRankEstimator.EstimateMinimumRank(
            frame,
            matcher,
            targetBandIndices);

        Console.WriteLine($"Scenario: {scenario.Case.Id} [{mode}]");
        Console.WriteLine(
            $"  image={Path.GetFileName(scenario.ImagePath)}; ROI={frame.Width}x{frame.Height}; " +
            $"target=\"{scenario.Case.Template}\"");
        if (wrappedSplitX is not null)
        {
            Console.WriteLine(
                $"  layout=real-pixel-derived wrapped variant; split-x={wrappedSplitX}; " +
                $"target-bands={string.Join(',', targetBandIndices)}");
        }

        PreflightDecision preflight;
        using (var preflightRecognizer = CreateRecognizer(options))
        {
            preflight = await ValidateAndWarmAsync(
                frameSet,
                preflightRecognizer,
                matcher,
                scenario.Case.Id);
        }

        Console.WriteLine(
            $"  preflight: route={preflight.Route}; physical-lines={preflight.PhysicalLineCount}; " +
            $"decision-slices={preflight.Slices}; estimated-scheduling-rank={schedulingRank}");

        IOcrRecognizer recognizer = CreateRecognizer(options);
        if (exhaustive)
        {
            recognizer = new ExhaustiveFrameRecognizer(recognizer);
        }

        await using var monitor = new AffixMonitor(capture, recognizer);
        TaskCompletionSource<Detection>? activeDetection = null;
        monitor.InputGuardRequested += (_, _) => capture.OnGuardRequested();
        monitor.InputGuardReleased += (_, _) => capture.OnGuardReleased();
        monitor.SnapshotChanged += (_, snapshot) =>
        {
            if (snapshot.ScanCount > 0)
            {
                capture.RecordDecision(
                    snapshot.ScanCount,
                    snapshot.OcrElapsed.TotalMilliseconds,
                    snapshot.OcrWasCached);
            }
        };
        monitor.AffixDetected += (_, eventArgs) =>
        {
            capture.OnMatch();
            var completionMs = capture.ElapsedMilliseconds;
            var sourceSample = capture.GetLastSample();
            Volatile.Read(ref activeDetection)?.TrySetResult(
                new Detection(
                    completionMs,
                    eventArgs.Snapshot.OcrElapsed.TotalMilliseconds,
                    sourceSample?.ElapsedMilliseconds,
                    sourceSample?.TargetPresent ?? false));
        };

        var results = new List<TrialResult>(options.DurationsMs.Count * options.Trials);
        foreach (var dwellMs in options.DurationsMs)
        {
            var leads = CreateStratifiedLeads(options, scenario.Case.Id, dwellMs);
            for (var trial = 0; trial < options.Trials; trial++)
            {
                var leadMs = leads[trial];
                capture.Configure(leadMs, dwellMs);
                var completion = new TaskCompletionSource<Detection>(
                    TaskCreationOptions.RunContinuationsAsynchronously);
                Volatile.Write(ref activeDetection, completion);
                monitor.Start(scenario.Case.Template, capture.Region);

                var deadlineMs = leadMs + dwellMs + options.TailMs;
                var timeout = Task.Delay(TimeSpan.FromMilliseconds(deadlineMs));
                var finished = await Task.WhenAny(completion.Task, timeout);
                Detection? detection = finished == completion.Task
                    ? await completion.Task
                    : null;
                await monitor.StopAsync();
                Volatile.Write(ref activeDetection, null);

                var samples = capture.GetSamples();
                var rollStats = capture.GetStats();
                var presentSamples = samples.Where(item => item.TargetPresent).ToArray();
                var firstPresentCaptureMs = presentSamples.Length == 0
                    ? (double?)null
                    : presentSamples[0].ElapsedMilliseconds;
                results.Add(new TrialResult(
                    scenario.Case.Id,
                    mode,
                    options.Fingerprints.ToString(),
                    dwellMs,
                    trial,
                    leadMs,
                    samples.Count,
                    presentSamples.Length,
                    firstPresentCaptureMs,
                    detection?.CompletionMs,
                    detection?.OcrMs,
                    detection?.SourceCaptureMs,
                    detection?.SourceTargetPresent ?? false,
                    rollStats.TargetOnsetMs,
                    rollStats.TargetEndMs,
                    rollStats.Overroll,
                    rollStats.PhysicalClicks,
                    rollStats.AcceptedClicks,
                    rollStats.SwallowedClicks,
                    rollStats.FirstGuardRequestedMs,
                    rollStats.LastGuardReleasedMs,
                    rollStats.GuardHeldMs));

                if (options.Trace is { } trace &&
                    trace.DwellMs == dwellMs &&
                    trace.Trial == trial)
                {
                    PrintTrace(scenario.Case.Id, mode, dwellMs, trial, capture.GetTraceEvents());
                }
            }

            PrintDuration(results.Where(item =>
                item.CaseId == scenario.Case.Id && item.Mode == mode && item.DwellMs == dwellMs).ToArray());
        }

        Console.WriteLine();
        return results;
    }

    private static IOcrRecognizer CreateRecognizer(ReplayOptions options) =>
        options.Recognizer switch
        {
            ReplayRecognizer.Paddle => new PaddleOcrRecognizer(
                options.ModelPath,
                options.DictionaryPath),
            ReplayRecognizer.WindowsEnglish => new Poe2EnglishRecognizer(),
            ReplayRecognizer.WindowsChinese => new WindowsChineseOcrRecognizer(),
            _ => throw new InvalidOperationException($"Unsupported replay recognizer: {options.Recognizer}"),
        };

    private static async Task<PreflightDecision> ValidateAndWarmAsync(
        TransientFrameSet frameSet,
        IOcrRecognizer recognizer,
        FullLineAffixMatcher matcher,
        string caseId)
    {
        PreflightDecision? firstPresent = null;
        for (var iteration = 0; iteration < 3; iteration++)
        {
            var absent = await RecognizeToDecisionAsync(
                frameSet.Next(targetPresent: false),
                recognizer,
                matcher);
            if (absent.Match is not null)
            {
                throw new InvalidDataException(
                    $"{caseId}: constructed absent frame falsely matches the target.");
            }

            var present = await RecognizeToDecisionAsync(
                frameSet.Next(targetPresent: true),
                recognizer,
                matcher);
            if (present.Match is null)
            {
                throw new InvalidDataException(
                    $"{caseId}: target-frame variant was not recognized during exhaustive preflight.");
            }

            firstPresent ??= present;
        }

        return firstPresent!;
    }

    private static async Task<PreflightDecision> RecognizeToDecisionAsync(
        PoeAlarm.App.Capture.CapturedFrame frame,
        IOcrRecognizer recognizer,
        FullLineAffixMatcher matcher)
    {
        for (var slice = 1; slice <= 128; slice++)
        {
            var result = await recognizer.RecognizeAsync(frame, matcher);
            if (matcher.TryFindMatch(result.Lines, out var strict) && strict is not null)
            {
                return new PreflightDecision(strict, "strict", slice);
            }

            if (result.TargetAssistedMatch is { } assisted &&
                string.Equals(
                    assisted.CanonicalText,
                    matcher.Template.Text,
                    StringComparison.Ordinal))
            {
                return new PreflightDecision(assisted, "ctc-assisted", slice);
            }

            if (!result.RequiresRescan)
            {
                return new PreflightDecision(null, "none", slice);
            }
        }

        throw new InvalidOperationException(
            "Preflight did not exhaust one stable frame within 128 continuation slices.");
    }

    private static IReadOnlyList<double> CreateStratifiedLeads(
        ReplayOptions options,
        string caseId,
        int dwellMs)
    {
        var width = options.LeadMaximumMs - options.LeadMinimumMs;
        var leads = Enumerable.Range(0, options.Trials)
            .Select(index => options.LeadMinimumMs + (((index + 0.5) / options.Trials) * width))
            .ToArray();
        var random = new Random(StableSeed(options.Seed, caseId, dwellMs));
        for (var index = leads.Length - 1; index > 0; index--)
        {
            var swap = random.Next(index + 1);
            (leads[index], leads[swap]) = (leads[swap], leads[index]);
        }

        return leads;
    }

    private static int StableSeed(int seed, string caseId, int dwellMs)
    {
        unchecked
        {
            var hash = seed;
            foreach (var character in caseId)
            {
                hash = (hash * 16777619) ^ character;
            }

            return (hash * 397) ^ dwellMs;
        }
    }

    private static IReadOnlyList<ReplayScenario> SelectScenarios(
        ReplayOptions options,
        ReplayManifest manifest)
    {
        var selected = manifest.Screenshots
            .SelectMany(screenshot => screenshot.Cases.Select(item =>
                new ReplayScenario(options.ManifestPath, screenshot, item)))
            .Where(scenario => options.AllCases || options.CaseIds.Contains(scenario.Case.Id))
            .ToArray();
        if (selected.Length == 0)
        {
            throw new ArgumentException(
                $"No selected case exists in {options.ManifestPath}. Requested: " +
                string.Join(',', options.CaseIds));
        }

        return selected;
    }

    private static void PrintDuration(IReadOnlyList<TrialResult> trials)
    {
        var sampled = trials.Count(item => item.PresentCaptureCount > 0);
        var detected = trials.Count(item => item.Hit);
        var timely = trials.Count(item => item.Timely);
        var stale = trials.Count(item => item.Stale);
        var capturedMiss = trials.Count(item => item.CapturedMiss);
        var overroll = trials.Count(item => item.Overroll);
        var earlyFalseAlerts = trials.Count(item => item.EarlyFalseAlert);
        var falseAlerts = trials.Count(item => item.FalseAlert);
        var onsetLatency = trials
            .Where(item => item.Hit && item.TargetOnsetMs is not null)
            .Select(item => item.DetectionMs!.Value - item.TargetOnsetMs!.Value)
            .ToArray();
        var capturedLatency = trials
            .Where(item => item.Hit && item.DetectionCaptureMs is not null)
            .Select(item => item.DetectionMs!.Value - item.DetectionCaptureMs!.Value)
            .ToArray();

        Console.WriteLine(
            $"  {trials[0].DwellMs,4} ms: sampled={sampled,2}/{trials.Count} ({Rate(sampled, trials.Count)}), " +
            $"hit={detected,2}/{trials.Count} ({Rate(detected, trials.Count)}), " +
            $"timely={timely,2}/{trials.Count} ({Rate(timely, trials.Count)}), " +
            $"stale={stale}, overroll={overroll}, captured-miss={capturedMiss}, " +
            $"early-false={earlyFalseAlerts}, false-alert={falseAlerts}");
        Console.WriteLine(
            $"           clicks: physical={trials.Sum(item => item.PhysicalClicks)}, " +
            $"accepted={trials.Sum(item => item.AcceptedClicks)}, " +
            $"swallowed={trials.Sum(item => item.SwallowedClicks)}; " +
            $"guard-held: {Format(trials.Select(item => item.GuardHeldMs).ToArray())}");
        Console.WriteLine($"           onset->decision: {Format(onsetLatency)}");
        Console.WriteLine($"           capture->decision: {Format(capturedLatency)}");
    }

    private static void PrintSummary(IReadOnlyList<TrialResult> results)
    {
        Console.WriteLine("Aggregate by dwell:");
        foreach (var mode in results.GroupBy(item => item.Mode).OrderBy(group => group.Key))
        {
            Console.WriteLine($"  mode={mode.Key}");
            foreach (var group in mode.GroupBy(item => item.DwellMs).OrderBy(group => group.Key))
            {
                PrintDuration(group.ToArray());
            }
        }
    }

    private static string Rate(int count, int total) =>
        (count / (double)total).ToString("P0", CultureInfo.InvariantCulture);

    private static string Format(IReadOnlyCollection<double> samples) =>
        samples.Count == 0 ? "n/a" : Distribution.Create(samples).ToString();

    private static IReadOnlyList<string> EvaluateThresholds(
        ReplayOptions options,
        IReadOnlyList<TrialResult> results)
    {
        var candidateMode = results.Any(item => item.Mode == "sliced")
            ? "sliced"
            : "baseline-0.4.1-upper-bound";
        var failures = new List<string>();
        foreach (var scenario in results
                     .Where(item => item.Mode == candidateMode)
                     .GroupBy(item => item.CaseId))
        {
            var falseAlerts = scenario.Count(item => item.FalseAlert);
            if (falseAlerts > 0)
            {
                failures.Add($"{scenario.Key}: false-alert={falseAlerts}, required 0");
            }

            var capturedMisses = scenario.Count(item => item.CapturedMiss);
            if (capturedMisses > 0)
            {
                failures.Add($"{scenario.Key}: captured-miss={capturedMisses}, required 0");
            }

            if (options.RollModel == RollModel.Guarded)
            {
                var missedTargets = scenario.Count(item => !item.Hit);
                if (missedTargets > 0)
                {
                    failures.Add(
                        $"{scenario.Key}: guarded final-hit={scenario.Count(item => item.Hit)}/{scenario.Count()}, " +
                        "required 100%");
                }

                var overrolls = scenario.Count(item => item.Overroll);
                if (overrolls > 0)
                {
                    failures.Add($"{scenario.Key}: overroll={overrolls}, required 0 in guarded mode");
                }
            }

            foreach (var threshold in AcceptanceThresholds)
            {
                var dwell = scenario.Where(item => item.DwellMs == threshold.DwellMs).ToArray();
                if (dwell.Length == 0)
                {
                    continue;
                }

                var hitRate = dwell.Count(item => item.Hit) / (double)dwell.Length;
                var timelyRate = dwell.Count(item => item.Timely) / (double)dwell.Length;
                if (hitRate + 1e-12 < threshold.MinimumHitRate)
                {
                    failures.Add(
                        $"{scenario.Key}/{threshold.DwellMs}ms: hit={hitRate:P0}, " +
                        $"required {threshold.MinimumHitRate:P0}");
                }

                if (timelyRate + 1e-12 < threshold.MinimumTimelyRate)
                {
                    failures.Add(
                        $"{scenario.Key}/{threshold.DwellMs}ms: timely={timelyRate:P0}, " +
                        $"required {threshold.MinimumTimelyRate:P0}");
                }
            }
        }

        return failures;
    }

    private static void WriteCsv(string path, IReadOnlyList<TrialResult> results)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        using var writer = new StreamWriter(path, append: false, new UTF8Encoding(false));
        writer.WriteLine(
            "case,mode,fingerprints,dwell_ms,trial,first_click_ms,scan_count,present_capture_count," +
            "first_present_capture_ms,detection_ms,ocr_ms,detection_capture_ms," +
            "source_target_present,target_onset_ms,target_end_ms,hit,timely,stale,overroll," +
            "physical_clicks,accepted_clicks,swallowed_clicks,first_guard_requested_ms," +
            "last_guard_released_ms,guard_held_ms,captured_miss,early_false_alert,false_alert");
        foreach (var item in results)
        {
            writer.WriteLine(string.Join(',',
                Escape(item.CaseId),
                Escape(item.Mode),
                Escape(item.Fingerprints),
                F(item.DwellMs),
                F(item.Trial),
                F(item.LeadMs),
                F(item.ScanCount),
                F(item.PresentCaptureCount),
                F(item.FirstPresentCaptureMs),
                F(item.DetectionMs),
                F(item.OcrMs),
                F(item.DetectionCaptureMs),
                item.SourceTargetPresent ? "true" : "false",
                F(item.TargetOnsetMs),
                F(item.TargetEndMs),
                item.Hit ? "true" : "false",
                item.Timely ? "true" : "false",
                item.Stale ? "true" : "false",
                item.Overroll ? "true" : "false",
                F(item.PhysicalClicks),
                F(item.AcceptedClicks),
                F(item.SwallowedClicks),
                F(item.FirstGuardRequestedMs),
                F(item.LastGuardReleasedMs),
                F(item.GuardHeldMs),
                item.CapturedMiss ? "true" : "false",
                item.EarlyFalseAlert ? "true" : "false",
                item.FalseAlert ? "true" : "false"));
        }

        static string F(object? value) => value switch
        {
            null => string.Empty,
            IFormattable formattable => formattable.ToString(null, CultureInfo.InvariantCulture),
            _ => value.ToString() ?? string.Empty,
        };

        static string Escape(string value) => $"\"{value.Replace("\"", "\"\"")}\"";
    }

    private static void RequireFile(string path, string description)
    {
        if (!File.Exists(path))
        {
            throw new FileNotFoundException($"The {description} was not found: {path}", path);
        }
    }

    private static void PrintTrace(
        string caseId,
        string mode,
        int dwellMs,
        int trial,
        IReadOnlyList<ReplayTraceEvent> events)
    {
        Console.WriteLine();
        Console.WriteLine($"TRACE {caseId} [{mode}] dwell={dwellMs} trial={trial}");
        foreach (var item in events)
        {
            Console.WriteLine($"  {item.ElapsedMs,8:F2} ms  {item.Kind,-17} {item.Detail}");
        }

        Console.WriteLine();
    }

    private static readonly AcceptanceThreshold[] AcceptanceThresholds =
    [
        new(40, MinimumHitRate: 1.00, MinimumTimelyRate: 0.95),
        new(50, MinimumHitRate: 1.00, MinimumTimelyRate: 0.95),
        new(60, MinimumHitRate: 1.00, MinimumTimelyRate: 1.00),
        new(80, MinimumHitRate: 1.00, MinimumTimelyRate: 1.00),
        new(100, MinimumHitRate: 1.00, MinimumTimelyRate: 1.00),
    ];

    private sealed record Detection(
        double CompletionMs,
        double OcrMs,
        double? SourceCaptureMs,
        bool SourceTargetPresent);

    private sealed record AcceptanceThreshold(
        int DwellMs,
        double MinimumHitRate,
        double MinimumTimelyRate);

    private sealed record PreflightDecision(
        LogicalAffixMatch? Match,
        string Route,
        int Slices)
    {
        public int PhysicalLineCount => Match?.PhysicalLineCount ?? 0;
    }
}

internal sealed record TrialResult(
    string CaseId,
    string Mode,
    string Fingerprints,
    int DwellMs,
    int Trial,
    double LeadMs,
    int ScanCount,
    int PresentCaptureCount,
    double? FirstPresentCaptureMs,
    double? DetectionMs,
    double? OcrMs,
    double? DetectionCaptureMs,
    bool SourceTargetPresent,
    double? TargetOnsetMs,
    double? TargetEndMs,
    bool Overroll,
    int PhysicalClicks,
    int AcceptedClicks,
    int SwallowedClicks,
    double? FirstGuardRequestedMs,
    double? LastGuardReleasedMs,
    double GuardHeldMs)
{
    public bool EarlyFalseAlert =>
        DetectionMs is not null &&
        !SourceTargetPresent &&
        (TargetOnsetMs is null || DetectionMs.Value < TargetOnsetMs.Value);

    public bool FalseAlert => DetectionMs is not null && !SourceTargetPresent;

    public bool Hit =>
        DetectionMs is not null &&
        SourceTargetPresent &&
        TargetOnsetMs is not null &&
        DetectionMs.Value >= TargetOnsetMs.Value;

    public bool Timely =>
        Hit &&
        DetectionMs!.Value >= TargetOnsetMs!.Value &&
        !Overroll &&
        (TargetEndMs is null || DetectionMs.Value < TargetEndMs.Value);

    public bool Stale =>
        Hit &&
        (Overroll || TargetEndMs is not null && DetectionMs!.Value >= TargetEndMs.Value);

    public bool CapturedMiss => PresentCaptureCount > 0 && !Hit;
}
