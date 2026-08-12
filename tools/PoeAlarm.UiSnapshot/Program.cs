using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using PoeAlarm.App;
using PoeAlarm.App.Alerts;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Configuration;
using PoeAlarm.App.Monitoring;
using PoeAlarm.App.Recognition;
using PoeAlarm.Core.Matching;

internal static class Program
{
    [STAThread]
    public static int Main(string[] args)
    {
        var corpusArgument = Array.FindIndex(args, argument =>
            argument.Equals("--poedb-corpus", StringComparison.OrdinalIgnoreCase));
        if (corpusArgument >= 0)
        {
            if (corpusArgument + 1 >= args.Length)
            {
                throw new ArgumentException("--poedb-corpus requires a JSON path.");
            }

            return RunPoedbCorpus(Path.GetFullPath(args[corpusArgument + 1]));
        }

        var alertMode = args.Contains("--alert", StringComparer.OrdinalIgnoreCase);
        var assertBlocking = args.Contains("--assert-blocking", StringComparer.OrdinalIgnoreCase);
        var assertLiveBlocking = args.Contains("--assert-live-blocking", StringComparer.OrdinalIgnoreCase);
        var assertMonitorWait = args.Contains("--assert-monitor-wait", StringComparer.OrdinalIgnoreCase);
        var assertStopMatchRace = args.Contains("--assert-stop-match-race", StringComparer.OrdinalIgnoreCase);
        var assertBandFingerprints = args.Contains("--assert-band-fingerprints", StringComparer.OrdinalIgnoreCase);
        var assertTargetAwareMonitor = args.Contains("--assert-target-aware-monitor", StringComparer.OrdinalIgnoreCase);
        var assertConcurrentDispose = args.Contains("--assert-concurrent-dispose", StringComparer.OrdinalIgnoreCase);
        var assertInputGuard = args.Contains("--assert-input-guard", StringComparer.OrdinalIgnoreCase);
        var assertSettingsProfiles = args.Contains(
            "--assert-settings-profiles",
            StringComparer.OrdinalIgnoreCase);
        var assertStartHotKey = args.Contains(
            "--assert-start-hotkey",
            StringComparer.OrdinalIgnoreCase);
        var hudMode = args.Contains("--hud", StringComparer.OrdinalIgnoreCase);
        var assertHud = args.Contains("--assert-hud", StringComparer.OrdinalIgnoreCase);
        var bottomMode = args.Contains("--bottom", StringComparer.OrdinalIgnoreCase);
        var helpMode = args.Contains("--help-window", StringComparer.OrdinalIgnoreCase);
        var englishMode = args.Contains("--english", StringComparer.OrdinalIgnoreCase);
        var poe2Mode = args.Contains("--poe2", StringComparer.OrdinalIgnoreCase);
        var pathArgument = args.FirstOrDefault(argument => !argument.StartsWith("--", StringComparison.Ordinal));
        var outputPath = pathArgument is not null
            ? Path.GetFullPath(pathArgument)
            : Path.GetFullPath(Path.Combine(
                "artifacts",
                alertMode ? "ui-alert.png" : hudMode ? "ui-hud.png" : "ui-main.png"));

        var outputDirectory = Path.GetDirectoryName(outputPath)
                              ?? throw new InvalidOperationException("Snapshot path has no parent directory.");
        Directory.CreateDirectory(outputDirectory);

        var application = new App { ShutdownMode = ShutdownMode.OnExplicitShutdown };
        Exception? dispatcherFailure = null;
        application.DispatcherUnhandledException += (_, eventArgs) =>
        {
            dispatcherFailure ??= eventArgs.Exception;
            Console.Error.WriteLine("Unhandled WPF dispatcher exception:");
            Console.Error.WriteLine(eventArgs.Exception);
            // This is a test executable: report a deterministic non-zero exit instead of letting
            // Windows surface a hidden-window CLR error dialog after Main has returned.
            eventArgs.Handled = true;
        };

        var exitCode = 0;
        try
        {
            application.InitializeComponent();
            if (assertSettingsProfiles)
            {
                SettingsProfileAssertions.RunAsync().GetAwaiter().GetResult();
            }
            if (assertMonitorWait)
            {
                AssertMonitorWaitsIndefinitelyAsync().GetAwaiter().GetResult();
            }

            if (assertStopMatchRace)
            {
                AssertStopWinsPendingRecognitionAsync().GetAwaiter().GetResult();
            }

            if (assertBandFingerprints)
            {
                AssertBandFingerprintsIgnoreVerticalPosition();
            }

            if (assertTargetAwareMonitor)
            {
                AssertTargetAwareMonitorAsync().GetAwaiter().GetResult();
            }

            if (assertConcurrentDispose)
            {
                AssertConcurrentDisposeAsync().GetAwaiter().GetResult();
            }

            if (assertInputGuard)
            {
                AssertPendingMouseInputGuardStateMachine();
                AssertPendingMouseInputGuardHookLifecycle();
                AssertInputGuardKeepsOverlayHidden();
                AssertAlertHandoffFailsOpenWhenDispatcherStalls();
                AssertProgressiveInputGuardAsync().GetAwaiter().GetResult();
            }

            if (helpMode)
            {
                RenderHelpWindow(outputPath, englishMode);
            }
            else if (hudMode)
            {
                RenderHud(outputPath, assertHud);
            }
            else if (alertMode)
            {
                RenderAlert(outputPath, assertBlocking, assertLiveBlocking);
            }
            else
            {
                RenderMainWindow(
                    outputPath,
                    bottomMode,
                    englishMode,
                    poe2Mode,
                    assertStartHotKey);
            }

            // Drain work posted by Close/Dispose paths before judging the run successful. In
            // particular, MainWindow has an async-void Closing handler whose first Close is
            // intentionally cancelled while settings and monitor resources are released.
            application.Dispatcher.Invoke(static () => { }, DispatcherPriority.ApplicationIdle);
            if (dispatcherFailure is not null)
            {
                throw new InvalidOperationException(
                    "A WPF dispatcher callback failed during snapshot cleanup.",
                    dispatcherFailure);
            }

            Console.WriteLine(outputPath);
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine("POE Alarm UI snapshot failed:");
            Console.Error.WriteLine(exception);
            exitCode = 1;
        }
        finally
        {
            try
            {
                application.Shutdown(exitCode);
            }
            catch (Exception exception)
            {
                Console.Error.WriteLine("POE Alarm UI snapshot shutdown failed:");
                Console.Error.WriteLine(exception);
                exitCode = 1;
            }
        }

        return exitCode;
    }

    private static int RunPoedbCorpus(string corpusPath)
    {
        var corpus = JsonSerializer.Deserialize<PoedbCorpus>(
                         File.ReadAllText(corpusPath),
                         new JsonSerializerOptions { PropertyNameCaseInsensitive = true })
                     ?? throw new InvalidDataException("PoEDB corpus is empty.");
        using var recognizer = new WindowsChineseOcrRecognizer();
        var failures = new List<string>();
        var falseAlerts = new List<string>();
        var assisted = 0;
        var cached = 0;
        var decisionTimes = new List<double>();

        Console.WriteLine("POE Alarm synthetic PoEDB Traditional Chinese target-matching regression");
        Console.WriteLine($"Corpus:     {corpusPath}");
        Console.WriteLine($"Recognizer: {recognizer.RecognizerLanguageTag}");
        Console.WriteLine($"Cases:      {corpus.Cases.Count}");

        foreach (var corpusCase in corpus.Cases)
        {
            var renderedText = MaterializeNumericSlots(corpusCase.Template);
            var frame = RenderSyntheticAffix(renderedText);
            var matcher = new FullLineAffixMatcher(corpusCase.Template);
            _ = recognizer.ComputeFrameFingerprint(frame);
            var watch = Stopwatch.StartNew();
            var result = recognizer.RecognizeAsync(frame, matcher).GetAwaiter().GetResult();
            watch.Stop();
            decisionTimes.Add(watch.Elapsed.TotalMilliseconds);

            var match = HasValidatedMatch(result, matcher);
            if (result.TargetAssistedMatch is not null)
            {
                assisted++;
            }

            if (result.WasCached)
            {
                cached++;
            }

            if (match is null)
            {
                failures.Add(
                    $"{corpusCase.Id}: {corpusCase.Template.Replace("\n", " / ", StringComparison.Ordinal)} => " +
                    string.Join(" | ", result.Lines));
            }
        }

        // Fixed layout rows must never satisfy a real PoEDB target by themselves. This also
        // catches accidental reintroduction of a distractor that happens to be a corpus affix.
        var distractorOnly = RenderSyntheticAffix(string.Empty);
        var uniqueCases = corpus.Cases
            .GroupBy(item => new FullLineAffixMatcher(item.Template).Template.Text, StringComparer.Ordinal)
            .Select(group => group.First())
            .ToArray();
        var distractorNegativeChecks = 0;
        foreach (var corpusCase in uniqueCases)
        {
            distractorNegativeChecks++;
            var matcher = new FullLineAffixMatcher(corpusCase.Template);
            _ = recognizer.ComputeFrameFingerprint(distractorOnly);
            var result = recognizer.RecognizeAsync(distractorOnly, matcher).GetAwaiter().GetResult();
            if (HasValidatedMatch(result, matcher) is not null)
            {
                falseAlerts.Add($"distractor-only => {corpusCase.Id}");
            }
        }

        // A compound target must not be reconstructed when any one of its authored physical
        // lines is absent. This protects the synthetic harness itself from hiding line loss.
        var missingLineChecks = 0;
        foreach (var corpusCase in uniqueCases)
        {
            var lines = corpusCase.Template
                .Replace("\r\n", "\n", StringComparison.Ordinal)
                .Split('\n');
            if (lines.Length <= 1)
            {
                continue;
            }

            var matcher = new FullLineAffixMatcher(corpusCase.Template);
            for (var omitted = 0; omitted < lines.Length; omitted++)
            {
                missingLineChecks++;
                var incomplete = string.Join(
                    '\n',
                    lines.Where((_, index) => index != omitted));
                var frame = RenderSyntheticAffix(MaterializeNumericSlots(incomplete));
                _ = recognizer.ComputeFrameFingerprint(frame);
                var result = recognizer.RecognizeAsync(frame, matcher).GetAwaiter().GetResult();
                if (HasValidatedMatch(result, matcher) is not null)
                {
                    falseAlerts.Add($"missing-line[{omitted}] {corpusCase.Id}");
                }
            }
        }

        var spatialBoundaryChecks = 0;
        foreach (var corpusCase in uniqueCases)
        {
            var lines = corpusCase.Template
                .Replace("\r\n", "\n", StringComparison.Ordinal)
                .Split('\n');
            if (lines.Length <= 1)
            {
                continue;
            }

            spatialBoundaryChecks++;
            var matcher = new FullLineAffixMatcher(corpusCase.Template);
            var separatedFrame = RenderSyntheticAffix(
                MaterializeNumericSlots(corpusCase.Template),
                extraGapAfterFirstTargetLine: 70);
            _ = recognizer.ComputeFrameFingerprint(separatedFrame);
            var result = recognizer.RecognizeAsync(separatedFrame, matcher).GetAwaiter().GetResult();
            if (HasValidatedMatch(result, matcher) is not null)
            {
                falseAlerts.Add($"spatial-boundary {corpusCase.Id}");
            }
        }

        // The target-conditioned recovery path is part of production behavior, so exercise both
        // directions of every labelled PoEDB near-neighbour pair. A primary line or an assisted
        // match for the wrong template is equally a false alert.
        var similarityNegativeChecks = 0;
        foreach (var group in corpus.Cases
                     .Where(item => !string.IsNullOrWhiteSpace(item.SimilarityGroup))
                     .GroupBy(item => item.SimilarityGroup!, StringComparer.Ordinal))
        {
            var cases = group.ToArray();
            foreach (var sourceCase in cases)
            {
                var frame = RenderSyntheticAffix(MaterializeNumericSlots(sourceCase.Template));
                var sourceCanonical = new FullLineAffixMatcher(sourceCase.Template).Template.Text;
                foreach (var targetCase in cases)
                {
                    var matcher = new FullLineAffixMatcher(targetCase.Template);
                    if (sourceCase.Id == targetCase.Id || matcher.Template.Text == sourceCanonical)
                    {
                        continue;
                    }

                    similarityNegativeChecks++;
                    _ = recognizer.ComputeFrameFingerprint(frame);
                    var result = recognizer.RecognizeAsync(frame, matcher).GetAwaiter().GetResult();
                    if (HasValidatedMatch(result, matcher) is not null)
                    {
                        falseAlerts.Add($"near-neighbour {sourceCase.Id} => {targetCase.Id}");
                    }
                }
            }
        }

        // Keep the 13,203-pair text claim reproducible inside the repository. Exact templates
        // reused across equipment domains are intentionally allowed; distinct canonical
        // templates must reject each other's concrete numeric rendering in both directions.
        var templatePairChecks = 0;
        var exactDuplicatePairs = 0;
        for (var left = 0; left < corpus.Cases.Count; left++)
        {
            var leftMatcher = new FullLineAffixMatcher(corpus.Cases[left].Template);
            for (var right = left + 1; right < corpus.Cases.Count; right++)
            {
                templatePairChecks++;
                var rightMatcher = new FullLineAffixMatcher(corpus.Cases[right].Template);
                if (leftMatcher.Template.Text == rightMatcher.Template.Text)
                {
                    exactDuplicatePairs++;
                    continue;
                }

                if (leftMatcher.IsMatch(MaterializeNumericSlots(corpus.Cases[right].Template)) ||
                    rightMatcher.IsMatch(MaterializeNumericSlots(corpus.Cases[left].Template)))
                {
                    falseAlerts.Add(
                        $"text pair {corpus.Cases[left].Id} <=> {corpus.Cases[right].Id}");
                }
            }
        }

        decisionTimes.Sort();
        Console.WriteLine(
            $"Positive summary: total={corpus.Cases.Count}, " +
            $"passed={corpus.Cases.Count - failures.Count}, failed={failures.Count}, " +
            $"direct={corpus.Cases.Count - failures.Count - assisted}, assisted={assisted}, " +
            $"cached={cached}, p50={Percentile(decisionTimes, 0.50):F1}ms, " +
            $"p95={Percentile(decisionTimes, 0.95):F1}ms, " +
            $"max={decisionTimes[^1]:F1}ms");
        Console.WriteLine(
            $"Negative summary: distractor={distractorNegativeChecks}, " +
            $"missingLine={missingLineChecks}, spatialBoundary={spatialBoundaryChecks}, " +
            $"similarityDirected={similarityNegativeChecks}, " +
            $"templatePairs={templatePairChecks}, exactDuplicatePairs={exactDuplicatePairs}, " +
            $"falseAlerts={falseAlerts.Count}");
        foreach (var failure in failures)
        {
            Console.WriteLine($"  FAILED: {failure}");
        }

        foreach (var falseAlert in falseAlerts)
        {
            Console.WriteLine($"  FALSE ALERT: {falseAlert}");
        }

        return failures.Count == 0 && falseAlerts.Count == 0 && cached == 0 ? 0 : 1;
    }

    private static LogicalAffixMatch? HasValidatedMatch(
        OcrRecognitionResult result,
        FullLineAffixMatcher matcher)
    {
        matcher.TryFindMatch(result.Lines, out var match);
        return match ?? (result.TargetAssistedMatch is { } targetAssisted &&
                         targetAssisted.CanonicalText == matcher.Template.Text
            ? targetAssisted
            : null);
    }

    private static CapturedFrame RenderSyntheticAffix(
        string text,
        double extraGapAfterFirstTargetLine = 0)
    {
        const int width = 1080;
        const int horizontalPadding = 30;
        const int verticalPadding = 18;
        const double fontSize = 25;
        const double lineHeight = 35;
        // Windows OCR is exercised the same way as the production combined tooltip mask: a
        // target line sits among unrelated affix rows instead of being an unnaturally tiny
        // one-line bitmap.
        var targetLines = text.Replace("\r\n", "\n", StringComparison.Ordinal).Split('\n');
        var lines = new[] { "測試干擾上行" }
            .Concat(targetLines)
            .Append("測試干擾下行")
            .ToArray();
        var height = checked(
            verticalPadding * 2 +
            Math.Max(1, lines.Length) * (int)lineHeight +
            (int)Math.Ceiling(extraGapAfterFirstTargetLine));
        var visual = new DrawingVisual();
        using (var drawing = visual.RenderOpen())
        {
            drawing.DrawRectangle(
                new SolidColorBrush(Color.FromRgb(8, 9, 12)),
                null,
                new Rect(0, 0, width, height));
            var typeface = new Typeface(
                new FontFamily("Microsoft JhengHei"),
                FontStyles.Normal,
                FontWeights.SemiBold,
                FontStretches.Normal);
            var brush = new SolidColorBrush(Color.FromRgb(150, 150, 255));
            for (var index = 0; index < lines.Length; index++)
            {
                var formatted = new FormattedText(
                    lines[index],
                    CultureInfo.GetCultureInfo("zh-TW"),
                    FlowDirection.LeftToRight,
                    typeface,
                    fontSize,
                    brush,
                    pixelsPerDip: 1.0);
                var left = Math.Max(horizontalPadding, (width - formatted.WidthIncludingTrailingWhitespace) / 2);
                var extraOffset = extraGapAfterFirstTargetLine > 0 && index >= 2
                    ? extraGapAfterFirstTargetLine
                    : 0;
                drawing.DrawText(
                    formatted,
                    new Point(left, verticalPadding + index * lineHeight + extraOffset));
            }
        }

        var bitmap = new RenderTargetBitmap(width, height, 96, 96, PixelFormats.Pbgra32);
        bitmap.Render(visual);
        var stride = checked(width * 4);
        var pixels = new byte[checked(stride * height)];
        bitmap.CopyPixels(pixels, stride, 0);
        return new CapturedFrame(width, height, stride, pixels, DateTimeOffset.UtcNow);
    }

    private static string MaterializeNumericSlots(string template)
    {
        ReadOnlySpan<string> values = ["13", "27", "8", "41"];
        var result = new System.Text.StringBuilder(template.Length + 16);
        var slot = 0;
        foreach (var character in template)
        {
            if (character == '#')
            {
                result.Append(values[slot % values.Length]);
                slot++;
            }
            else
            {
                result.Append(character);
            }
        }

        return result.ToString();
    }

    private static double Percentile(IReadOnlyList<double> sorted, double percentile)
    {
        if (sorted.Count == 0)
        {
            return 0;
        }

        var index = (int)Math.Ceiling((sorted.Count - 1) * percentile);
        return sorted[Math.Clamp(index, 0, sorted.Count - 1)];
    }

    private static void RenderHud(string outputPath, bool assertHud)
    {
        const int width = 420;
        const int height = 110;
        var hudType = typeof(LatchedAlertService).Assembly.GetType(
            "PoeAlarm.App.Monitoring.MonitoringHudWindow",
            throwOnError: true)!;
        var hud = (Window)(Activator.CreateInstance(hudType, nonPublic: true)
                           ?? throw new InvalidOperationException("Could not create monitoring HUD."));
        if (assertHud)
        {
            AssertMonitoringHud(hud, hudType);
        }

        var targetField = hudType.GetField("targetText", BindingFlags.Instance | BindingFlags.NonPublic)
                          ?? throw new InvalidOperationException("Monitoring HUD target text is missing.");
        var target = targetField.GetValue(hud) as System.Windows.Controls.TextBlock
                     ?? throw new InvalidOperationException("Monitoring HUD target is not a TextBlock.");
        target.Text = "(6-8)% increased Attack Speed if you've dealt a Critical Strike Recently";

        var content = (FrameworkElement)(hud.Content
                                         ?? throw new InvalidOperationException("Monitoring HUD has no content."));
        RenderElement(content, width, height, outputPath);
    }

    private static void AssertMonitoringHud(Window hud, Type hudType)
    {
        const long wsExTransparent = 0x00000020L;
        const long wsExToolWindow = 0x00000080L;
        const long wsExNoActivate = 0x08000000L;

        Assert(hud.Topmost, "Monitoring HUD must stay topmost.");
        Assert(!hud.ShowActivated, "Monitoring HUD must not activate itself.");
        Assert(!hud.ShowInTaskbar, "Monitoring HUD must stay out of the taskbar.");
        Assert(hud.WindowStyle == WindowStyle.None, "Monitoring HUD must be borderless.");
        Assert(!hud.IsHitTestVisible, "Monitoring HUD must be click-through in WPF.");

        var composeMethod = hudType.GetMethod(
            "ComposeHudExtendedStyle",
            BindingFlags.Static | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("HUD extended-style composer is missing.");
        var composed = (long)(composeMethod.Invoke(null, [0L])
                              ?? throw new InvalidOperationException("HUD style composer returned no value."));
        Assert((composed & wsExTransparent) != 0, "HUD must set WS_EX_TRANSPARENT.");
        Assert((composed & wsExNoActivate) != 0, "HUD must set WS_EX_NOACTIVATE.");
        Assert((composed & wsExToolWindow) != 0, "HUD must set WS_EX_TOOLWINDOW.");
    }

    private static async Task AssertMonitorWaitsIndefinitelyAsync()
    {
        await using var monitor = new AffixMonitor(new EmptyCapture(), new EmptyOcrRecognizer());
        monitor.Start("increased Attack Speed", new ScreenRegion(0, 0, 1, 1));

        // The former fault threshold was four seconds. Staying Running beyond it proves that
        // an absent tooltip no longer stops monitoring behind the user's back.
        await Task.Delay(TimeSpan.FromMilliseconds(4300));
        Assert(monitor.State == MonitorState.Running, "Empty OCR must keep waiting until manual stop.");

        await monitor.StopAsync();
        Assert(monitor.State == MonitorState.Idle, "Manual stop must still end monitoring.");
    }

    private static async Task AssertStopWinsPendingRecognitionAsync()
    {
        var recognizer = new ControlledOcrRecognizer();
        await using var monitor = new AffixMonitor(new EmptyCapture(), recognizer);
        var detected = false;
        monitor.AffixDetected += (_, _) => detected = true;
        monitor.Start("(6-8)% increased Attack Speed", new ScreenRegion(0, 0, 1, 1));

        await recognizer.Started.Task;
        var stopTask = monitor.StopAsync();
        recognizer.Complete("8(6-8)% INCREASED ATTACK SPEED");
        await stopTask;

        Assert(!detected, "A recognition completed after Stop began must not raise an alert.");
        Assert(monitor.State == MonitorState.Idle, "Stop/match race must finish Idle.");
    }

    private static void AssertBandFingerprintsIgnoreVerticalPosition()
    {
        var preprocessor = new PoeTextPreprocessor();
        var upper = preprocessor.PrepareBlueAffixBands(CreateSyntheticBlueBand(20));
        var lower = preprocessor.PrepareBlueAffixBands(CreateSyntheticBlueBand(55));
        var changed = preprocessor.PrepareBlueAffixBands(CreateSyntheticBlueBand(55, changed: true));

        Assert(upper.Count == 1 && lower.Count == 1 && changed.Count == 1,
            "Synthetic blue rows must each produce one prepared band.");
        Assert(upper[0].SourceTop != lower[0].SourceTop,
            "Synthetic rows must exercise different vertical positions.");
        Assert(upper[0].ContentFingerprint == lower[0].ContentFingerprint,
            "The same row pixels must keep their fingerprint after moving vertically.");
        Assert(lower[0].ContentFingerprint != changed[0].ContentFingerprint,
            "A changed row glyph must invalidate the band fingerprint.");

        var tall = preprocessor.PrepareBlueAffixBands(CreateSyntheticBlueBand(90, height: 120));
        var shorterAfterTall = preprocessor.PrepareBlueAffixBands(CreateSyntheticBlueBand(20, height: 50));
        Assert(tall.Count == 1 && shorterAfterTall.Count == 1,
            "Shrinking an ROI must not retain phantom rows from the reusable row buffer.");
        Assert(shorterAfterTall[0].SourceBottom < 50,
            "A prepared row after ROI shrink must remain inside the new frame bounds.");
    }

    private static async Task AssertTargetAwareMonitorAsync()
    {
        const string template = "增加 #% 暈眩恢復和格擋恢復";
        var recognizer = new AssistedOcrRecognizer();
        await using var monitor = new AffixMonitor(new EmptyCapture(), recognizer);
        var detected = new TaskCompletionSource<AffixDetectedEventArgs>(
            TaskCreationOptions.RunContinuationsAsynchronously);
        monitor.AffixDetected += (_, args) => detected.TrySetResult(args);
        monitor.Start(template, new ScreenRegion(0, 0, 1, 1));

        var result = await detected.Task.WaitAsync(TimeSpan.FromSeconds(2));
        Assert(recognizer.ReceivedTarget == template,
            "AffixMonitor must forward the compiled target to a target-aware recognizer.");
        Assert(result.Match.OriginalText.Contains("量眩", StringComparison.Ordinal),
            "A target-assisted detection must preserve the actual greedy OCR text.");
        Assert(monitor.State == MonitorState.MatchFound,
            "A validated target-assisted result must lock monitoring in MatchFound.");
    }

    private static async Task AssertConcurrentDisposeAsync()
    {
        var capture = new EmptyCapture();
        var recognizer = new ControlledOcrRecognizer();
        var monitor = new AffixMonitor(capture, recognizer);
        monitor.Start("increased Attack Speed", new ScreenRegion(0, 0, 1, 1));
        await recognizer.Started.Task;

        var firstDispose = monitor.DisposeAsync().AsTask();
        var secondDispose = monitor.DisposeAsync().AsTask();
        recognizer.Complete(string.Empty);
        await Task.WhenAll(firstDispose, secondDispose);
        await monitor.DisposeAsync();
        await monitor.StopAsync();

        Assert(capture.DisposeCount == 1,
            "Concurrent AffixMonitor disposal must release screen capture exactly once.");
        Assert(recognizer.DisposeCount == 1,
            "Concurrent AffixMonitor disposal must release OCR exactly once.");
    }

    private static async Task AssertProgressiveInputGuardAsync()
    {
        var recognizer = new ProgressiveFingerprintRecognizer(matches: false);
        await using (var monitor = new AffixMonitor(new EmptyCapture(), recognizer))
        {
            var requested = 0;
            var released = 0;
            var releaseObserved = new TaskCompletionSource(
                TaskCreationOptions.RunContinuationsAsynchronously);
            monitor.InputGuardRequested += (_, _) => Interlocked.Increment(ref requested);
            monitor.InputGuardReleased += (_, _) =>
            {
                Interlocked.Increment(ref released);
                releaseObserved.TrySetResult();
            };

            monitor.Start("increased Attack Speed", new ScreenRegion(0, 0, 1, 1));
            await releaseObserved.Task.WaitAsync(TimeSpan.FromSeconds(2));
            Assert(requested >= 2,
                "Every progressive slice must refresh the changed-frame input guard watchdog.");
            Assert(released == 1,
                "A completed non-match must release its changed-frame input guard exactly once.");
            await monitor.StopAsync();
        }

        var matchingRecognizer = new ProgressiveFingerprintRecognizer(matches: true);
        await using (var monitor = new AffixMonitor(new EmptyCapture(), matchingRecognizer))
        {
            var requested = 0;
            var released = 0;
            var detected = new TaskCompletionSource(
                TaskCreationOptions.RunContinuationsAsynchronously);
            monitor.InputGuardRequested += (_, _) => Interlocked.Increment(ref requested);
            monitor.InputGuardReleased += (_, _) => Interlocked.Increment(ref released);
            monitor.AffixDetected += (_, _) => detected.TrySetResult();

            monitor.Start("increased Attack Speed", new ScreenRegion(0, 0, 1, 1));
            await detected.Task.WaitAsync(TimeSpan.FromSeconds(2));
            await Task.Delay(25);
            Assert(requested == 1, "A changed matching frame must request the input guard.");
            Assert(released == 1,
                "A matching monitor must relinquish its guard request after subscribers accept the match.");
        }
    }

    private static void AssertPendingMouseInputGuardStateMachine()
    {
        var state = new MouseInputGuardStateMachine();

        // The down that produced the screenshot has already reached POE. Its matching up must
        // pass too; only the following click may be consumed.
        state.InitializeReleased(MouseButtonBits.Left);
        state.Arm();
        Assert(state.Mode == MouseInputGuardMode.WaitingForExistingRelease,
            "Arming with an existing physical down must wait for its paired up.");
        var currentUp = state.ProcessButton(MouseButtonBits.Left, isDown: false);
        Assert(!currentUp.Suppress && state.Mode == MouseInputGuardMode.Guarding,
            "The existing click's up must pass before the guard becomes active.");

        var nextDown = state.ProcessButton(MouseButtonBits.Left, isDown: true);
        Assert(nextDown.Suppress &&
               state.SuppressedButtons == MouseButtonBits.Left,
            "The first left down after the release boundary must be consumed.");

        Assert(!state.Release() && state.Mode == MouseInputGuardMode.Draining,
            "Release must drain an already consumed down instead of orphaning its up.");
        var pairedUp = state.ProcessButton(MouseButtonBits.Left, isDown: false);
        Assert(pairedUp.Suppress && pairedUp.BecameReleased &&
               state.Mode == MouseInputGuardMode.Released,
            "A consumed down and its up must be consumed as one pair before fail-open.");

        // A progressive watchdog refresh must not reopen an already active gate.
        state.InitializeReleased(physicalButtons: 0);
        state.Arm();
        state.Arm();
        Assert(state.Mode == MouseInputGuardMode.Guarding,
            "A progressive guard refresh must preserve the active blocking state.");

        // Buttons that overlap the screenshot-producing click are all allowed to finish before
        // guarding, while wheel input is represented by the mode and needs no pairing state.
        state.InitializeReleased(MouseButtonBits.Left | MouseButtonBits.Right);
        state.Arm();
        Assert(!state.ProcessButton(MouseButtonBits.Left, isDown: false).Suppress &&
               state.Mode == MouseInputGuardMode.WaitingForExistingRelease,
            "A second pre-existing button must keep the release gate open.");
        Assert(!state.ProcessButton(MouseButtonBits.Right, isDown: false).Suppress &&
               state.Mode == MouseInputGuardMode.Guarding,
            "The release gate must close exactly after all pre-existing buttons are up.");
    }

    private static void AssertPendingMouseInputGuardHookLifecycle()
    {
        using var guard = new PendingMouseInputGuard();
        Assert(guard.Prepare(), "The low-level pending mouse hook must install for monitoring.");
        Assert(guard.IsInstalled && guard.Mode == MouseInputGuardMode.Released,
            "A prepared low-level hook must remain pass-through.");
        Assert(guard.Arm() && guard.Mode is MouseInputGuardMode.Guarding or
            MouseInputGuardMode.WaitingForExistingRelease,
            "The installed low-level hook must arm synchronously.");
        guard.TransferToBlockingOverlay();

        Assert(guard.Prepare() && guard.IsInstalled &&
               guard.Mode == MouseInputGuardMode.Released,
            "An immediate acknowledge-then-restart must replace a hook already queued to exit.");
        guard.TransferToBlockingOverlay();

        var deadline = Stopwatch.StartNew();
        while (guard.IsInstalled && deadline.Elapsed < TimeSpan.FromSeconds(1))
        {
            Thread.Sleep(1);
        }

        Assert(!guard.IsInstalled && guard.Mode == MouseInputGuardMode.Released,
            "Transferring to a blocking overlay must fail open and uninstall the hook.");
    }

    private static void AssertInputGuardKeepsOverlayHidden()
    {
        using var service = new LatchedAlertService(Application.Current.Dispatcher);
        service.Prepare(preparePendingInputGuard: true);

        var overlayField = typeof(LatchedAlertService).GetField(
            "overlay",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Alert service overlay field is missing.");
        var overlay = overlayField.GetValue(service) as Window
                      ?? throw new InvalidOperationException(
                          "Preparing the alert service did not create its reusable overlay.");
        var overlayHandle = new System.Windows.Interop.WindowInteropHelper(overlay).Handle;
        var pendingGuardField = typeof(LatchedAlertService).GetField(
            "pendingMouseInputGuard",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Alert service pending hook field is missing.");
        var pendingGuard = pendingGuardField.GetValue(service) as PendingMouseInputGuard
                           ?? throw new InvalidOperationException(
                               "Alert service pending hook field has the wrong type.");
        Assert(overlayHandle != IntPtr.Zero, "Prepared alert overlay must own a reusable HWND.");
        Assert(!overlay.IsVisible && !IsWindowVisible(overlayHandle),
            "A prepared alert overlay must remain natively hidden.");
        Assert(pendingGuard.IsInstalled && pendingGuard.Mode == MouseInputGuardMode.Released,
            "Preparing Traditional Chinese protection must install a pass-through hook.");

        for (var refresh = 0; refresh < 5; refresh++)
        {
            service.BeginInputGuard(new ScreenRegion(0, 0, 1, 1));
            Assert(service.IsInputGuardActive,
                "The low-level changed-frame input guard must become logically active.");
            Assert(pendingGuard.IsInstalled && pendingGuard.Mode is
                    MouseInputGuardMode.Guarding or MouseInputGuardMode.WaitingForExistingRelease,
                "A logical pending guard must always have a synchronously armed native hook.");
            Assert(!overlay.IsVisible && !IsWindowVisible(overlayHandle),
                "Repeated pending OCR refreshes must not show a full-screen WPF window or " +
                "disturb POE hover state.");
        }

        for (var cycle = 0; cycle < 3; cycle++)
        {
            service.ReleaseInputGuard();
            Application.Current.Dispatcher.Invoke(
                () => { },
                DispatcherPriority.ApplicationIdle);
            Assert(!service.IsInputGuardActive,
                "Releasing a changed-frame input guard must clear its logical state.");
            Assert(!overlay.IsVisible && !IsWindowVisible(overlayHandle),
                "Releasing a hook-only input guard must leave the alert overlay hidden.");

            Task.Run(() => service.BeginInputGuard(new ScreenRegion(0, 0, 1, 1)))
                .GetAwaiter()
                .GetResult();
            // Drain every higher-priority dispatcher item. If a future worker-thread Begin queues
            // WPF presentation again, this assertion observes it instead of racing the queue.
            Application.Current.Dispatcher.Invoke(
                () => { },
                DispatcherPriority.ApplicationIdle);
            Assert(service.IsInputGuardActive,
                "A worker-thread guard request must become logically active.");
            Assert(pendingGuard.IsInstalled && pendingGuard.Mode is
                    MouseInputGuardMode.Guarding or MouseInputGuardMode.WaitingForExistingRelease,
                "A worker-thread guard request must synchronously arm the native hook.");
            Assert(!overlay.IsVisible && !IsWindowVisible(overlayHandle),
                "Worker-thread pending OCR must also keep the full-screen overlay natively hidden.");
        }

        service.ReleaseInputGuard();
    }

    private static void AssertAlertHandoffFailsOpenWhenDispatcherStalls()
    {
        using var service = new LatchedAlertService(Application.Current.Dispatcher);
        service.Prepare(preparePendingInputGuard: true);
        service.BeginInputGuard(new ScreenRegion(0, 0, 1, 1));

        var overlayField = typeof(LatchedAlertService).GetField(
            "overlay",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Alert service overlay field is missing.");
        var overlay = overlayField.GetValue(service) as Window
                      ?? throw new InvalidOperationException("Prepared alert overlay is missing.");
        var overlayHandle = new System.Windows.Interop.WindowInteropHelper(overlay).Handle;
        var pendingGuardField = typeof(LatchedAlertService).GetField(
            "pendingMouseInputGuard",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Alert service pending hook field is missing.");
        var pendingGuard = pendingGuardField.GetValue(service) as PendingMouseInputGuard
                           ?? throw new InvalidOperationException(
                               "Alert service pending hook field has the wrong type.");

        // Trigger from the monitor worker, then deliberately keep this UI thread occupied so the
        // queued red-window presentation cannot run before the independent hand-off watchdog.
        Task.Run(() => service.Trigger("target affix", new ScreenRegion(0, 0, 1, 1)))
            .GetAwaiter()
            .GetResult();
        Assert(service.IsActive, "A matched target must latch the alert before UI presentation.");
        var deadline = Stopwatch.StartNew();
        while (pendingGuard.Mode is MouseInputGuardMode.Guarding or
               MouseInputGuardMode.WaitingForExistingRelease &&
               deadline.Elapsed < TimeSpan.FromSeconds(2))
        {
            Thread.Sleep(10);
        }

        Assert(pendingGuard.Mode is MouseInputGuardMode.Released or MouseInputGuardMode.Draining,
            "A stalled Trigger-to-overlay hand-off must fail the system-wide hook open.");
        Assert(!overlay.IsVisible && !IsWindowVisible(overlayHandle),
            "The deliberately stalled red overlay must still be hidden before dispatcher drain.");

        // Invalidate the queued presentation before pumping the dispatcher, then prove that the
        // stale operation cannot resurrect the overlay.
        service.Acknowledge();
        Application.Current.Dispatcher.Invoke(
            () => { },
            DispatcherPriority.ApplicationIdle);
        Assert(!overlay.IsVisible && !IsWindowVisible(overlayHandle),
            "A stale queued alert presentation must remain hidden after acknowledgement.");
    }

    private static CapturedFrame CreateSyntheticBlueBand(int top, bool changed = false, int height = 90)
    {
        const int width = 120;
        const int stride = width * 4;
        var pixels = new byte[stride * height];
        for (var y = top; y < top + 7; y++)
        {
            for (var x = 20; x <= 82; x++)
            {
                var pixel = (y * stride) + (x * 4);
                pixels[pixel] = 220;
                pixels[pixel + 1] = 90;
                pixels[pixel + 2] = 90;
                pixels[pixel + 3] = 255;
            }
        }

        if (changed)
        {
            var pixel = ((top + 3) * stride) + (50 * 4);
            pixels[pixel] = 140;
        }

        return new CapturedFrame(width, height, stride, pixels, DateTimeOffset.UtcNow);
    }

    private static void RenderMainWindow(
        string outputPath,
        bool bottomMode,
        bool englishMode,
        bool poe2Mode,
        bool assertStartHotKey)
    {
        var closed = false;
        var snapshotDirectory = Path.Combine(
            Path.GetTempPath(),
            "PoeAlarm.UiSnapshot",
            Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(snapshotDirectory);
        var window = new MainWindow(new SettingsStore(Path.Combine(snapshotDirectory, "settings.json")))
        {
            Width = 570,
            Height = 680,
            Left = -10_000,
            Top = -10_000,
            ShowActivated = false,
            ShowInTaskbar = false,
            WindowStartupLocation = WindowStartupLocation.Manual,
        };
        window.Closed += (_, _) => closed = true;

        try
        {
            window.Show();
            window.Dispatcher.Invoke(static () => { }, DispatcherPriority.ApplicationIdle);
            if (englishMode)
            {
                ApplySnapshotUiLanguage(window, "en");
            }

            if (poe2Mode)
            {
                var gameProfile = window.FindName("GameProfileComboBox") as
                    System.Windows.Controls.ComboBox
                    ?? throw new InvalidOperationException(
                        "Main window game-profile selector was not found.");
                gameProfile.SelectedValue = "Poe2";
            }

            if (bottomMode)
            {
                var settings = window.FindName("ExperienceSettingsExpander") as System.Windows.Controls.Expander
                               ?? throw new InvalidOperationException(
                                   "Main window experience-settings expander was not found.");
                var scrollViewer = window.FindName("MainScrollViewer") as System.Windows.Controls.ScrollViewer
                                   ?? throw new InvalidOperationException(
                                       "Main window scroll viewer was not found.");
                settings.IsExpanded = true;
                window.UpdateLayout();
                scrollViewer.ScrollToEnd();
            }

            if (assertStartHotKey)
            {
                var startHotKey = window.FindName("StartHotKeyComboBox") as
                    System.Windows.Controls.ComboBox
                    ?? throw new InvalidOperationException(
                        "Main window start-hotkey selector was not found.");
                Assert(
                    string.Equals(
                        startHotKey.SelectedValue?.ToString(),
                        "Ctrl+Shift+F10",
                        StringComparison.Ordinal),
                    "The default global start shortcut must be Ctrl+Shift+F10.");
                var registrationField = typeof(MainWindow).GetField(
                    "_startMonitoringHotKeyRegistration",
                    BindingFlags.Instance | BindingFlags.NonPublic)
                    ?? throw new InvalidOperationException(
                        "Main window start-hotkey registration field was not found.");
                Assert(
                    registrationField.GetValue(window) is IDisposable,
                    "The default global start shortcut must own a live RegisterHotKey registration.");
                Console.WriteLine(
                    "Start hotkey assertion passed: Ctrl+Shift+F10 owns a live registration and is selected.");
            }

            window.Dispatcher.Invoke(static () => { }, DispatcherPriority.ApplicationIdle);
            RenderElement(window, 570, 680, outputPath);
        }
        finally
        {
            BeginMainWindowShutdown(window);
            // MainWindow uses an async-void cleanup handler and performs the one real Close after
            // settings/monitor teardown. Keep its dispatcher alive until that close completes;
            // returning from Main earlier can crash a hidden WPF HWND with CLR 0xe0434352.
            PumpDispatcherUntil(
                () => closed,
                TimeSpan.FromSeconds(10),
                "Main window did not finish asynchronous shutdown cleanup.");
            if (assertStartHotKey)
            {
                var registrationField = typeof(MainWindow).GetField(
                    "_startMonitoringHotKeyRegistration",
                    BindingFlags.Instance | BindingFlags.NonPublic)
                    ?? throw new InvalidOperationException(
                        "Main window start-hotkey registration field was not found after shutdown.");
                Assert(
                    registrationField.GetValue(window) is null,
                    "Closing the main window must dispose and clear the start-hotkey registration.");
                Console.WriteLine("Start hotkey shutdown assertion passed: registration released.");
            }
            Directory.Delete(snapshotDirectory, recursive: true);
        }
    }

    private static void RenderHelpWindow(string outputPath, bool englishMode)
    {
        var uiTextType = typeof(MainWindow).Assembly.GetType(
                             "PoeAlarm.App.Localization.UiText",
                             throwOnError: true)!;
        var useMethod = uiTextType.GetMethod(
                            "Use",
                            BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic)
                        ?? throw new InvalidOperationException("UI language catalog lookup was not found.");
        _ = useMethod.Invoke(null, [englishMode ? "en" : "zh-CN"]);

        var window = new HelpWindow
        {
            Left = -10_000,
            Top = -10_000,
            ShowActivated = false,
            ShowInTaskbar = false,
            WindowStartupLocation = WindowStartupLocation.Manual,
        };
        try
        {
            window.Show();
            window.Dispatcher.Invoke(static () => { }, DispatcherPriority.ApplicationIdle);
            RenderElement(window, 560, 700, outputPath);
        }
        finally
        {
            window.Close();
        }
    }

    private static void BeginMainWindowShutdown(MainWindow window)
    {
        var closingHandler = typeof(MainWindow).GetMethod(
                                 "OnClosing",
                                 BindingFlags.Instance | BindingFlags.NonPublic,
                                 binder: null,
                                 types:
                                 [
                                     typeof(object),
                                     typeof(System.ComponentModel.CancelEventArgs),
                                 ],
                                 modifiers: null)
                             ?? throw new InvalidOperationException(
                                 "Main window shutdown handler was not found.");

        // Do not call Window.Close here. The product's first Closing pass cancels that close while
        // it performs async cleanup, then calls Close again. If every await happens to complete
        // synchronously (common in this empty test state), that second call is re-entrant and WPF
        // throws "Cannot ... Close ... while a Window is closing." Invoking the cleanup handler
        // outside WPF's closing stack preserves its production teardown and lets its final Close
        // be the sole native close operation.
        try
        {
            closingHandler.Invoke(
                window,
                [window, new System.ComponentModel.CancelEventArgs()]);
        }
        catch (TargetInvocationException exception) when (exception.InnerException is not null)
        {
            throw exception.InnerException;
        }
    }

    private static void ApplySnapshotUiLanguage(MainWindow window, string languageCode)
    {
        var isLoadedField = typeof(MainWindow).GetField(
                                "_isLoaded",
                                BindingFlags.Instance | BindingFlags.NonPublic)
                            ?? throw new InvalidOperationException(
                                "Main window loaded-state field was not found.");
        var comboBox = window.FindName("UiLanguageComboBox") as System.Windows.Controls.ComboBox
                       ?? throw new InvalidOperationException(
                           "Main window UI-language selector was not found.");
        var uiTextType = typeof(MainWindow).Assembly.GetType(
                             "PoeAlarm.App.Localization.UiText",
                             throwOnError: true)!;
        var useMethod = uiTextType.GetMethod(
                            "Use",
                            BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic)
                        ?? throw new InvalidOperationException("UI language catalog lookup was not found.");
        var applyMethod = typeof(MainWindow).GetMethod(
                              "ApplyUiStrings",
                              BindingFlags.Instance | BindingFlags.NonPublic)
                          ?? throw new InvalidOperationException(
                              "Main window UI language application method was not found.");

        // Suppress the production selection-change handler so a visual snapshot cannot persist
        // its forced language into the user's real settings file.
        var wasLoaded = (bool)(isLoadedField.GetValue(window) ?? false);
        isLoadedField.SetValue(window, false);
        try
        {
            comboBox.SelectedValue = languageCode;
            var strings = useMethod.Invoke(null, [languageCode])
                          ?? throw new InvalidOperationException("UI language catalog was empty.");
            applyMethod.Invoke(window, [strings]);
        }
        finally
        {
            isLoadedField.SetValue(window, wasLoaded);
        }
    }

    private static void RenderAlert(
        string outputPath,
        bool assertBlocking,
        bool assertLiveBlocking)
    {
        // Arm initializes the reusable native window at the full virtual-desktop bounds. Render
        // that same surface size so high-resolution desktops do not crop the centered card into
        // the lower-right corner of a smaller synthetic bitmap.
        var width = Math.Max(1280, (int)Math.Ceiling(SystemParameters.VirtualScreenWidth));
        var height = Math.Max(720, (int)Math.Ceiling(SystemParameters.VirtualScreenHeight));
        var overlayType = typeof(LatchedAlertService).Assembly.GetType(
            "PoeAlarm.App.Alerts.AffixHitOverlayWindow",
            throwOnError: true)!;
        var overlay = (Window)(Activator.CreateInstance(overlayType, nonPublic: true)
                               ?? throw new InvalidOperationException("Could not create the alert overlay."));
        if (assertBlocking)
        {
            AssertBlockingOverlay(overlay, overlayType);
        }

        if (assertLiveBlocking)
        {
            AssertLiveReusableShield(overlay, overlayType);
        }

        overlayType.GetMethod("Arm", BindingFlags.Instance | BindingFlags.Public)!
            .Invoke(overlay,
            [
                "8(6-8)% INCREASED ATTACK SPEED IF YOU'VE DEALT A CRITICAL STRIKE RECENTLY",
                999,
                null,
            ]);

        var content = (FrameworkElement)(overlay.Content
                                         ?? throw new InvalidOperationException("Alert overlay has no content."));
        RenderElement(content, width, height, outputPath);
        overlayType.GetMethod("CloseFromService", BindingFlags.Instance | BindingFlags.Public)!
            .Invoke(overlay, null);
    }

    private static void AssertBlockingOverlay(Window overlay, Type overlayType)
    {
        const long wsExTransparent = 0x00000020L;
        const long wsExToolWindow = 0x00000080L;
        const long wsExNoActivate = 0x08000000L;

        Assert(overlay.Topmost, "Alert overlay must stay topmost.");
        Assert(overlay.ShowActivated, "Alert overlay must request foreground activation.");
        Assert(!overlay.ShowInTaskbar, "Alert overlay must remain outside Alt+Tab/taskbar.");
        Assert(overlay.WindowStyle == WindowStyle.None, "Alert overlay must be borderless.");
        Assert(!overlay.IsHitTestVisible,
            "A newly constructed reusable overlay must start released from mouse hit testing.");
        Assert(HasNonZeroAlpha(overlay.Background), "Window background must have non-zero alpha.");

        var root = overlay.Content as System.Windows.Controls.Grid
                   ?? throw new InvalidOperationException("Alert overlay root must be a Grid.");
        Assert(HasNonZeroAlpha(root.Background), "Root input shield must have non-zero alpha.");

        var buttonField = overlayType.GetField(
            "acknowledgementButton",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Alert overlay has no acknowledgement button.");
        var button = buttonField.GetValue(overlay) as System.Windows.Controls.Button
                     ?? throw new InvalidOperationException("Acknowledgement field is not a Button.");
        Assert(button.IsEnabled, "Acknowledgement button must be available immediately.");
        Assert(
            button.Content?.ToString()?.Contains("检查", StringComparison.Ordinal) == true,
            "Acknowledgement button must require an explicit inspection confirmation.");

        var composeMethod = overlayType.GetMethod(
            "ComposeBlockingExtendedStyle",
            BindingFlags.Static | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Blocking extended-style composer is missing.");
        var composed = (long)(composeMethod.Invoke(
            null,
            [wsExTransparent | wsExNoActivate])
            ?? throw new InvalidOperationException("Style composer returned no value."));
        Assert((composed & wsExTransparent) == 0, "WS_EX_TRANSPARENT must be cleared.");
        Assert((composed & wsExNoActivate) == 0, "WS_EX_NOACTIVATE must be cleared.");
        Assert((composed & wsExToolWindow) != 0, "WS_EX_TOOLWINDOW must remain enabled.");

        var composeReleasedMethod = overlayType.GetMethod(
            "ComposeReleasedExtendedStyle",
            BindingFlags.Static | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Released extended-style composer is missing.");
        var releasedComposition = (long)(composeReleasedMethod.Invoke(null, [composed])
            ?? throw new InvalidOperationException("Released style composer returned no value."));
        Assert((releasedComposition & wsExTransparent) != 0,
            "A released shield must set WS_EX_TRANSPARENT.");
        Assert((releasedComposition & wsExNoActivate) != 0,
            "A released shield must set WS_EX_NOACTIVATE.");
        Assert((releasedComposition & wsExToolWindow) != 0,
            "A released shield must remain a tool window.");

        AssertReusableShieldLifecycle(
            overlay,
            overlayType,
            wsExTransparent,
            wsExNoActivate,
            wsExToolWindow);
    }

    private static void AssertReusableShieldLifecycle(
        Window overlay,
        Type overlayType,
        long wsExTransparent,
        long wsExNoActivate,
        long wsExToolWindow)
    {
        const int gwlExStyle = -20;
        var prepare = overlayType.GetMethod(
            "PrepareForReuse",
            BindingFlags.Instance | BindingFlags.Public)
            ?? throw new InvalidOperationException("Reusable shield prepare method is missing.");
        var arm = overlayType.GetMethod("Arm", BindingFlags.Instance | BindingFlags.Public)
                  ?? throw new InvalidOperationException("Reusable shield arm method is missing.");
        var release = overlayType.GetMethod(
            "ReleaseForReuse",
            BindingFlags.Instance | BindingFlags.Public)
            ?? throw new InvalidOperationException("Reusable shield release method is missing.");
        var handleField = overlayType.GetField("handle", BindingFlags.Instance | BindingFlags.NonPublic)
                          ?? throw new InvalidOperationException("Reusable shield handle is missing.");
        var alertVisualLayerField = overlayType.GetField(
            "alertVisualLayer",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Alert visual layer is missing.");
        var alertVisualLayer = alertVisualLayerField.GetValue(overlay) as FrameworkElement
                               ?? throw new InvalidOperationException(
                                   "Alert visual layer is not a FrameworkElement.");
        var getWindowLong = overlayType.GetMethod(
            "GetWindowLongPtr",
            BindingFlags.Static | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Native style reader is missing.");

        prepare.Invoke(overlay, null);
        var preparedHandle = (IntPtr)(handleField.GetValue(overlay) ?? IntPtr.Zero);
        Assert(preparedHandle != IntPtr.Zero, "Prepare must eagerly create the reusable HWND.");
        Assert(!overlay.IsVisible, "A prepared shield must remain hidden.");
        Assert(!IsWindowVisible(preparedHandle), "A prepared shield HWND must remain natively hidden.");
        Assert(!overlay.IsHitTestVisible, "A prepared shield must be released in WPF hit testing.");
        Assert(alertVisualLayer.Visibility == Visibility.Collapsed,
            "A prepared reusable shield must not retain stale red alert pixels.");
        AssertReleasedNativeStyle(ReadStyle(getWindowLong, preparedHandle, gwlExStyle));

        arm.Invoke(overlay, ["target affix", 1, null]);
        Assert(!overlay.IsVisible, "Arming must not expose the window before Present.");
        Assert(alertVisualLayer.Visibility == Visibility.Visible,
            "A real target hit must restore the complete red alert visual layer.");
        Assert(overlay.IsHitTestVisible, "Arming must enable WPF hit testing.");
        var armedStyle = ReadStyle(getWindowLong, preparedHandle, gwlExStyle);
        Assert((armedStyle & wsExTransparent) == 0,
            "Arming must clear WS_EX_TRANSPARENT before presentation.");
        Assert((armedStyle & wsExNoActivate) == 0,
            "Arming must clear WS_EX_NOACTIVATE before presentation.");
        Assert((armedStyle & wsExToolWindow) != 0,
            "An armed shield must remain a tool window.");

        release.Invoke(overlay, null);
        Assert(!overlay.IsVisible, "A released reusable shield must be hidden.");
        Assert(!IsWindowVisible(preparedHandle), "A released reusable shield must be natively hidden.");
        Assert(!overlay.IsHitTestVisible, "A hidden reusable shield must not hit-test in WPF.");
        AssertReleasedNativeStyle(ReadStyle(getWindowLong, preparedHandle, gwlExStyle));

        arm.Invoke(overlay, ["second target", 2, null]);
        release.Invoke(overlay, null);
        Assert((IntPtr)(handleField.GetValue(overlay) ?? IntPtr.Zero) == preparedHandle,
            "Repeated alerts must reuse the prepared HWND.");
        AssertReleasedNativeStyle(ReadStyle(getWindowLong, preparedHandle, gwlExStyle));

        void AssertReleasedNativeStyle(long style)
        {
            Assert((style & wsExTransparent) != 0,
                "A hidden reusable shield must be natively input-transparent.");
            Assert((style & wsExNoActivate) != 0,
                "A hidden reusable shield must not activate itself.");
            Assert((style & wsExToolWindow) != 0,
                "A hidden reusable shield must remain outside task switching.");
        }

        static long ReadStyle(MethodInfo getter, IntPtr windowHandle, int index) =>
            ((IntPtr)(getter.Invoke(null, [windowHandle, index]) ?? IntPtr.Zero)).ToInt64();
    }

    private static void AssertLiveReusableShield(Window overlay, Type overlayType)
    {
        var prepare = overlayType.GetMethod(
            "PrepareForReuse",
            BindingFlags.Instance | BindingFlags.Public)
            ?? throw new InvalidOperationException("Reusable shield prepare method is missing.");
        var arm = overlayType.GetMethod("Arm", BindingFlags.Instance | BindingFlags.Public)
                  ?? throw new InvalidOperationException("Reusable shield arm method is missing.");
        var present = overlayType.GetMethod("Present", BindingFlags.Instance | BindingFlags.Public)
                      ?? throw new InvalidOperationException("Reusable shield present method is missing.");
        var release = overlayType.GetMethod(
            "ReleaseForReuse",
            BindingFlags.Instance | BindingFlags.Public)
            ?? throw new InvalidOperationException("Reusable shield release method is missing.");
        var handleField = overlayType.GetField("handle", BindingFlags.Instance | BindingFlags.NonPublic)
                          ?? throw new InvalidOperationException("Reusable shield handle is missing.");
        var anyMouseButtonPressed = overlayType.GetMethod(
            "AnyMouseButtonPressed",
            BindingFlags.Static | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Physical mouse-state reader is missing.");

        var prepareWatch = Stopwatch.StartNew();
        prepare.Invoke(overlay, null);
        prepareWatch.Stop();
        var preparedHandle = (IntPtr)(handleField.GetValue(overlay) ?? IntPtr.Zero);
        var presentationTimes = new List<double>(capacity: 2);
        for (var generation = 1; generation <= 2; generation++)
        {
            WaitForMouseButtonsReleased(anyMouseButtonPressed);
            arm.Invoke(overlay, [$"target {generation}", generation, null]);
            var watch = Stopwatch.StartNew();
            try
            {
                present.Invoke(overlay, null);
                watch.Stop();
                presentationTimes.Add(watch.Elapsed.TotalMilliseconds);
                Assert(overlay.IsVisible, "Present must make the native shield visible.");
                Assert(IsWindowVisible(preparedHandle),
                    "Present must synchronously make the prepared HWND visible.");
                Assert(overlay.IsHitTestVisible, "A presented shield must receive WPF mouse input.");
                Assert((IntPtr)(handleField.GetValue(overlay) ?? IntPtr.Zero) == preparedHandle,
                    "Present must retain the eagerly prepared HWND.");
                if (generation == 1)
                {
                    AssertCaptureAffinityLifecycle(overlay, overlayType, preparedHandle);
                    AssertAcknowledgementCommitLifecycle(overlay, overlayType);
                }
            }
            finally
            {
                release.Invoke(overlay, null);
            }

            Assert(!overlay.IsVisible, "Release must synchronously hide a presented shield.");
            Assert(!IsWindowVisible(preparedHandle),
                "Release must synchronously hide the native shield HWND.");
            Assert(!overlay.IsHitTestVisible,
                "Release must synchronously remove WPF mouse hit testing.");
        }

        Console.WriteLine(
            $"Reusable shield: prepare {prepareWatch.Elapsed.TotalMilliseconds:F2} ms, " +
            $"first {presentationTimes[0]:F2} ms, " +
            $"warm {presentationTimes[1]:F2} ms");
    }

    private static void AssertAcknowledgementCommitLifecycle(Window overlay, Type overlayType)
    {
        var buttonField = overlayType.GetField(
            "acknowledgementButton",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Alert overlay has no acknowledgement button.");
        var button = buttonField.GetValue(overlay) as System.Windows.Controls.Button
                     ?? throw new InvalidOperationException(
                         "Acknowledgement field is not a Button.");
        var click = overlayType.GetMethod(
            "OnAcknowledgementClick",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Acknowledgement click handler is missing.");
        var commitTick = overlayType.GetMethod(
            "OnAcknowledgementCommitDelayElapsed",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException("Acknowledgement commit handler is missing.");

        var acknowledgementRaised = false;
        EventHandler<OverlayAcknowledgementRequestedEventArgs> handler = (_, _) =>
            acknowledgementRaised = true;
        ((AffixHitOverlayWindow)overlay).AcknowledgeRequested += handler;
        try
        {
            Assert(button.IsEnabled,
                "Arm and Present must make acknowledgement available immediately.");

            var watch = Stopwatch.StartNew();
            click.Invoke(overlay, [button, new RoutedEventArgs(System.Windows.Controls.Button.ClickEvent)]);
            Assert(!button.IsEnabled,
                "The acknowledgement button must disable while its click tail is absorbed.");
            Assert(!acknowledgementRaised,
                "Acknowledgement must not commit synchronously with the button click.");

            // Invoke the timer callback before its deadline instead of sleeping. This tests the
            // same deadline guard deterministically even on machines with a coarse or heavily
            // delayed Windows timer quantum; the callback must simply re-arm its dispatcher timer.
            commitTick.Invoke(overlay, [null, EventArgs.Empty]);
            Assert(watch.Elapsed < TimeSpan.FromMilliseconds(300),
                "The pre-commit assertion must run before the 300 ms tail-absorption period.");
            Assert(!acknowledgementRaised,
                "Acknowledgement must not commit before the 300 ms tail-absorption period.");
            Assert(overlay.IsVisible && IsWindowVisible(new System.Windows.Interop.WindowInteropHelper(overlay).Handle),
                "The native shield must remain visible during acknowledgement tail absorption.");
            Assert(overlay.IsHitTestVisible,
                "The shield must continue receiving mouse input during acknowledgement tail absorption.");

            PumpDispatcherUntil(
                () => acknowledgementRaised,
                TimeSpan.FromSeconds(2),
                "Acknowledgement did not commit after the 300 ms tail-absorption period.");
            Assert(watch.Elapsed >= TimeSpan.FromMilliseconds(300),
                "Acknowledgement committed before the full 300 ms tail-absorption period.");
        }
        finally
        {
            ((AffixHitOverlayWindow)overlay).AcknowledgeRequested -= handler;
        }
    }

    private static void AssertCaptureAffinityLifecycle(
        Window overlay,
        Type overlayType,
        IntPtr windowHandle)
    {
        const uint wdaNone = 0x00000000;
        const uint wdaExcludeFromCapture = 0x00000011;
        var setExcludeFromCapture = overlayType.GetMethod(
            "SetExcludeFromCapture",
            BindingFlags.Instance | BindingFlags.Public)
            ?? throw new InvalidOperationException("Capture-affinity setter is missing.");

        AssertDisplayAffinity(windowHandle, wdaNone,
            "A default alert overlay must be visible in screen recordings (WDA_NONE).");
        setExcludeFromCapture.Invoke(overlay, [true]);
        AssertDisplayAffinity(windowHandle, wdaExcludeFromCapture,
            "Opting out of overlay capture must apply WDA_EXCLUDEFROMCAPTURE.");
        setExcludeFromCapture.Invoke(overlay, [false]);
        AssertDisplayAffinity(windowHandle, wdaNone,
            "Re-enabling overlay capture must restore WDA_NONE.");
    }

    private static void AssertDisplayAffinity(IntPtr windowHandle, uint expected, string message)
    {
        Assert(GetWindowDisplayAffinity(windowHandle, out var actual),
            $"GetWindowDisplayAffinity failed with Win32 error {Marshal.GetLastWin32Error()}.");
        Assert(actual == expected, $"{message} Expected 0x{expected:X8}, received 0x{actual:X8}.");
    }

    private static void WaitForMouseButtonsReleased(MethodInfo anyMouseButtonPressed)
    {
        PumpDispatcherUntil(
            () => !(bool)(anyMouseButtonPressed.Invoke(null, null) ?? true),
            TimeSpan.FromSeconds(2),
            "Release all mouse buttons before running the live alert assertion.");
    }

    private static void PumpDispatcherUntil(
        Func<bool> condition,
        TimeSpan timeout,
        string timeoutMessage)
    {
        var watch = Stopwatch.StartNew();
        while (!condition())
        {
            Assert(watch.Elapsed < timeout, timeoutMessage);
            PumpDispatcherOnce(TimeSpan.FromMilliseconds(5));
        }
    }

    private static void PumpDispatcherOnce(TimeSpan maximumWait)
    {
        var frame = new DispatcherFrame();
        var timer = new DispatcherTimer(DispatcherPriority.Background)
        {
            Interval = maximumWait,
        };
        timer.Tick += (_, _) =>
        {
            timer.Stop();
            frame.Continue = false;
        };
        timer.Start();
        Dispatcher.PushFrame(frame);
    }

    private static bool HasNonZeroAlpha(Brush? brush) =>
        brush is SolidColorBrush solid && solid.Color.A > 0;

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool IsWindowVisible(IntPtr windowHandle);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetWindowDisplayAffinity(
        IntPtr windowHandle,
        out uint affinity);

    private static void Assert(bool condition, string message)
    {
        if (!condition)
        {
            throw new InvalidOperationException(message);
        }
    }

    private sealed class EmptyCapture : IScreenCapture
    {
        private static readonly CapturedFrame Frame = new(
            1,
            1,
            4,
            [0, 0, 0, 255],
            DateTimeOffset.UtcNow);

        public CapturedFrame Capture(ScreenRegion region) => Frame;

        public int DisposeCount { get; private set; }

        public void Dispose()
        {
            DisposeCount++;
        }
    }

    private sealed class EmptyOcrRecognizer : IOcrRecognizer
    {
        public Task<OcrRecognitionResult> RecognizeAsync(
            CapturedFrame frame,
            CancellationToken cancellationToken = default) =>
            Task.FromResult(new OcrRecognitionResult([], TimeSpan.Zero, TimeSpan.Zero, WasCached: true));

        public void Dispose()
        {
        }
    }

    private sealed class ControlledOcrRecognizer : IOcrRecognizer
    {
        private readonly TaskCompletionSource<OcrRecognitionResult> completion =
            new(TaskCreationOptions.RunContinuationsAsynchronously);

        public TaskCompletionSource Started { get; } =
            new(TaskCreationOptions.RunContinuationsAsynchronously);

        public int DisposeCount { get; private set; }

        public Task<OcrRecognitionResult> RecognizeAsync(
            CapturedFrame frame,
            CancellationToken cancellationToken = default)
        {
            Started.TrySetResult();
            return completion.Task;
        }

        public void Complete(string line) => completion.TrySetResult(
            new OcrRecognitionResult([line], TimeSpan.Zero, TimeSpan.Zero));

        public void Dispose()
        {
            DisposeCount++;
        }
    }

    private sealed class AssistedOcrRecognizer : IOcrRecognizer
    {
        public string? ReceivedTarget { get; private set; }

        public Task<OcrRecognitionResult> RecognizeAsync(
            CapturedFrame frame,
            CancellationToken cancellationToken = default) =>
            Task.FromResult(new OcrRecognitionResult(
                ["增加24%量眩恢復和格擋恢復"],
                TimeSpan.Zero,
                TimeSpan.Zero));

        public Task<OcrRecognitionResult> RecognizeAsync(
            CapturedFrame frame,
            FullLineAffixMatcher target,
            CancellationToken cancellationToken = default)
        {
            ReceivedTarget = target.SourceTemplate;
            return Task.FromResult(new OcrRecognitionResult(
                ["增加24%量眩恢復和格擋恢復"],
                TimeSpan.Zero,
                TimeSpan.Zero,
                TargetAssistedMatch: new LogicalAffixMatch(
                    0,
                    1,
                    "增加24%量眩恢復和格擋恢復",
                    target.Template.Text)));
        }

        public void Dispose()
        {
        }
    }

    private sealed class ProgressiveFingerprintRecognizer(bool matches)
        : IOcrRecognizer, IFrameFingerprintProvider
    {
        private int calls;

        public ulong ComputeFrameFingerprint(CapturedFrame frame) => 42;

        public Task<OcrRecognitionResult> RecognizeAsync(
            CapturedFrame frame,
            CancellationToken cancellationToken = default)
        {
            var call = Interlocked.Increment(ref calls);
            if (!matches && call == 1)
            {
                return Task.FromResult(new OcrRecognitionResult(
                    [],
                    TimeSpan.Zero,
                    TimeSpan.Zero,
                    RequiresRescan: true));
            }

            return Task.FromResult(new OcrRecognitionResult(
                matches ? ["increased Attack Speed"] : [],
                TimeSpan.Zero,
                TimeSpan.Zero));
        }

        public void Dispose()
        {
        }
    }

    private sealed record PoedbCorpus(IReadOnlyList<PoedbCorpusCase> Cases);

    private sealed record PoedbCorpusCase(
        string Id,
        string Template,
        string? SimilarityGroup);

    private static void RenderElement(FrameworkElement element, int width, int height, string outputPath)
    {
        var renderSize = new Size(width, height);
        element.Measure(renderSize);
        element.Arrange(new Rect(renderSize));
        element.UpdateLayout();

        var bitmap = new RenderTargetBitmap(width, height, 96, 96, PixelFormats.Pbgra32);
        bitmap.Render(element);

        var encoder = new PngBitmapEncoder();
        encoder.Frames.Add(BitmapFrame.Create(bitmap));
        using var stream = File.Create(outputPath);
        encoder.Save(stream);
    }
}
