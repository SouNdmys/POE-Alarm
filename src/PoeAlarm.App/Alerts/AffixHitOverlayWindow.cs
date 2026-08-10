using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Threading;
using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Alerts;

internal sealed class AffixHitOverlayWindow : Window
{
    private static readonly TimeSpan AcknowledgementQuietPeriod = TimeSpan.FromMilliseconds(1200);
    private static readonly TimeSpan AcknowledgementCommitPeriod = TimeSpan.FromMilliseconds(300);

    private const int GwlExStyle = -20;
    private const long WsExTransparent = 0x00000020L;
    private const long WsExToolWindow = 0x00000080L;
    private const long WsExNoActivate = 0x08000000L;

    private const int WmMouseActivate = 0x0021;
    private const int WmDisplayChange = 0x007E;
    private const int WmDpiChanged = 0x02E0;
    private const int MaActivateAndEat = 2;

    private const int SmXVirtualScreen = 76;
    private const int SmYVirtualScreen = 77;
    private const int SmCxVirtualScreen = 78;
    private const int SmCyVirtualScreen = 79;
    private const uint MonitorDefaultToPrimary = 1;

    private const uint SwpNoActivate = 0x0010;
    private const uint SwpShowWindow = 0x0040;

    private static readonly IntPtr HwndTopmost = new(-1);

    private readonly TextBlock detectedTextBlock;
    private readonly Button acknowledgementButton;
    private readonly DispatcherTimer acknowledgementDelay;
    private readonly DispatcherTimer acknowledgementCommitDelay;
    private readonly Border messagePanel;
    private IntPtr handle;
    private HwndSource? source;
    private bool serviceCloseRequested;
    private bool acknowledgementPending;
    private int alertGeneration;
    private int pendingAcknowledgementGeneration;
    private long quietPeriodNotBefore;
    private long commitNotBefore;
    private ScreenRegion? anchorRegion;

    public AffixHitOverlayWindow()
    {
        AllowsTransparency = true;
        // A fully transparent layered-window pixel can be skipped by native hit testing.
        // Alpha=1 is visually imperceptible but makes the whole shield receive mouse input.
        Background = new SolidColorBrush(Color.FromArgb(1, 0, 0, 0));
        Focusable = true;
        IsHitTestVisible = true;
        ResizeMode = ResizeMode.NoResize;
        ShowActivated = true;
        ShowInTaskbar = false;
        SizeToContent = SizeToContent.Manual;
        Topmost = true;
        WindowStartupLocation = WindowStartupLocation.Manual;
        WindowStyle = WindowStyle.None;

        Left = SystemParameters.VirtualScreenLeft;
        Top = SystemParameters.VirtualScreenTop;
        Width = SystemParameters.VirtualScreenWidth;
        Height = SystemParameters.VirtualScreenHeight;

        detectedTextBlock = new TextBlock
        {
            Foreground = Brushes.White,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 16,
            FontWeight = FontWeights.SemiBold,
            MaxWidth = 430,
            TextAlignment = TextAlignment.Center,
            TextTrimming = TextTrimming.CharacterEllipsis,
            TextWrapping = TextWrapping.Wrap,
        };

        var headline = new TextBlock
        {
            Foreground = Brushes.White,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 28,
            FontWeight = FontWeights.Bold,
            Text = "目标词缀命中",
            TextAlignment = TextAlignment.Center,
        };

        var instruction = new TextBlock
        {
            Foreground = Brushes.White,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 14,
            Margin = new Thickness(0, 8, 0, 0),
            Opacity = 0.92,
            Text = "后续鼠标点击已被此窗口阻挡，请先检查装备",
            TextAlignment = TextAlignment.Center,
        };

        acknowledgementButton = new Button
        {
            Background = new SolidColorBrush(Color.FromRgb(45, 12, 12)),
            BorderBrush = Brushes.White,
            BorderThickness = new Thickness(2),
            Content = "请停手 1.2 秒后再确认……",
            Cursor = Cursors.Hand,
            Focusable = false,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 15,
            FontWeight = FontWeights.SemiBold,
            Foreground = Brushes.White,
            HorizontalAlignment = HorizontalAlignment.Center,
            IsEnabled = false,
            Margin = new Thickness(0, 14, 0, 0),
            Padding = new Thickness(18, 9, 18, 9),
        };
        acknowledgementButton.Click += OnAcknowledgementClick;

        var hotkeyHint = new TextBlock
        {
            Foreground = Brushes.White,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 12,
            Margin = new Thickness(0, 8, 0, 0),
            Opacity = 0.85,
            Text = "也可按 Ctrl + Shift + F12 解除锁定",
            TextAlignment = TextAlignment.Center,
        };

        var messageStack = new StackPanel();
        messageStack.Children.Add(headline);
        messageStack.Children.Add(detectedTextBlock);
        messageStack.Children.Add(instruction);
        messageStack.Children.Add(acknowledgementButton);
        messageStack.Children.Add(hotkeyHint);

        messagePanel = new Border
        {
            Background = new SolidColorBrush(Color.FromArgb(232, 105, 0, 0)),
            BorderBrush = Brushes.Red,
            BorderThickness = new Thickness(4),
            CornerRadius = new CornerRadius(10),
            HorizontalAlignment = HorizontalAlignment.Center,
            Padding = new Thickness(22, 16, 22, 16),
            VerticalAlignment = VerticalAlignment.Center,
            Width = 480,
            Child = messageStack,
        };

        var root = new Grid
        {
            // Do not use Brushes.Transparent here. The non-zero alpha makes the input shield
            // cover the empty area outside the red panel as well as the visible controls.
            Background = new SolidColorBrush(Color.FromArgb(1, 0, 0, 0)),
        };
        root.Children.Add(new Border
        {
            BorderBrush = Brushes.Red,
            BorderThickness = new Thickness(14),
        });
        root.Children.Add(messagePanel);
        Content = root;

        acknowledgementDelay = new DispatcherTimer(DispatcherPriority.Send)
        {
            Interval = AcknowledgementQuietPeriod,
        };
        acknowledgementDelay.Tick += OnAcknowledgementDelayElapsed;
        acknowledgementCommitDelay = new DispatcherTimer(DispatcherPriority.Send)
        {
            Interval = AcknowledgementCommitPeriod,
        };
        acknowledgementCommitDelay.Tick += OnAcknowledgementCommitDelayElapsed;
        Loaded += OnLoaded;
        PreviewMouseDown += OnPreviewMouseInput;
        PreviewMouseUp += OnPreviewMouseInput;
        PreviewMouseWheel += OnPreviewMouseWheel;
    }

    public event EventHandler<OverlayAcknowledgementRequestedEventArgs>? AcknowledgeRequested;

    public void SetDetectedText(string detectedText)
    {
        detectedTextBlock.Text = detectedText;
    }

    public void Arm(string detectedText, int generation, ScreenRegion? region)
    {
        detectedTextBlock.Text = detectedText;
        alertGeneration = generation;
        anchorRegion = region is { IsValid: true } ? region : null;
        ResetAcknowledgementQuietPeriod(startTimer: IsLoaded && IsVisible);
        if (IsLoaded && IsVisible)
        {
            _ = Dispatcher.BeginInvoke(PositionMessageOnTargetMonitor, DispatcherPriority.Render);
        }
    }

    public void ReassertTopmostPosition()
    {
        if (handle == IntPtr.Zero)
        {
            return;
        }

        SetExactVirtualDesktopBounds(handle);
        _ = Activate();
    }

    public void CloseFromService()
    {
        serviceCloseRequested = true;
        acknowledgementDelay.Stop();
        acknowledgementCommitDelay.Stop();
        IsHitTestVisible = false;
        if (handle != IntPtr.Zero)
        {
            var releasedStyle = GetWindowLongPtr(handle, GwlExStyle).ToInt64() | WsExTransparent;
            _ = SetWindowLongPtr(handle, GwlExStyle, new IntPtr(releasedStyle));
        }

        // Fail open before closing: even if WPF rejects Close during dispatcher shutdown or a
        // re-entrant transition, the hidden/input-transparent window can no longer trap clicks.
        Hide();
        Close();
    }

    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);

        handle = new WindowInteropHelper(this).Handle;
        var extendedStyle = ComposeBlockingExtendedStyle(
            GetWindowLongPtr(handle, GwlExStyle).ToInt64());
        _ = SetWindowLongPtr(handle, GwlExStyle, new IntPtr(extendedStyle));

        source = HwndSource.FromHwnd(handle);
        source?.AddHook(WindowProcedure);

        // SetWindowPos uses physical virtual-screen coordinates and therefore avoids the gaps
        // that WPF device-independent coordinates can create on mixed-DPI monitor layouts.
        SetExactVirtualDesktopBounds(handle);
    }

    protected override void OnClosing(CancelEventArgs e)
    {
        if (!serviceCloseRequested)
        {
            // Treat Alt+F4/system close as an explicit emergency acknowledgement. Queue it so
            // the current Closing event can be cancelled before the service closes the window.
            e.Cancel = true;
            var requestedGeneration = alertGeneration;
            _ = Dispatcher.BeginInvoke(() => AcknowledgeRequested?.Invoke(
                this,
                new OverlayAcknowledgementRequestedEventArgs(requestedGeneration)));
            return;
        }

        acknowledgementDelay.Stop();
        acknowledgementCommitDelay.Stop();
        source?.RemoveHook(WindowProcedure);
        source = null;
        handle = IntPtr.Zero;
        base.OnClosing(e);
    }

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        ResetAcknowledgementQuietPeriod(startTimer: true);
        PositionMessageOnTargetMonitor();
        _ = Activate();
    }

    private void OnAcknowledgementDelayElapsed(object? sender, EventArgs e)
    {
        if (RestartTimerUntilDeadline(acknowledgementDelay, quietPeriodNotBefore))
        {
            return;
        }

        if (Mouse.LeftButton != MouseButtonState.Released ||
            Mouse.RightButton != MouseButtonState.Released ||
            Mouse.MiddleButton != MouseButtonState.Released)
        {
            ResetAcknowledgementQuietPeriod(startTimer: true);
            return;
        }

        acknowledgementDelay.Stop();
        acknowledgementButton.Content = "我已检查，解除鼠标锁定";
        acknowledgementButton.IsEnabled = true;
    }

    private void OnAcknowledgementClick(object sender, RoutedEventArgs e)
    {
        acknowledgementPending = true;
        pendingAcknowledgementGeneration = alertGeneration;
        commitNotBefore = DeadlineFromNow(AcknowledgementCommitPeriod);
        acknowledgementButton.IsEnabled = false;
        acknowledgementButton.Content = "正在解除锁定……";
        acknowledgementCommitDelay.Stop();
        acknowledgementCommitDelay.Interval = AcknowledgementCommitPeriod;
        acknowledgementCommitDelay.Start();
    }

    private void OnAcknowledgementCommitDelayElapsed(object? sender, EventArgs e)
    {
        acknowledgementCommitDelay.Stop();
        if (!acknowledgementPending || pendingAcknowledgementGeneration == 0)
        {
            return;
        }

        if (RestartTimerUntilDeadline(acknowledgementCommitDelay, commitNotBefore))
        {
            return;
        }

        if (
            Mouse.LeftButton != MouseButtonState.Released ||
            Mouse.RightButton != MouseButtonState.Released ||
            Mouse.MiddleButton != MouseButtonState.Released)
        {
            ResetAcknowledgementQuietPeriod(startTimer: true);
            return;
        }

        var requestedGeneration = pendingAcknowledgementGeneration;
        acknowledgementPending = false;
        pendingAcknowledgementGeneration = 0;
        AcknowledgeRequested?.Invoke(
            this,
            new OverlayAcknowledgementRequestedEventArgs(requestedGeneration));
    }

    private void OnPreviewMouseInput(object sender, MouseButtonEventArgs e)
    {
        if (acknowledgementPending)
        {
            ResetAcknowledgementQuietPeriod(startTimer: true);
            return;
        }

        var isExplicitConfirmation =
            e.ChangedButton == MouseButton.Left &&
            acknowledgementButton.IsEnabled &&
            acknowledgementButton.IsMouseOver;
        if (!isExplicitConfirmation)
        {
            ResetAcknowledgementQuietPeriod(startTimer: true);
        }
    }

    private void OnPreviewMouseWheel(object sender, MouseWheelEventArgs e)
    {
        ResetAcknowledgementQuietPeriod(startTimer: true);
    }

    private void ResetAcknowledgementQuietPeriod(bool startTimer)
    {
        acknowledgementDelay.Stop();
        acknowledgementCommitDelay.Stop();
        acknowledgementPending = false;
        pendingAcknowledgementGeneration = 0;
        commitNotBefore = 0;
        acknowledgementButton.IsEnabled = false;
        acknowledgementButton.Content = "请停手 1.2 秒后再确认……";
        if (startTimer)
        {
            quietPeriodNotBefore = DeadlineFromNow(AcknowledgementQuietPeriod);
            acknowledgementDelay.Interval = AcknowledgementQuietPeriod;
            acknowledgementDelay.Start();
        }
    }

    private IntPtr WindowProcedure(
        IntPtr windowHandle,
        int message,
        IntPtr wParam,
        IntPtr lParam,
        ref bool handled)
    {
        if (message == WmMouseActivate)
        {
            // If Windows did not grant foreground activation when the alert was shown, eat
            // the click that activates it. That click must never fall through to POE.
            ResetAcknowledgementQuietPeriod(startTimer: true);
            handled = true;
            return new IntPtr(MaActivateAndEat);
        }

        if (message is WmDisplayChange or WmDpiChanged)
        {
            _ = Dispatcher.BeginInvoke(() =>
            {
                if (handle == windowHandle && source is not null)
                {
                    SetExactVirtualDesktopBounds(windowHandle);
                    PositionMessageOnTargetMonitor();
                }
            });
        }

        return IntPtr.Zero;
    }

    private static long DeadlineFromNow(TimeSpan duration) =>
        Stopwatch.GetTimestamp() + (long)(duration.TotalSeconds * Stopwatch.Frequency);

    private static bool RestartTimerUntilDeadline(DispatcherTimer timer, long deadline)
    {
        var remainingTicks = deadline - Stopwatch.GetTimestamp();
        if (remainingTicks <= 0)
        {
            return false;
        }

        timer.Stop();
        timer.Interval = TimeSpan.FromSeconds(remainingTicks / (double)Stopwatch.Frequency);
        timer.Start();
        return true;
    }

    private void PositionMessageOnTargetMonitor()
    {
        if (source?.CompositionTarget is not { } compositionTarget ||
            Content is not FrameworkElement root ||
            root.ActualWidth <= 0 ||
            root.ActualHeight <= 0)
        {
            messagePanel.RenderTransform = Transform.Identity;
            return;
        }

        var monitor = anchorRegion is { IsValid: true } region
            ? MonitorFromPoint(
                new NativePoint(
                    region.X + (region.Width / 2),
                    region.Y + (region.Height / 2)),
                MonitorDefaultToPrimary)
            : MonitorFromPoint(new NativePoint(0, 0), MonitorDefaultToPrimary);
        var monitorInfo = new MonitorInfo
        {
            Size = Marshal.SizeOf<MonitorInfo>(),
        };
        if (monitor == IntPtr.Zero || !GetMonitorInfo(monitor, ref monitorInfo))
        {
            messagePanel.RenderTransform = Transform.Identity;
            return;
        }

        // Put the confirmation card in the middle of the monitor containing the selected OCR
        // region. The full virtual-desktop window remains behind it as an invisible input shield.
        var monitorCenterPhysicalX =
            ((monitorInfo.WorkArea.Left + monitorInfo.WorkArea.Right) / 2.0) -
            GetSystemMetrics(SmXVirtualScreen);
        var monitorCenterPhysicalY =
            ((monitorInfo.WorkArea.Top + monitorInfo.WorkArea.Bottom) / 2.0) -
            GetSystemMetrics(SmYVirtualScreen);
        var monitorCenterDip = compositionTarget.TransformFromDevice.Transform(
            new Point(monitorCenterPhysicalX, monitorCenterPhysicalY));
        messagePanel.RenderTransform = new TranslateTransform(
            monitorCenterDip.X - (root.ActualWidth / 2),
            monitorCenterDip.Y - (root.ActualHeight / 2));
    }

    private static void SetExactVirtualDesktopBounds(IntPtr windowHandle)
    {
        var x = GetSystemMetrics(SmXVirtualScreen);
        var y = GetSystemMetrics(SmYVirtualScreen);
        var width = GetSystemMetrics(SmCxVirtualScreen);
        var height = GetSystemMetrics(SmCyVirtualScreen);

        _ = SetWindowPos(
            windowHandle,
            HwndTopmost,
            x,
            y,
            width,
            height,
            SwpNoActivate | SwpShowWindow);
    }

    private static long ComposeBlockingExtendedStyle(long extendedStyle) =>
        (extendedStyle | WsExToolWindow) & ~(WsExTransparent | WsExNoActivate);

    private static IntPtr GetWindowLongPtr(IntPtr windowHandle, int index)
    {
        return IntPtr.Size == 8
            ? GetWindowLongPtr64(windowHandle, index)
            : new IntPtr(GetWindowLong32(windowHandle, index));
    }

    private static IntPtr SetWindowLongPtr(IntPtr windowHandle, int index, IntPtr newValue)
    {
        return IntPtr.Size == 8
            ? SetWindowLongPtr64(windowHandle, index, newValue)
            : new IntPtr(SetWindowLong32(windowHandle, index, newValue.ToInt32()));
    }

    [DllImport("user32.dll", EntryPoint = "GetWindowLongW")]
    private static extern int GetWindowLong32(IntPtr windowHandle, int index);

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
    private static extern IntPtr GetWindowLongPtr64(IntPtr windowHandle, int index);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongW")]
    private static extern int SetWindowLong32(IntPtr windowHandle, int index, int newValue);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")]
    private static extern IntPtr SetWindowLongPtr64(IntPtr windowHandle, int index, IntPtr newValue);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowPos(
        IntPtr windowHandle,
        IntPtr insertAfter,
        int x,
        int y,
        int width,
        int height,
        uint flags);

    [DllImport("user32.dll")]
    private static extern int GetSystemMetrics(int index);

    [DllImport("user32.dll")]
    private static extern IntPtr MonitorFromPoint(NativePoint point, uint flags);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetMonitorInfo(IntPtr monitor, ref MonitorInfo monitorInfo);

    [StructLayout(LayoutKind.Sequential)]
    private readonly struct NativePoint
    {
        public NativePoint(int x, int y)
        {
            X = x;
            Y = y;
        }

        public readonly int X;
        public readonly int Y;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeRect
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct MonitorInfo
    {
        public int Size;
        public NativeRect Monitor;
        public NativeRect WorkArea;
        public uint Flags;
    }
}

internal sealed class OverlayAcknowledgementRequestedEventArgs(int generation) : EventArgs
{
    public int Generation { get; } = generation;
}
