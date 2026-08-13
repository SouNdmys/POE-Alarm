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
using PoeAlarm.App.Localization;

namespace PoeAlarm.App.Alerts;

internal sealed class AffixHitOverlayWindow : Window
{
    private static readonly TimeSpan AcknowledgementCommitPeriod = TimeSpan.FromMilliseconds(300);

    private const int GwlExStyle = -20;
    private const long WsExTransparent = 0x00000020L;
    private const long WsExToolWindow = 0x00000080L;
    private const long WsExNoActivate = 0x08000000L;

    private const int WmMouseActivate = 0x0021;
    private const int WmDisplayChange = 0x007E;
    private const int WmDpiChanged = 0x02E0;
    private const int MaActivateAndEat = 2;
    private const uint WdaExcludeFromCapture = 0x00000011;
    private const uint WdaNone = 0;
    private const int VkLeftButton = 0x01;
    private const int VkRightButton = 0x02;
    private const int VkMiddleButton = 0x04;
    private const int VkXButton1 = 0x05;
    private const int VkXButton2 = 0x06;

    private const int SmXVirtualScreen = 76;
    private const int SmYVirtualScreen = 77;
    private const int SmCxVirtualScreen = 78;
    private const int SmCyVirtualScreen = 79;
    private const uint MonitorDefaultToPrimary = 1;

    private const uint SwpNoActivate = 0x0010;
    private const uint SwpShowWindow = 0x0040;
    private const int SwHide = 0;

    private static readonly IntPtr HwndTopmost = new(-1);

    // Native SendInput is unavailable on some CI/sandbox desktops. This internal seam lets the
    // UI contract probe drive the same held/released boundary without mutating real user input.
    internal static Func<bool>? MouseButtonPressedTestOverride { get; set; }

    private readonly TextBlock detectedTextBlock;
    private readonly TextBlock headline;
    private readonly TextBlock instruction;
    private readonly TextBlock hotkeyHint;
    private readonly Button acknowledgementButton;
    private readonly Button guardedStopButton;
    private readonly DispatcherTimer acknowledgementCommitDelay;
    private readonly DispatcherTimer alertPromotionDelay;
    private readonly Grid alertVisualLayer;
    private readonly Border messagePanel;
    private readonly Border desktopBorder;
    private IntPtr handle;
    private HwndSource? source;
    private bool serviceCloseRequested;
    private bool presentationPrepared;
    private bool alertPromotionPending;
    private bool acknowledgementPending;
    private OverlayAction pendingAction;
    private OverlayPresentationKind presentationKind;
    private int alertGeneration;
    private int pendingAcknowledgementGeneration;
    private long commitNotBefore;
    private ScreenRegion? anchorRegion;
    private bool excludeFromCapture;

    public AffixHitOverlayWindow()
        : this(excludeFromCapture: false)
    {
    }

    public AffixHitOverlayWindow(bool excludeFromCapture)
    {
        this.excludeFromCapture = excludeFromCapture;
        AllowsTransparency = true;
        // A fully transparent layered-window pixel can be skipped by native hit testing.
        // Alpha=1 is visually imperceptible but makes the whole shield receive mouse input.
        Background = new SolidColorBrush(Color.FromArgb(1, 0, 0, 0));
        Focusable = true;
        // The reusable native window starts fail-open. Arm enables WPF and native hit testing only
        // for the short interval in which the visible alert is intentionally blocking input.
        IsHitTestVisible = false;
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

        headline = new TextBlock
        {
            Foreground = Brushes.White,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 28,
            FontWeight = FontWeights.Bold,
            Text = UiText.Current.AlertTitle,
            TextAlignment = TextAlignment.Center,
        };

        instruction = new TextBlock
        {
            Foreground = Brushes.White,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 14,
            Margin = new Thickness(0, 8, 0, 0),
            Opacity = 0.92,
            Text = UiText.Current.AlertBlockingMessage,
            TextAlignment = TextAlignment.Center,
        };

        acknowledgementButton = new Button
        {
            Background = new SolidColorBrush(Color.FromRgb(45, 12, 12)),
            BorderBrush = Brushes.White,
            BorderThickness = new Thickness(2),
            Content = UiText.Current.AlertAcknowledge,
            Cursor = Cursors.Hand,
            Focusable = false,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 15,
            FontWeight = FontWeights.SemiBold,
            Foreground = Brushes.White,
            HorizontalAlignment = HorizontalAlignment.Center,
            IsEnabled = true,
            Margin = new Thickness(0, 14, 0, 0),
            Padding = new Thickness(18, 9, 18, 9),
        };
        acknowledgementButton.Click += OnAcknowledgementClick;

        guardedStopButton = new Button
        {
            Background = new SolidColorBrush(Color.FromRgb(66, 45, 6)),
            BorderBrush = Brushes.White,
            BorderThickness = new Thickness(2),
            Cursor = Cursors.Hand,
            Focusable = false,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 15,
            FontWeight = FontWeights.SemiBold,
            Foreground = Brushes.White,
            HorizontalAlignment = HorizontalAlignment.Center,
            IsEnabled = true,
            Margin = new Thickness(10, 14, 0, 0),
            Padding = new Thickness(18, 9, 18, 9),
            Visibility = Visibility.Collapsed,
        };
        guardedStopButton.Click += OnGuardedStopClick;

        hotkeyHint = new TextBlock
        {
            Foreground = Brushes.White,
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 12,
            Margin = new Thickness(0, 8, 0, 0),
            Opacity = 0.85,
            Text = UiText.Current.AlertShortcut,
            TextAlignment = TextAlignment.Center,
        };

        var guardedButtonPanel = new StackPanel
        {
            HorizontalAlignment = HorizontalAlignment.Center,
            Orientation = Orientation.Horizontal,
        };
        guardedButtonPanel.Children.Add(acknowledgementButton);
        guardedButtonPanel.Children.Add(guardedStopButton);

        var messageStack = new StackPanel();
        messageStack.Children.Add(headline);
        messageStack.Children.Add(detectedTextBlock);
        messageStack.Children.Add(instruction);
        messageStack.Children.Add(guardedButtonPanel);
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
        alertVisualLayer = new Grid();
        desktopBorder = new Border
        {
            BorderBrush = Brushes.Red,
            BorderThickness = new Thickness(14),
        };
        alertVisualLayer.Children.Add(desktopBorder);
        alertVisualLayer.Children.Add(messagePanel);
        root.Children.Add(alertVisualLayer);
        Content = root;

        acknowledgementCommitDelay = new DispatcherTimer(DispatcherPriority.Send)
        {
            Interval = AcknowledgementCommitPeriod,
        };
        acknowledgementCommitDelay.Tick += OnAcknowledgementCommitDelayElapsed;
        alertPromotionDelay = new DispatcherTimer(DispatcherPriority.Send)
        {
            Interval = TimeSpan.FromMilliseconds(1),
        };
        alertPromotionDelay.Tick += OnAlertPromotionDelayElapsed;
    }

    public void ApplyLanguage()
    {
        if (presentationKind == OverlayPresentationKind.GuardedUncertain)
        {
            return;
        }

        var text = UiText.Current;
        headline.Text = text.AlertTitle;
        instruction.Text = text.AlertBlockingMessage;
        hotkeyHint.Text = text.AlertShortcut;
        if (!acknowledgementPending)
        {
            acknowledgementButton.Content = text.AlertAcknowledge;
            acknowledgementButton.IsEnabled = true;
        }
    }

    public void SetExcludeFromCapture(bool exclude)
    {
        excludeFromCapture = exclude;
        ApplyDisplayAffinity();
    }

    private void ApplyDisplayAffinity()
    {
        if (handle != IntPtr.Zero)
        {
            _ = SetWindowDisplayAffinity(
                handle,
                excludeFromCapture ? WdaExcludeFromCapture : WdaNone);
        }
    }

    public event EventHandler<OverlayAcknowledgementRequestedEventArgs>? AcknowledgeRequested;

    public event EventHandler<OverlayPresentedEventArgs>? AlertPresented;

    public event EventHandler<GuardedOverlayActionRequestedEventArgs>? GuardedActionRequested;

    /// <summary>
    /// Creates the native window while it is hidden and explicitly input-transparent. This keeps
    /// HWND creation and source initialization out of the later match-to-block critical path.
    /// </summary>
    public void PrepareForReuse()
    {
        if (serviceCloseRequested)
        {
            throw new InvalidOperationException("A closed alert overlay cannot be prepared again.");
        }

        _ = new WindowInteropHelper(this).EnsureHandle();
        ReleaseForReuse();
        if (presentationPrepared)
        {
            return;
        }

        // WPF's first Show performs substantially more work than later Hide/Show cycles. Pay that
        // cost while the user is still on the Start button, with the non-activating,
        // input-transparent window entirely offscreen, rather than after the target is found.
        var previousLeft = Left;
        var previousTop = Top;
        var previousShowActivated = ShowActivated;
        try
        {
            ShowActivated = false;
            Left = SystemParameters.VirtualScreenLeft + SystemParameters.VirtualScreenWidth + 10_000;
            Top = SystemParameters.VirtualScreenTop;
            Show();
            ReleaseInputShield();
            UpdateLayout();
            presentationPrepared = true;
        }
        finally
        {
            ReleaseForReuse();
            Left = previousLeft;
            Top = previousTop;
            if (handle != IntPtr.Zero)
            {
                SetExactVirtualDesktopBounds(handle, showWindow: false);
            }

            ShowActivated = previousShowActivated;
        }
    }

    public void SetDetectedText(string detectedText)
    {
        detectedTextBlock.Text = detectedText;
    }

    public void Arm(string detectedText, int generation, ScreenRegion? region)
    {
        if (serviceCloseRequested)
        {
            throw new InvalidOperationException("A closed alert overlay cannot be armed again.");
        }

        _ = new WindowInteropHelper(this).EnsureHandle();
        presentationKind = OverlayPresentationKind.RedAlert;
        ApplyRedVisuals();
        detectedTextBlock.Text = detectedText;
        alertGeneration = generation;
        anchorRegion = region is { IsValid: true } ? region : null;
        alertPromotionPending = AnyMouseButtonPressed();
        if (alertPromotionPending)
        {
            // Keep the window dormant until the click that produced the matching frame has sent
            // its button-up to POE. The release gate will promote this exact generation.
            alertVisualLayer.Visibility = Visibility.Collapsed;
            IsHitTestVisible = false;
            ApplyReleasedStyle();
            alertPromotionDelay.Start();
        }
        else
        {
            ConfigureBlockingAlert();
        }

        if (IsLoaded && IsVisible)
        {
            _ = Dispatcher.BeginInvoke(PositionMessageOnTargetMonitor, DispatcherPriority.Render);
        }
    }

    public void ArmGuarded(
        string title,
        string detail,
        string instructionText,
        string continueButtonText,
        string stopButtonText,
        int generation,
        ScreenRegion? region)
    {
        if (serviceCloseRequested)
        {
            throw new InvalidOperationException("A closed alert overlay cannot be armed again.");
        }

        ArgumentException.ThrowIfNullOrWhiteSpace(title);
        ArgumentException.ThrowIfNullOrWhiteSpace(detail);
        ArgumentException.ThrowIfNullOrWhiteSpace(instructionText);
        ArgumentException.ThrowIfNullOrWhiteSpace(continueButtonText);
        ArgumentException.ThrowIfNullOrWhiteSpace(stopButtonText);

        _ = new WindowInteropHelper(this).EnsureHandle();
        presentationKind = OverlayPresentationKind.GuardedUncertain;
        headline.Text = title.Trim();
        detectedTextBlock.Text = detail.Trim();
        instruction.Text = instructionText.Trim();
        hotkeyHint.Visibility = Visibility.Collapsed;
        acknowledgementButton.Content = continueButtonText.Trim();
        acknowledgementButton.Background = new SolidColorBrush(Color.FromRgb(54, 52, 12));
        guardedStopButton.Content = stopButtonText.Trim();
        guardedStopButton.Visibility = Visibility.Visible;
        desktopBorder.BorderBrush = Brushes.Gold;
        messagePanel.BorderBrush = Brushes.Gold;
        messagePanel.Background = new SolidColorBrush(Color.FromArgb(238, 96, 76, 4));
        alertGeneration = generation;
        anchorRegion = region is { IsValid: true } ? region : null;

        alertPromotionPending = AnyMouseButtonPressed();
        if (alertPromotionPending)
        {
            // Exactly like the red path, the yellow HWND remains visually dormant and natively
            // input-transparent while the physical click that caused this decision is held. The
            // PendingMouseInputGuard therefore remains the sole owner and lets that causative Up
            // reach POE. Only a fully released five-button mask may promote this HWND.
            alertVisualLayer.Visibility = Visibility.Collapsed;
            IsHitTestVisible = false;
            ApplyReleasedStyle();
            alertPromotionDelay.Start();
        }
        else
        {
            ConfigureBlockingAlert();
        }
    }

    /// <summary>Shows the already-armed native input shield before starting non-critical UI work.</summary>
    public void Present()
    {
        if (alertPromotionPending)
        {
            PresentDormantWindow();
            alertPromotionDelay.Start();
            return;
        }

        if (!IsHitTestVisible || handle == IntPtr.Zero)
        {
            throw new InvalidOperationException("Prepare and arm the alert overlay before presenting it.");
        }

        // Show the already-rendered HWND natively first. This is the exact point at which the
        // prewarmed topmost window begins receiving clicks; WPF can synchronize its Visibility
        // state immediately afterwards without leaving the game exposed during first-Show work.
        SetExactVirtualDesktopBounds(handle, showWindow: true);
        try
        {
            if (!IsVisible)
            {
                Show();
            }
        }
        catch
        {
            // A failed WPF visibility transition must never leave a native invisible-looking
            // window swallowing input.
            ReleaseForReuse();
            throw;
        }

        // WPF may recompute extended styles and DIP-based bounds while synchronizing Visibility.
        // Reassert both without changing the fact that the shield was already input-ready above.
        var blockingStyle = ComposeBlockingExtendedStyle(
            GetWindowLongPtr(handle, GwlExStyle).ToInt64());
        _ = SetWindowLongPtr(handle, GwlExStyle, new IntPtr(blockingStyle));
        SetExactVirtualDesktopBounds(handle, showWindow: true);
        presentationPrepared = true;
        ResetAcknowledgementState();
        // Native topmost visibility is enough to begin swallowing clicks. Card positioning and
        // foreground activation can finish on the next dispatcher item without delaying that
        // input-ready point.
        _ = Dispatcher.BeginInvoke(CompletePresentation, DispatcherPriority.Send);
        AlertPresented?.Invoke(this, new OverlayPresentedEventArgs(alertGeneration));
    }

    public void ReassertTopmostPosition()
    {
        if (handle == IntPtr.Zero)
        {
            return;
        }

        SetExactVirtualDesktopBounds(handle, showWindow: true);
        _ = Activate();
    }

    /// <summary>
    /// Fails open immediately, then hides the window while retaining its initialized HWND for the
    /// next monitoring session.
    /// </summary>
    public void ReleaseForReuse()
    {
        acknowledgementCommitDelay.Stop();
        alertPromotionDelay.Stop();
        acknowledgementPending = false;
        pendingAcknowledgementGeneration = 0;
        pendingAction = OverlayAction.None;
        alertPromotionPending = false;
        // Keep the reusable surface visually empty while hidden. This prevents a dormant native
        // Show during a held matching click from briefly exposing stale red pixels rendered by a
        // previous alert or by the prewarm pass.
        alertVisualLayer.Visibility = Visibility.Collapsed;
        ReleaseInputShield();
        if (IsVisible)
        {
            Hide();
        }
        else if (handle != IntPtr.Zero)
        {
            // Present shows the HWND natively just before WPF updates IsVisible. Cover the narrow
            // failure/re-entrancy case where only the native half of that transition completed.
            _ = ShowWindow(handle, SwHide);
        }
    }

    public void CloseFromService()
    {
        serviceCloseRequested = true;
        ReleaseForReuse();

        // Fail open before closing: even if WPF rejects Close during dispatcher shutdown or a
        // re-entrant transition, the hidden/input-transparent window can no longer trap clicks.
        Close();
    }

    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);

        handle = new WindowInteropHelper(this).Handle;
        var extendedStyle = ComposeReleasedExtendedStyle(
            GetWindowLongPtr(handle, GwlExStyle).ToInt64());
        _ = SetWindowLongPtr(handle, GwlExStyle, new IntPtr(extendedStyle));

        source = HwndSource.FromHwnd(handle);
        source?.AddHook(WindowProcedure);
        ApplyDisplayAffinity();

        // SetWindowPos uses physical virtual-screen coordinates and therefore avoids the gaps
        // that WPF device-independent coordinates can create on mixed-DPI monitor layouts.
        SetExactVirtualDesktopBounds(handle, showWindow: false);
    }

    protected override void OnClosing(CancelEventArgs e)
    {
        if (!serviceCloseRequested)
        {
            // Treat Alt+F4/system close as an explicit emergency acknowledgement. Queue it so
            // the current Closing event can be cancelled before the service closes the window.
            e.Cancel = true;
            var requestedGeneration = alertGeneration;
            _ = Dispatcher.BeginInvoke(() =>
            {
                if (presentationKind == OverlayPresentationKind.GuardedUncertain)
                {
                    // Keep the desktop shield in place until the service has stopped Guarded and
                    // released its native owner. This mirrors the button path and avoids a small
                    // close-to-stop window in which input could reach the game.
                    GuardedActionRequested?.Invoke(
                        this,
                        new GuardedOverlayActionRequestedEventArgs(
                            requestedGeneration,
                            GuardedOverlayAction.StopMonitoring));
                }
                else
                {
                    AcknowledgeRequested?.Invoke(
                        this,
                        new OverlayAcknowledgementRequestedEventArgs(requestedGeneration));
                }
            });
            return;
        }

        acknowledgementCommitDelay.Stop();
        source?.RemoveHook(WindowProcedure);
        source = null;
        handle = IntPtr.Zero;
        base.OnClosing(e);
    }

    private void CompletePresentation()
    {
        if (!IsVisible || !IsHitTestVisible || handle == IntPtr.Zero)
        {
            return;
        }

        PositionMessageOnTargetMonitor();
        _ = Activate();
    }

    private void OnAcknowledgementClick(object sender, RoutedEventArgs e)
    {
        pendingAction = presentationKind == OverlayPresentationKind.GuardedUncertain
            ? OverlayAction.GuardedContinue
            : OverlayAction.Acknowledge;
        BeginActionCommit();
    }

    /// <summary>
    /// Native-only emergency release for a dispatcher watchdog. Win32 window style/visibility
    /// calls are safe across threads, so a stalled WPF dispatcher cannot leave this HWND as an
    /// invisible or half-presented desktop-wide input sink. WPF cleanup follows when it recovers.
    /// </summary>
    public void FailOpenNativeInputShield()
    {
        var windowHandle = handle;
        if (windowHandle == IntPtr.Zero)
        {
            return;
        }

        var releasedStyle = ComposeReleasedExtendedStyle(
            GetWindowLongPtr(windowHandle, GwlExStyle).ToInt64());
        _ = SetWindowLongPtr(windowHandle, GwlExStyle, new IntPtr(releasedStyle));
        _ = ShowWindow(windowHandle, SwHide);
    }

    private void OnGuardedStopClick(object sender, RoutedEventArgs e)
    {
        pendingAction = OverlayAction.GuardedStop;
        BeginActionCommit();
    }

    private void BeginActionCommit()
    {
        acknowledgementPending = true;
        pendingAcknowledgementGeneration = alertGeneration;
        commitNotBefore = DeadlineFromNow(AcknowledgementCommitPeriod);
        acknowledgementButton.IsEnabled = false;
        guardedStopButton.IsEnabled = false;
        if (presentationKind == OverlayPresentationKind.RedAlert)
        {
            acknowledgementButton.Content = UiText.Current.AlertReleasing;
        }
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

        if (AnyMouseButtonPressed())
        {
            // Keep swallowing the confirmation click's tail until L/R/M/X1/X2 are all released.
            commitNotBefore = DeadlineFromNow(AcknowledgementCommitPeriod);
            acknowledgementCommitDelay.Interval = AcknowledgementCommitPeriod;
            acknowledgementCommitDelay.Start();
            return;
        }

        var requestedGeneration = pendingAcknowledgementGeneration;
        var requestedAction = pendingAction;
        acknowledgementPending = false;
        pendingAcknowledgementGeneration = 0;
        pendingAction = OverlayAction.None;
        if (requestedAction == OverlayAction.Acknowledge)
        {
            AcknowledgeRequested?.Invoke(
                this,
                new OverlayAcknowledgementRequestedEventArgs(requestedGeneration));
            return;
        }

        // Keep the blocking HWND in place while the service atomically installs either the next
        // one-click Guarded cycle or the terminal stop. The service releases this overlay only
        // after its replacement input owner is ready.
        GuardedActionRequested?.Invoke(
            this,
            new GuardedOverlayActionRequestedEventArgs(
                requestedGeneration,
                requestedAction == OverlayAction.GuardedContinue
                    ? GuardedOverlayAction.ContinueMonitoring
                    : GuardedOverlayAction.StopMonitoring));
    }

    private void OnAlertPromotionDelayElapsed(object? sender, EventArgs e)
    {
        if (!alertPromotionPending)
        {
            alertPromotionDelay.Stop();
            return;
        }

        if (!AnyMouseButtonPressed())
        {
            PromoteDeferredAlertNow();
        }
    }

    private void ResetAcknowledgementState()
    {
        acknowledgementCommitDelay.Stop();
        acknowledgementPending = false;
        pendingAcknowledgementGeneration = 0;
        pendingAction = OverlayAction.None;
        commitNotBefore = 0;
        acknowledgementButton.IsEnabled = true;
        guardedStopButton.IsEnabled = true;
        if (presentationKind == OverlayPresentationKind.RedAlert)
        {
            acknowledgementButton.Content = UiText.Current.AlertAcknowledge;
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
            ResetAcknowledgementState();
            handled = true;
            return new IntPtr(MaActivateAndEat);
        }

        if (message is WmDisplayChange or WmDpiChanged)
        {
            _ = Dispatcher.BeginInvoke(() =>
            {
                if (handle == windowHandle && source is not null)
                {
                    SetExactVirtualDesktopBounds(windowHandle, showWindow: IsVisible);
                    PositionMessageOnTargetMonitor();
                }
            });
        }

        return IntPtr.Zero;
    }

    private static long DeadlineFromNow(TimeSpan duration) =>
        Stopwatch.GetTimestamp() + (long)(duration.TotalSeconds * Stopwatch.Frequency);

    private static bool AnyMouseButtonPressed() =>
        MouseButtonPressedTestOverride?.Invoke() ??
        (IsVirtualKeyDown(VkLeftButton) ||
         IsVirtualKeyDown(VkRightButton) ||
         IsVirtualKeyDown(VkMiddleButton) ||
         IsVirtualKeyDown(VkXButton1) ||
         IsVirtualKeyDown(VkXButton2));

    private static bool IsVirtualKeyDown(int virtualKey) =>
        (GetAsyncKeyState(virtualKey) & 0x8000) != 0;

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

    private void ConfigureBlockingAlert()
    {
        alertPromotionDelay.Stop();
        alertPromotionPending = false;
        alertVisualLayer.Visibility = Visibility.Visible;
        ResetAcknowledgementState();
        IsHitTestVisible = true;
        ApplyBlockingStyle();
    }

    private void ApplyRedVisuals()
    {
        var text = UiText.Current;
        headline.Text = text.AlertTitle;
        instruction.Text = text.AlertBlockingMessage;
        hotkeyHint.Text = text.AlertShortcut;
        hotkeyHint.Visibility = Visibility.Visible;
        acknowledgementButton.Content = text.AlertAcknowledge;
        acknowledgementButton.Background = new SolidColorBrush(Color.FromRgb(45, 12, 12));
        guardedStopButton.Visibility = Visibility.Collapsed;
        desktopBorder.BorderBrush = Brushes.Red;
        messagePanel.BorderBrush = Brushes.Red;
        messagePanel.Background = new SolidColorBrush(Color.FromArgb(232, 105, 0, 0));
    }

    private void PromoteDeferredAlertNow()
    {
        if (!alertPromotionPending || handle == IntPtr.Zero || AnyMouseButtonPressed())
        {
            return;
        }

        ConfigureBlockingAlert();
        SetExactVirtualDesktopBounds(handle, showWindow: true);
        presentationPrepared = true;
        ResetAcknowledgementState();
        _ = Dispatcher.BeginInvoke(CompletePresentation, DispatcherPriority.Send);
        AlertPresented?.Invoke(this, new OverlayPresentedEventArgs(alertGeneration));
    }

    private void PresentDormantWindow()
    {
        if (handle == IntPtr.Zero)
        {
            return;
        }

        IsHitTestVisible = false;
        ApplyReleasedStyle();
        SetExactVirtualDesktopBounds(handle, showWindow: true);
        var previousShowActivated = ShowActivated;
        try
        {
            ShowActivated = false;
            if (!IsVisible)
            {
                Show();
            }
        }
        catch
        {
            ReleaseForReuse();
            throw;
        }
        finally
        {
            ShowActivated = previousShowActivated;
        }

        ApplyReleasedStyle();
        SetExactVirtualDesktopBounds(handle, showWindow: true);
        presentationPrepared = true;
    }

    private void ApplyBlockingStyle()
    {
        if (handle == IntPtr.Zero)
        {
            return;
        }

        var style = ComposeBlockingExtendedStyle(GetWindowLongPtr(handle, GwlExStyle).ToInt64());
        _ = SetWindowLongPtr(handle, GwlExStyle, new IntPtr(style));
    }

    private void ApplyReleasedStyle()
    {
        if (handle == IntPtr.Zero)
        {
            return;
        }

        var style = ComposeReleasedExtendedStyle(GetWindowLongPtr(handle, GwlExStyle).ToInt64());
        _ = SetWindowLongPtr(handle, GwlExStyle, new IntPtr(style));
    }

    private void ReleaseInputShield()
    {
        IsHitTestVisible = false;
        if (handle == IntPtr.Zero)
        {
            return;
        }

        ApplyReleasedStyle();
    }

    private static void SetExactVirtualDesktopBounds(IntPtr windowHandle, bool showWindow)
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
            SwpNoActivate | (showWindow ? SwpShowWindow : 0));
    }

    private static long ComposeBlockingExtendedStyle(long extendedStyle) =>
        (extendedStyle | WsExToolWindow) & ~(WsExTransparent | WsExNoActivate);

    private static long ComposeReleasedExtendedStyle(long extendedStyle) =>
        extendedStyle | WsExToolWindow | WsExTransparent | WsExNoActivate;

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
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowDisplayAffinity(IntPtr windowHandle, uint affinity);

    [DllImport("user32.dll")]
    private static extern short GetAsyncKeyState(int virtualKey);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ShowWindow(IntPtr windowHandle, int command);

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

internal sealed class OverlayPresentedEventArgs(int generation) : EventArgs
{
    public int Generation { get; } = generation;
}

internal enum GuardedOverlayAction
{
    ContinueMonitoring,
    StopMonitoring,
}

internal sealed class GuardedOverlayActionRequestedEventArgs(
    int generation,
    GuardedOverlayAction action) : EventArgs
{
    public int Generation { get; } = generation;

    public GuardedOverlayAction Action { get; } = action;
}

internal enum OverlayPresentationKind
{
    RedAlert,
    GuardedUncertain,
}

internal enum OverlayAction
{
    None,
    Acknowledge,
    GuardedContinue,
    GuardedStop,
}
