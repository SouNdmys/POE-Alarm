using System.IO;
using System.Reflection;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using PoeAlarm.App;
using PoeAlarm.App.Alerts;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Monitoring;
using PoeAlarm.App.Recognition;

internal static class Program
{
    [STAThread]
    public static int Main(string[] args)
    {
        var alertMode = args.Contains("--alert", StringComparer.OrdinalIgnoreCase);
        var assertBlocking = args.Contains("--assert-blocking", StringComparer.OrdinalIgnoreCase);
        var assertMonitorWait = args.Contains("--assert-monitor-wait", StringComparer.OrdinalIgnoreCase);
        var assertStopMatchRace = args.Contains("--assert-stop-match-race", StringComparer.OrdinalIgnoreCase);
        var hudMode = args.Contains("--hud", StringComparer.OrdinalIgnoreCase);
        var assertHud = args.Contains("--assert-hud", StringComparer.OrdinalIgnoreCase);
        var pathArgument = args.FirstOrDefault(argument => !argument.StartsWith("--", StringComparison.Ordinal));
        var outputPath = pathArgument is not null
            ? Path.GetFullPath(pathArgument)
            : Path.GetFullPath(Path.Combine(
                "artifacts",
                alertMode ? "ui-alert.png" : hudMode ? "ui-hud.png" : "ui-main.png"));

        var outputDirectory = Path.GetDirectoryName(outputPath)
                              ?? throw new InvalidOperationException("Snapshot path has no parent directory.");
        Directory.CreateDirectory(outputDirectory);

        _ = new Application { ShutdownMode = ShutdownMode.OnExplicitShutdown };
        if (assertMonitorWait)
        {
            AssertMonitorWaitsIndefinitelyAsync().GetAwaiter().GetResult();
        }

        if (assertStopMatchRace)
        {
            AssertStopWinsPendingRecognitionAsync().GetAwaiter().GetResult();
        }

        if (hudMode)
        {
            RenderHud(outputPath, assertHud);
        }
        else if (alertMode)
        {
            RenderAlert(outputPath, assertBlocking);
        }
        else
        {
            RenderMainWindow(outputPath);
        }

        Console.WriteLine(outputPath);
        return 0;
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

    private static void RenderMainWindow(string outputPath)
    {
        var window = new MainWindow
        {
            Width = 860,
            Height = 820,
            Left = -10_000,
            Top = -10_000,
            ShowActivated = false,
            ShowInTaskbar = false,
            WindowStartupLocation = WindowStartupLocation.Manual,
        };

        window.Show();
        window.Dispatcher.Invoke(static () => { }, DispatcherPriority.ApplicationIdle);
        RenderElement(window, 860, 820, outputPath);
        window.Close();
    }

    private static void RenderAlert(string outputPath, bool assertBlocking)
    {
        const int width = 1280;
        const int height = 720;
        var overlayType = typeof(LatchedAlertService).Assembly.GetType(
            "PoeAlarm.App.Alerts.AffixHitOverlayWindow",
            throwOnError: true)!;
        var overlay = (Window)(Activator.CreateInstance(overlayType, nonPublic: true)
                               ?? throw new InvalidOperationException("Could not create the alert overlay."));
        if (assertBlocking)
        {
            AssertBlockingOverlay(overlay, overlayType);
        }

        overlayType.GetMethod("SetDetectedText", BindingFlags.Instance | BindingFlags.Public)!
            .Invoke(overlay, ["8(6-8)% INCREASED ATTACK SPEED IF YOU'VE DEALT A CRITICAL STRIKE RECENTLY"]);

        var content = (FrameworkElement)(overlay.Content
                                         ?? throw new InvalidOperationException("Alert overlay has no content."));
        RenderElement(content, width, height, outputPath);
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
        Assert(overlay.IsHitTestVisible, "Alert overlay must participate in mouse hit testing.");
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
        Assert(!button.IsEnabled, "Acknowledgement button must start disabled against carry-over clicks.");
        Assert(
            button.Content?.ToString()?.Contains("停手", StringComparison.Ordinal) == true,
            "Acknowledgement button must require a quiet period before confirmation.");

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
    }

    private static bool HasNonZeroAlpha(Brush? brush) =>
        brush is SolidColorBrush solid && solid.Color.A > 0;

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

        public void Dispose()
        {
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
        }
    }

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
