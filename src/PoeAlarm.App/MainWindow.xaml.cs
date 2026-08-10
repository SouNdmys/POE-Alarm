using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Media;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Threading;
using Microsoft.Win32;
using PoeAlarm.App.Alerts;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Configuration;
using PoeAlarm.App.Input;
using PoeAlarm.App.Monitoring;
using PoeAlarm.App.Recognition;
using PoeAlarm.App.Replay;
using PoeAlarm.App.Selection;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.App;

public partial class MainWindow : Window
{
    private readonly SettingsStore _settingsStore = new();
    private readonly IAlertService _alertService;
    private AffixMonitor? _monitor;
    private MonitoringHudWindow? _monitoringHud;
    private GlobalHotKeyRegistration? _acknowledgementHotKey;
    private GlobalHotKeyRegistration? _selectionHotKey;
    private ScreenRegion? _captureRegion;
    private bool _isLoaded;
    private bool _isSelectingRegion;
    private bool _isClosing;
    private bool _closeReady;
    private long _lastUiUpdateTimestamp;
    private long _nextMonitoringSession;
    private long _activeMonitoringSession;

    public MainWindow()
    {
        InitializeComponent();
        _alertService = new LatchedAlertService();
        _alertService.Acknowledged += OnAlertAcknowledged;
    }

    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        var settings = await _settingsStore.LoadAsync();
        TargetAffixTextBox.Text = settings.TargetAffix;
        _captureRegion = settings.CaptureRegion is { IsValid: true } region ? region : null;
        UpdateRegionText();
        UpdateCanonicalPreview();
        _isLoaded = true;
    }

    private void OnSourceInitialized(object? sender, EventArgs e)
    {
        try
        {
            _acknowledgementHotKey = new GlobalHotKeyRegistration(
                this,
                identifier: 0x504F45,
                ModifierKeys.Control | ModifierKeys.Shift,
                Key.F12);
            _acknowledgementHotKey.Pressed += (_, _) => AcknowledgeAlert();
        }
        catch (Win32Exception exception)
        {
            StatusText.Text = $"全局确认快捷键不可用：{exception.Message}";
        }

        try
        {
            _selectionHotKey = new GlobalHotKeyRegistration(
                this,
                identifier: 0x504F46,
                ModifierKeys.Control | ModifierKeys.Shift,
                Key.F11);
            _selectionHotKey.Pressed += async (_, _) => await SelectRegionAsync();
        }
        catch (Win32Exception exception)
        {
            StatusText.Text = $"全局框选快捷键不可用：{exception.Message}";
        }
    }

    private void OnPasteTemplate(object sender, RoutedEventArgs e)
    {
        if (Clipboard.ContainsText())
        {
            TargetAffixTextBox.Text = Clipboard.GetText().Trim();
        }
    }

    private void OnTargetAffixChanged(object sender, System.Windows.Controls.TextChangedEventArgs e) =>
        UpdateCanonicalPreview();

    private async void OnSelectRegion(object sender, RoutedEventArgs e) => await SelectRegionAsync();

    private async Task SelectRegionAsync()
    {
        if (_isSelectingRegion || _closeReady || !IsEnabled)
        {
            return;
        }

        _isSelectingRegion = true;
        try
        {
            await StopMonitorAsync(showIdleStatus: false);
            Hide();
            var selector = new RegionSelectionWindow();
            if (selector.ShowDialog() == true && selector.SelectedRegion is { IsValid: true } selected)
            {
                _captureRegion = selected;
                UpdateRegionText();
                StatusText.Text = "监控区域已更新。";
                await SaveSettingsAsync();
            }
            else
            {
                StatusText.Text = "已取消框选；监控保持停止。";
            }
        }
        catch (Exception exception)
        {
            SystemSounds.Exclamation.Play();
            StatusText.Text = $"无法框选监控区域：{exception.Message}";
        }
        finally
        {
            _isSelectingRegion = false;
            Show();
            WindowState = WindowState.Normal;
            Activate();
        }
    }

    private async void OnStartMonitoring(object sender, RoutedEventArgs e)
    {
        var template = TargetAffixTextBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(template))
        {
            ShowValidationMessage("请先粘贴 PoEDB 的完整目标词缀。");
            return;
        }

        if (_captureRegion is not { IsValid: true } region)
        {
            ShowValidationMessage("请先框选装备提示框所在区域。");
            return;
        }

        try
        {
            _ = new FullLineAffixMatcher(template);
            EnsureMonitorCreated();
            _alertService.Acknowledge();
            AcknowledgeButton.IsEnabled = false;
            var session = Interlocked.Increment(ref _nextMonitoringSession);
            Interlocked.Exchange(ref _activeMonitoringSession, session);
            try
            {
                _monitor!.Start(template, region);
                if (_monitor.State == MonitorState.Running &&
                    Interlocked.Read(ref _activeMonitoringSession) == session)
                {
                    ShowMonitoringHud(template, region);
                    StartButton.IsEnabled = false;
                    StopButton.IsEnabled = true;
                    StatusText.Foreground = new SolidColorBrush(Color.FromRgb(212, 213, 216));
                    StatusText.Text = "正在监控；切回游戏后保持装备提示框位于所选区域。";
                    WindowState = WindowState.Minimized;
                }
            }
            catch
            {
                _ = Interlocked.CompareExchange(ref _activeMonitoringSession, 0, session);
                throw;
            }

            await SaveSettingsAsync(session);
        }
        catch (Exception exception)
        {
            Interlocked.Exchange(ref _activeMonitoringSession, 0);
            await StopMonitorAsync(showIdleStatus: false);
            ShowValidationMessage($"无法开始监控：{exception.Message}");
        }
    }

    private async void OnStopMonitoring(object sender, RoutedEventArgs e) =>
        await StopMonitorAsync(showIdleStatus: true);

    private async void OnAnalyzeScreenshot(object sender, RoutedEventArgs e)
    {
        await StopMonitorAsync(showIdleStatus: false);
        var dialog = new OpenFileDialog
        {
            Title = "选择 POE 截图",
            Filter = "图片 (*.png;*.jpg;*.jpeg)|*.png;*.jpg;*.jpeg|所有文件 (*.*)|*.*",
            CheckFileExists = true,
        };

        if (dialog.ShowDialog(this) != true)
        {
            StatusText.Text = "已取消截图分析；监控保持停止。";
            return;
        }

        StatusText.Text = "正在分析截图……";
        await Task.Yield();

        try
        {
            // The replay loader is kept separate from live capture so the exact same OCR
            // and matcher can be regression-tested against recorded screenshots.
            await AnalyzeScreenshotAsync(dialog.FileName);
        }
        catch (Exception exception)
        {
            SystemSounds.Exclamation.Play();
            StatusText.Text = $"截图分析失败：{exception.Message}";
        }
    }

    private async void OnTestAlert(object sender, RoutedEventArgs e)
    {
        await StopMonitorAsync(showIdleStatus: false);
        _alertService.Trigger("告警自检：这是红色命中提示", _captureRegion);
        AcknowledgeButton.IsEnabled = true;
        StatusText.Foreground = Brushes.Red;
        StatusText.Text = "告警自检已触发；Ctrl + Shift + F12 可关闭。";
    }

    private void OnAcknowledgeAlert(object sender, RoutedEventArgs e) => AcknowledgeAlert();

    private void AcknowledgeAlert()
    {
        _alertService.Acknowledge();
    }

    private void OnAlertAcknowledged(object? sender, EventArgs e)
    {
        if (!Dispatcher.CheckAccess())
        {
            _ = Dispatcher.BeginInvoke(() => OnAlertAcknowledged(sender, e));
            return;
        }

        AcknowledgeButton.IsEnabled = false;
        StatusText.Foreground = new SolidColorBrush(Color.FromRgb(212, 213, 216));
        StatusText.Text = "告警已确认，监控保持停止；检查装备后可重新开始。";
    }

    private void EnsureMonitorCreated()
    {
        if (_monitor is not null)
        {
            return;
        }

        var recognizer = new WindowsOcrRecognizer(scale: 1);
        _monitor = new AffixMonitor(new GdiScreenCapture(), recognizer);
        _monitor.SnapshotChanged += OnMonitorSnapshotChanged;
        _monitor.AffixDetected += OnAffixDetected;
    }

    private void OnMonitorSnapshotChanged(object? sender, MonitorSnapshot snapshot)
    {
        if (_isClosing)
        {
            return;
        }

        var now = Stopwatch.GetTimestamp();
        var elapsedSinceUpdate = Stopwatch.GetElapsedTime(Interlocked.Read(ref _lastUiUpdateTimestamp), now);
        if (snapshot.State == MonitorState.Running && elapsedSinceUpdate < TimeSpan.FromMilliseconds(200))
        {
            return;
        }

        Interlocked.Exchange(ref _lastUiUpdateTimestamp, now);
        _ = Dispatcher.BeginInvoke(() => ApplySnapshot(snapshot));
    }

    private void ApplySnapshot(MonitorSnapshot snapshot)
    {
        if (_isClosing || (_monitor is not null && _monitor.State != snapshot.State))
        {
            return;
        }

        LatencyText.Text = snapshot.ScanCount == 0
            ? "尚无扫描数据"
            : $"扫描 #{snapshot.ScanCount}   截屏 {snapshot.CaptureElapsed.TotalMilliseconds:F1} ms   " +
              $"OCR {snapshot.OcrElapsed.TotalMilliseconds:F1} ms" +
              (snapshot.OcrWasCached ? "（画面未变）" : string.Empty);

        if (snapshot.LastLines.Count > 0)
        {
            OcrOutputTextBox.Text = string.Join(Environment.NewLine, snapshot.LastLines);
        }

        switch (snapshot.State)
        {
            case MonitorState.Running:
                StatusText.Text = snapshot.LastLines.Count == 0
                    ? "正在监控；当前没有蓝色词缀，灰色浮窗会保持显示直到你手动停止。"
                    : "正在监控；右上角灰色浮窗显示当前目标和运行时间。";
                break;
            case MonitorState.MatchFound:
                HideMonitoringHud();
                StartButton.IsEnabled = true;
                StopButton.IsEnabled = false;
                break;
            case MonitorState.Faulted:
                Interlocked.Exchange(ref _activeMonitoringSession, 0);
                HideMonitoringHud();
                StartButton.IsEnabled = true;
                StopButton.IsEnabled = false;
                StatusText.Foreground = new SolidColorBrush(Color.FromRgb(212, 213, 216));
                StatusText.Text = $"监控已停止：{snapshot.Detail}";
                SystemSounds.Hand.Play();
                WindowState = WindowState.Normal;
                Show();
                Activate();
                break;
            case MonitorState.Idle:
                Interlocked.Exchange(ref _activeMonitoringSession, 0);
                HideMonitoringHud();
                StartButton.IsEnabled = true;
                StopButton.IsEnabled = false;
                break;
        }
    }

    private void OnAffixDetected(object? sender, AffixDetectedEventArgs e)
    {
        var session = Interlocked.Read(ref _activeMonitoringSession);
        if (_isClosing || session == 0)
        {
            return;
        }

        _ = Dispatcher.BeginInvoke(DispatcherPriority.Send, new Action(() =>
        {
            if (_isClosing ||
                Interlocked.Read(ref _activeMonitoringSession) != session ||
                _monitor?.State != MonitorState.MatchFound)
            {
                return;
            }

            _ = Interlocked.CompareExchange(ref _activeMonitoringSession, 0, session);
            HideMonitoringHud();
            _alertService.Trigger(e.Match.OriginalText, _captureRegion);
            StatusText.Foreground = Brushes.Red;
            StatusText.Text = "目标整句已命中；监控已锁停，先检查装备再确认。";
            StartButton.IsEnabled = true;
            StopButton.IsEnabled = false;
            AcknowledgeButton.IsEnabled = true;
        }));
    }

    private async Task StopMonitorAsync(bool showIdleStatus)
    {
        Interlocked.Exchange(ref _activeMonitoringSession, 0);
        HideMonitoringHud();
        if (_monitor is not null)
        {
            await _monitor.StopAsync();
        }

        StartButton.IsEnabled = true;
        StopButton.IsEnabled = false;
        if (showIdleStatus)
        {
            StatusText.Text = "监控已停止。";
        }
    }

    private void ShowMonitoringHud(string targetAffix, ScreenRegion region)
    {
        if (_isClosing || _monitor?.State != MonitorState.Running || _alertService.IsActive)
        {
            return;
        }

        try
        {
            _monitoringHud ??= new MonitoringHudWindow();
            _monitoringHud.ShowMonitoring(targetAffix, region);
        }
        catch (InvalidOperationException)
        {
            _monitoringHud = new MonitoringHudWindow();
            _monitoringHud.ShowMonitoring(targetAffix, region);
        }
    }

    private void HideMonitoringHud() => _monitoringHud?.HideMonitoring();

    private void UpdateCanonicalPreview()
    {
        if (!IsInitialized || CanonicalPreviewText is null || TargetAffixTextBox is null)
        {
            return;
        }

        var canonical = AffixCanonicalizer.Normalize(TargetAffixTextBox.Text);
        CanonicalPreviewText.Text = canonical.HasWords
            ? $"实际匹配：{canonical.Text}"
            : "等待输入完整词缀；所有非数字词都会参与匹配。";
    }

    private void UpdateRegionText()
    {
        RegionText.Text = _captureRegion is { IsValid: true } region
            ? $"已选择：{region}"
            : "尚未选择。游戏内悬停装备后按 Ctrl + Shift + F11 最方便。";
    }

    private void ShowValidationMessage(string message)
    {
        StatusText.Foreground = new SolidColorBrush(Color.FromRgb(212, 213, 216));
        StatusText.Text = message;
        SystemSounds.Exclamation.Play();
    }

    private async Task AnalyzeScreenshotAsync(string path)
    {
        var template = TargetAffixTextBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(template))
        {
            throw new InvalidOperationException("请先粘贴完整目标词缀。");
        }

        using var recognizer = new WindowsOcrRecognizer(scale: 1);
        var analyzer = new ScreenshotAffixAnalyzer(recognizer);
        ScreenshotAffixAnalysis analysis;
        try
        {
            analysis = await analyzer.AnalyzeAsync(path, template, _captureRegion);
        }
        catch (ArgumentOutOfRangeException) when (_captureRegion is not null)
        {
            // A crop selected on a different monitor layout may not fit an archived image.
            // Falling back keeps screenshot replay useful; the OCR output makes misses visible.
            analysis = await analyzer.AnalyzeAsync(path, template);
        }

        OcrOutputTextBox.Text = analysis.Lines.Count == 0
            ? "没有识别到蓝色词缀文字。"
            : string.Join(Environment.NewLine, analysis.Lines);
        LatencyText.Text =
            $"读取 {analysis.ImageLoadElapsed.TotalMilliseconds:F1} ms   " +
            $"预处理 {analysis.PreprocessingElapsed.TotalMilliseconds:F1} ms   " +
            $"OCR {analysis.RecognitionElapsed.TotalMilliseconds:F1} ms";

        if (analysis.Match is { } match)
        {
            _alertService.Trigger(match.OriginalText, _captureRegion);
            AcknowledgeButton.IsEnabled = true;
            StatusText.Foreground = Brushes.Red;
            StatusText.Text = "截图中命中目标整句；已触发与实战相同的锁定告警。";
        }
        else
        {
            StatusText.Foreground = new SolidColorBrush(Color.FromRgb(212, 213, 216));
            StatusText.Text = "截图分析完成：未命中目标整句。可在下方检查 OCR 物理行。";
        }
    }

    private async Task SaveSettingsAsync(long? expectedMonitoringSession = null)
    {
        if (!_isLoaded)
        {
            return;
        }

        try
        {
            await _settingsStore.SaveAsync(new AppSettings
            {
                TargetAffix = TargetAffixTextBox.Text.Trim(),
                CaptureRegion = _captureRegion,
                OcrScale = 1,
            });
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
        {
            if (expectedMonitoringSession is { } expectedSession &&
                (Interlocked.Read(ref _activeMonitoringSession) != expectedSession ||
                 _monitor?.State != MonitorState.Running))
            {
                return;
            }

            StatusText.Text = $"设置无法保存，但本次仍可使用：{exception.Message}";
        }
    }

    private async void OnClosing(object? sender, CancelEventArgs e)
    {
        if (_closeReady)
        {
            return;
        }

        e.Cancel = true;
        if (_isClosing)
        {
            return;
        }

        _isClosing = true;
        IsEnabled = false;
        // Fail open first: a blocking alert must disappear before its fallback hotkey is
        // unregistered or any asynchronous shutdown work begins.
        _alertService.Acknowledged -= OnAlertAcknowledged;
        _alertService.Dispose();
        _monitoringHud?.CloseFromOwner();
        _monitoringHud = null;
        _acknowledgementHotKey?.Dispose();
        _acknowledgementHotKey = null;
        _selectionHotKey?.Dispose();
        _selectionHotKey = null;
        try
        {
            await SaveSettingsAsync();
            if (_monitor is not null)
            {
                _monitor.SnapshotChanged -= OnMonitorSnapshotChanged;
                _monitor.AffixDetected -= OnAffixDetected;
                await _monitor.DisposeAsync();
            }
        }
        catch (Exception exception)
        {
            Debug.WriteLine($"POE Alarm shutdown cleanup failed: {exception}");
        }
        finally
        {
            _closeReady = true;
            Close();
        }
    }
}
