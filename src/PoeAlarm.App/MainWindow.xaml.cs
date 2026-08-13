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
using PoeAlarm.App.Localization;
using PoeAlarm.App.Monitoring;
using PoeAlarm.App.Monitoring.Policies;
using PoeAlarm.App.Recognition;
using PoeAlarm.App.Replay;
using PoeAlarm.App.RulesUi;
using PoeAlarm.App.Selection;
using PoeAlarm.Core.Matching;
using PoeAlarm.Core.Rules;

namespace PoeAlarm.App;

public partial class MainWindow : Window
{
    private readonly SettingsStore _settingsStore;
    private readonly IAlertService _alertService;
    private AffixMonitor? _monitor;
    private MonitoringHudWindow? _monitoringHud;
    private GlobalHotKeyRegistration? _acknowledgementHotKey;
    private GlobalHotKeyRegistration? _selectionHotKey;
    private GlobalHotKeyRegistration? _startMonitoringHotKeyRegistration;
    private ScreenRegion? _captureRegion;
    private AppSettings _settings = new AppSettings().Normalize();
    private GameProfile _selectedGameProfile = GameProfile.Poe1;
    private RuleEditorMode _ruleEditorMode = RuleEditorMode.Quick;
    private MonitoringPolicyId _monitoringPolicy = MonitoringPolicyId.Fast;
    private RuleSetDefinition? _structuredRuleSet;
    private HudPlacement _hudPlacement = HudPlacement.Default;
    private string _uiLanguage = UiText.DefaultLanguageCode;
    private string? _customAlertSoundPath;
    private StartMonitoringHotKey _startMonitoringHotKey =
        StartMonitoringHotKey.Parse(null);
    private bool _keepHudVisible = true;
    private bool _allowOverlayCapture = true;
    private bool _isHudPlacementMode;
    private bool _isLoaded;
    private bool _isApplyingProfileSettings;
    private bool _isSelectingRegion;
    private bool _isStartingMonitoring;
    private bool _guardedInputProtectionFailed;
    private bool _isClosing;
    private bool _closeReady;
    private long _lastUiUpdateTimestamp;
    private long _nextMonitoringSession;
    private long _activeMonitoringSession;

    public MainWindow()
        : this(new SettingsStore())
    {
    }

    internal MainWindow(SettingsStore settingsStore)
    {
        InitializeComponent();
        _settingsStore = settingsStore ?? throw new ArgumentNullException(nameof(settingsStore));
        _alertService = new LatchedAlertService();
        _alertService.Acknowledged += OnAlertAcknowledged;
        _alertService.GuardedContinueRequested += OnGuardedContinueRequested;
        _alertService.GuardedStopRequested += OnGuardedStopRequested;
        _alertService.GuardedFailOpen += OnGuardedFailOpen;
    }

    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        var settings = await _settingsStore.LoadAsync();
        _settings = settings;
        _selectedGameProfile = settings.SelectedGameProfile;
        _uiLanguage = UiText.NormalizeLanguageCode(settings.UiLanguage);
        var strings = UiText.Use(_uiLanguage);
        _keepHudVisible = settings.KeepHudVisible;
        _allowOverlayCapture = settings.AllowOverlayCapture;
        _hudPlacement = settings.HudPlacement?.Sanitize() ?? HudPlacement.Default;
        _customAlertSoundPath = !string.IsNullOrWhiteSpace(settings.CustomAlertSoundPath) &&
                                LoopingAlertSound.TryValidateCustomWaveFile(
                                    settings.CustomAlertSoundPath,
                                    out _)
            ? Path.GetFullPath(settings.CustomAlertSoundPath)
            : null;
        _startMonitoringHotKey = StartMonitoringHotKey.Parse(settings.StartMonitoringHotKey);

        ApplyUiStrings(strings);
        GameProfileComboBox.SelectedValue = _selectedGameProfile.ToString();
        ApplyGameProfileSettings(CurrentProfileSettings);
        UiLanguageComboBox.SelectedValue = _uiLanguage;
        KeepHudVisibleCheckBox.IsChecked = _keepHudVisible;
        AllowScreenRecordingCheckBox.IsChecked = _allowOverlayCapture;
        StartHotKeyComboBox.SelectedValue = _startMonitoringHotKey.SettingValue;
        _alertService.Configure(
            _customAlertSoundPath,
            excludeOverlayFromCapture: !_allowOverlayCapture);
        UpdateRegionText();
        UpdateCanonicalPreview();
        UpdateCustomSoundText();
        _isLoaded = true;
        TryRegisterStartMonitoringHotKey(_startMonitoringHotKey, reportFailure: true);
        ShowIdleHudIfEnabled();
        if (_settingsStore.IsReadOnly)
        {
            SetStatus(
                UiText.Current.FutureSettingsReadOnly(
                    _settingsStore.DetectedSchemaVersion ?? 0),
                UiStatusKind.Warning);
        }
    }

    private async void OnGameProfileChanged(
        object sender,
        System.Windows.Controls.SelectionChangedEventArgs e)
    {
        if (!_isLoaded || _isClosing || _isApplyingProfileSettings)
        {
            return;
        }

        if (_monitor?.State == MonitorState.Running ||
            _alertService.IsActive ||
            _alertService.IsInputGuardActive)
        {
            GameProfileComboBox.SelectedValue = _selectedGameProfile.ToString();
            return;
        }

        var selected = ParseGameProfile(GameProfileComboBox.SelectedValue?.ToString());
        if (selected == _selectedGameProfile)
        {
            return;
        }

        CaptureCurrentProfileSettings();
        await ReleaseMonitorAsync();
        _selectedGameProfile = selected;
        ApplyGameProfileSettings(CurrentProfileSettings);
        UpdateGameProfileUi();
        ShowIdleHudIfEnabled();
        SetStatus(
            UiText.Current.SwitchedGameProfile(
                selected == GameProfile.Poe2
                    ? UiText.Current.Poe2GameProfile
                    : UiText.Current.Poe1GameProfile),
            UiStatusKind.Neutral);
        await SaveSettingsAsync();
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
            SetStatus(UiText.Current.GlobalAcknowledgeHotkeyUnavailable(exception.Message));
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
            SetStatus(UiText.Current.GlobalSelectionHotkeyUnavailable(exception.Message));
        }
    }

    private void OnPasteTemplate(object sender, RoutedEventArgs e)
    {
        if (Clipboard.ContainsText())
        {
            TargetAffixTextBox.Text = Clipboard.GetText().Trim();
        }
    }

    private void OnTargetAffixChanged(object sender, System.Windows.Controls.TextChangedEventArgs e)
    {
        UpdateCanonicalPreview();
        if (_isLoaded && _monitor?.State != MonitorState.Running && !_alertService.IsActive)
        {
            ShowIdleHudIfEnabled();
        }
    }

    private async void OnRuleModeChanged(
        object sender,
        System.Windows.Controls.SelectionChangedEventArgs e)
    {
        // ComboBox raises SelectionChanged while InitializeComponent is still constructing the
        // visual tree. At that point alert/HUD services and sibling controls are not ready.
        if (!_isLoaded || _isApplyingProfileSettings || RuleModeComboBox is null)
        {
            return;
        }

        if (_monitor?.State == MonitorState.Running ||
            _alertService.IsActive ||
            _alertService.IsInputGuardActive ||
            _alertService.IsGuardedAlertActive)
        {
            RuleModeComboBox.SelectedValue = _ruleEditorMode.ToString();
            return;
        }

        _ruleEditorMode = ParseRuleEditorMode(RuleModeComboBox.SelectedValue?.ToString());
        UpdateRuleEditorUi();
        ShowIdleHudIfEnabled();
        if (_isLoaded && !_isClosing)
        {
            await SaveSettingsAsync();
        }
    }

    private async void OnMonitoringPolicyChanged(
        object sender,
        System.Windows.Controls.SelectionChangedEventArgs e)
    {
        if (!_isLoaded || _isApplyingProfileSettings || MonitoringPolicyComboBox is null)
        {
            return;
        }

        if (_monitor?.State == MonitorState.Running ||
            _alertService.IsActive || _alertService.IsInputGuardActive ||
            _alertService.IsGuardedAlertActive)
        {
            MonitoringPolicyComboBox.SelectedValue = _monitoringPolicy.ToString();
            return;
        }

        _monitoringPolicy = ParseMonitoringPolicy(
            MonitoringPolicyComboBox.SelectedValue?.ToString());
        UpdateMonitoringPolicyUi();
        ShowIdleHudIfEnabled();
        await SaveSettingsAsync();
    }

    private async void OnEditStructuredRules(object sender, RoutedEventArgs e)
    {
        if (_isClosing || _monitor?.State == MonitorState.Running ||
            _alertService.IsActive || _alertService.IsInputGuardActive)
        {
            return;
        }

        var edited = StructuredRuleEditorWindow.Edit(
            this,
            _structuredRuleSet,
            string.Equals(_uiLanguage, "en", StringComparison.OrdinalIgnoreCase),
            SelectedMaximumLineSpan);
        if (edited is null)
        {
            return;
        }

        // The editor returns only a successfully compiled replacement. Cancelling never mutates
        // the previously saved definition.
        _structuredRuleSet = edited;
        _ruleEditorMode = RuleEditorMode.Structured;
        RuleModeComboBox.SelectedValue = _ruleEditorMode.ToString();
        UpdateRuleEditorUi();
        ShowIdleHudIfEnabled();
        if (_isLoaded)
        {
            await SaveSettingsAsync();
        }
    }

    private void OnShowHelp(object sender, RoutedEventArgs e)
    {
        var help = new HelpWindow { Owner = this };
        _ = help.ShowDialog();
    }

    private async void OnUiLanguageChanged(
        object sender,
        System.Windows.Controls.SelectionChangedEventArgs e)
    {
        if (!_isLoaded || _isClosing)
        {
            return;
        }

        _uiLanguage = UiText.NormalizeLanguageCode(
            UiLanguageComboBox.SelectedValue?.ToString());
        var strings = UiText.Use(_uiLanguage);
        ApplyUiStrings(strings);
        _monitoringHud?.ApplyLanguage();
        _alertService.Configure(
            _customAlertSoundPath,
            excludeOverlayFromCapture: !_allowOverlayCapture);
        await SaveSettingsAsync();
    }

    private async void OnHudPreferenceChanged(object sender, RoutedEventArgs e)
    {
        if (!_isLoaded || _isClosing)
        {
            return;
        }

        _keepHudVisible = KeepHudVisibleCheckBox.IsChecked == true;
        if (_monitor?.State == MonitorState.Running)
        {
            ShowMonitoringHud(ActiveRuleDisplayText, _captureRegion!.Value);
        }
        else if (_keepHudVisible)
        {
            ShowIdleHudIfEnabled();
        }
        else if (!_isHudPlacementMode)
        {
            _monitoringHud?.HideHud();
        }

        await SaveSettingsAsync();
    }

    private async void OnPositionHud(object sender, RoutedEventArgs e)
    {
        if (!_isLoaded || _isClosing)
        {
            return;
        }

        if (_isHudPlacementMode)
        {
            _monitoringHud?.EndPlacement();
            return;
        }

        await StopMonitorAsync(showIdleStatus: false);
        var hud = EnsureHudCreated();
        _isHudPlacementMode = true;
        PositionHudButton.Content = UiText.Current.FinishHudPosition;
        hud.BeginPlacement(ActiveRuleDisplayText, _captureRegion);
        SetStatus(UiText.Current.HudPlacementStarted, UiStatusKind.Neutral);
    }

    private async void OnChooseCustomSound(object sender, RoutedEventArgs e)
    {
        if (!_isLoaded || _isClosing)
        {
            return;
        }

        var dialog = new OpenFileDialog
        {
            Title = UiText.Current.ChooseWave,
            Filter = "WAV (*.wav)|*.wav|All files (*.*)|*.*",
            CheckFileExists = true,
        };
        if (dialog.ShowDialog(this) != true)
        {
            return;
        }

        if (!LoopingAlertSound.TryValidateCustomWaveFile(dialog.FileName, out var error))
        {
            ShowValidationMessage($"{UiText.Current.InvalidWave} {error}");
            return;
        }

        _customAlertSoundPath = Path.GetFullPath(dialog.FileName);
        _alertService.Configure(
            _customAlertSoundPath,
            excludeOverlayFromCapture: !_allowOverlayCapture);
        UpdateCustomSoundText();
        SetStatus(UiText.Current.SoundSelected, UiStatusKind.Neutral);
        await SaveSettingsAsync();
    }

    private async void OnResetAlertSound(object sender, RoutedEventArgs e)
    {
        if (!_isLoaded || _isClosing)
        {
            return;
        }

        _customAlertSoundPath = null;
        _alertService.Configure(
            customWavePath: null,
            excludeOverlayFromCapture: !_allowOverlayCapture);
        UpdateCustomSoundText();
        SetStatus(UiText.Current.SoundReset, UiStatusKind.Neutral);
        await SaveSettingsAsync();
    }

    private async void OnCapturePreferenceChanged(object sender, RoutedEventArgs e)
    {
        if (!_isLoaded || _isClosing)
        {
            return;
        }

        _allowOverlayCapture = AllowScreenRecordingCheckBox.IsChecked == true;
        _monitoringHud?.SetExcludeFromCapture(!_allowOverlayCapture);
        _alertService.Configure(
            _customAlertSoundPath,
            excludeOverlayFromCapture: !_allowOverlayCapture);
        await SaveSettingsAsync();
    }

    private async void OnOcrLanguageChanged(
        object sender,
        System.Windows.Controls.SelectionChangedEventArgs e)
    {
        if (!_isLoaded || _isClosing || _isApplyingProfileSettings)
        {
            return;
        }

        await StopMonitorAsync(showIdleStatus: false);
        await ReleaseMonitorAsync();
        UpdateRuleEditorUi();
        SetStatus(
            SelectedOcrLanguage == OcrRecognitionLanguage.TraditionalChinese
                ? UiText.Current.SwitchedChineseOcr
                : UiText.Current.SwitchedEnglishOcr,
            UiStatusKind.Neutral);
        await SaveSettingsAsync();
    }

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
                SetStatus(UiText.Current.RegionUpdated, UiStatusKind.Neutral);
                ShowIdleHudIfEnabled();
                await SaveSettingsAsync();
            }
            else
            {
                SetStatus(UiText.Current.RegionSelectionCancelled, UiStatusKind.Neutral);
            }
        }
        catch (Exception exception)
        {
            SystemSounds.Exclamation.Play();
            SetStatus(UiText.Current.RegionSelectionFailed(exception.Message), UiStatusKind.Warning);
        }
        finally
        {
            _isSelectingRegion = false;
            Show();
            WindowState = WindowState.Normal;
            Activate();
        }
    }

    private async void OnStartMonitoring(object sender, RoutedEventArgs e) =>
        await StartMonitoringAsync();

    private async Task StartMonitoringAsync()
    {
        if (_isStartingMonitoring || _isClosing || _isSelectingRegion ||
            _alertService.IsActive || _alertService.IsInputGuardActive ||
            _alertService.IsGuardedAlertActive ||
            _monitor?.State == MonitorState.Running)
        {
            return;
        }

        _isStartingMonitoring = true;
        try
        {
            var isStructured = _ruleEditorMode == RuleEditorMode.Structured;
            var isGuarded = _monitoringPolicy == MonitoringPolicyId.Guarded;
            var template = TargetAffixTextBox.Text.Trim();
            CompiledRuleSet? compiledRules = null;
            if (isStructured)
            {
                if (_structuredRuleSet is null)
                {
                    ShowValidationMessage(UiText.Current.NeedStructuredRules);
                    return;
                }

                try
                {
                    compiledRules = RuleCompiler.Compile(
                        _structuredRuleSet,
                        SelectedMaximumLineSpan);
                }
                catch (RuleValidationException exception)
                {
                    ShowValidationMessage(
                        UiText.Current.CannotStartMonitoring(exception.Message));
                    return;
                }
            }
            else if (string.IsNullOrWhiteSpace(template))
            {
                ShowValidationMessage(UiText.Current.NeedTarget);
                return;
            }

            if (_captureRegion is not { IsValid: true } region)
            {
                ShowValidationMessage(UiText.Current.NeedRegion);
                return;
            }

            try
            {
                var maximumLineSpan = SelectedMaximumLineSpan;
                if (!isStructured)
                {
                    // Keep the verified 0.6.1 quick matcher path exactly target-aware. It is not
                    // wrapped into a one-condition structured rule.
                    _ = new FullLineAffixMatcher(template, maximumLineSpan);
                    if (isGuarded)
                    {
                        compiledRules = CompileGuardedQuickRule(template, maximumLineSpan);
                    }
                }
                EnsureMonitorCreated();
                if (isGuarded && !_monitor!.SupportsGuardedMonitoring)
                {
                    ShowValidationMessage(UiText.Current.GuardedMonitoringUnsupported);
                    return;
                }
                if (compiledRules is not null &&
                    _monitor!.StructuredRuleSupport == StructuredRuleOcrSupport.Unsupported)
                {
                    ShowValidationMessage(UiText.Current.StructuredOcrUnsupported);
                    return;
                }

                _alertService.Acknowledge();
                _alertService.Prepare(
                    preparePendingInputGuard:
                        isGuarded ||
                        SelectedOcrLanguage == OcrRecognitionLanguage.TraditionalChinese ||
                        _selectedGameProfile == GameProfile.Poe2);
                _guardedInputProtectionFailed = false;
                AcknowledgeButton.IsEnabled = false;
                var session = Interlocked.Increment(ref _nextMonitoringSession);
                Interlocked.Exchange(ref _activeMonitoringSession, session);
                try
                {
                    if (isGuarded)
                    {
                        _monitor!.Start(
                            compiledRules!,
                            region,
                            new GuardedMonitoringPolicy());
                        if (_guardedInputProtectionFailed ||
                            _monitor.State != MonitorState.Running)
                        {
                            throw new InvalidOperationException(
                                UiText.Current.PendingGuardArmFailed);
                        }
                    }
                    else if (compiledRules is not null)
                    {
                        _monitor!.Start(compiledRules, region);
                    }
                    else
                    {
                        _monitor!.Start(template, region, maximumLineSpan);
                    }
                    if (_monitor.State == MonitorState.Running &&
                        Interlocked.Read(ref _activeMonitoringSession) == session)
                    {
                        ShowMonitoringHud(ActiveRuleDisplayText, region);
                        StartButton.IsEnabled = false;
                        StopButton.IsEnabled = true;
                        SetProfileSelectorsEnabled(false);
                        SetStatus(
                            isGuarded
                                ? UiText.Current.GuardedMonitoringStarted
                                : UiText.Current.MonitoringStarted,
                            UiStatusKind.Monitoring);
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
                ShowValidationMessage(UiText.Current.CannotStartMonitoring(exception.Message));
            }
        }
        finally
        {
            _isStartingMonitoring = false;
        }
    }

    private async void OnStopMonitoring(object sender, RoutedEventArgs e) =>
        await StopMonitorAsync(showIdleStatus: true);

    private async void OnAnalyzeScreenshot(object sender, RoutedEventArgs e)
    {
        await StopMonitorAsync(showIdleStatus: false);
        var dialog = new OpenFileDialog
        {
            Title = UiText.Current.ScreenshotDialogTitle,
            Filter = UiText.Current.ScreenshotDialogFilter,
            CheckFileExists = true,
        };

        if (dialog.ShowDialog(this) != true)
        {
            SetStatus(UiText.Current.ScreenshotCancelled, UiStatusKind.Neutral);
            return;
        }

        SetStatus(UiText.Current.ScreenshotAnalyzing, UiStatusKind.Neutral);
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
            SetStatus(UiText.Current.ScreenshotFailed(exception.Message), UiStatusKind.Warning);
        }
    }

    private async void OnTestAlert(object sender, RoutedEventArgs e)
    {
        await StopMonitorAsync(showIdleStatus: false);
        _monitoringHud?.HideHud();
        _alertService.Trigger(UiText.Current.TestAlertDetectedText, _captureRegion);
        SetProfileSelectorsEnabled(false);
        AcknowledgeButton.IsEnabled = true;
        SetStatus(UiText.Current.TestAlertTriggered, UiStatusKind.Alert);
    }

    private void OnAcknowledgeAlert(object sender, RoutedEventArgs e) => AcknowledgeAlert();

    private void AcknowledgeAlert()
    {
        if (_alertService.IsInputGuardActive && !_alertService.IsActive)
        {
            // Ctrl+Shift+F12 is also the fail-open escape hatch for a pending guarded OCR
            // decision. Cancel the monitoring session before acknowledging so the next
            // progressive slice cannot immediately arm the guard again.
            Interlocked.Exchange(ref _activeMonitoringSession, 0);
            _alertService.Acknowledge();
            _ = StopPendingGuardFromHotkeyAsync();
            return;
        }

        if (_alertService.IsGuardedAlertActive && !_alertService.IsActive)
        {
            Interlocked.Exchange(ref _activeMonitoringSession, 0);
            _alertService.ReleaseGuardedInputGuard();
            _ = StopPendingGuardFromHotkeyAsync();
            return;
        }

        _alertService.Acknowledge();
    }

    private async Task StopPendingGuardFromHotkeyAsync()
    {
        await StopMonitorAsync(showIdleStatus: false);
        if (_isClosing)
        {
            return;
        }

        SetStatus(UiText.Current.EmergencyGuardReleased, UiStatusKind.Warning);
    }

    private void OnAlertAcknowledged(object? sender, EventArgs e)
    {
        if (!Dispatcher.CheckAccess())
        {
            _ = Dispatcher.BeginInvoke(() => OnAlertAcknowledged(sender, e));
            return;
        }

        AcknowledgeButton.IsEnabled = false;
        SetProfileSelectorsEnabled(true);
        SetStatus(UiText.Current.AlertAcknowledged, UiStatusKind.Idle);
        ShowIdleHudIfEnabled();
    }

    private void EnsureMonitorCreated()
    {
        if (_monitor is not null)
        {
            return;
        }

        var recognizer = CreateOcrRecognizer();
        _monitor = new AffixMonitor(new GdiScreenCapture(), recognizer);
        _monitor.SnapshotChanged += OnMonitorSnapshotChanged;
        _monitor.AffixDetected += OnAffixDetected;
        _monitor.GuardedDecisionChanged += OnGuardedDecisionChanged;
        _monitor.InputGuardRequested += OnInputGuardRequested;
        _monitor.InputGuardReleased += OnInputGuardReleased;
        _monitor.GuardedPassCycleRequested += OnGuardedPassCycleRequested;
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
            ? UiText.Current.NoScanData
            : UiText.Current.ScanMetrics(
                snapshot.ScanCount,
                snapshot.CaptureElapsed.TotalMilliseconds,
                snapshot.OcrElapsed.TotalMilliseconds,
                snapshot.OcrWasCached);

        if (snapshot.LastLines.Count > 0)
        {
            OcrOutputTextBox.Text = string.Join(Environment.NewLine, snapshot.LastLines);
        }

        switch (snapshot.State)
        {
            case MonitorState.Running:
                SetStatus(
                    snapshot.LastLines.Count == 0
                        ? UiText.Current.MonitoringNoBlueText
                        : UiText.Current.MonitoringWithOutput,
                    UiStatusKind.Monitoring);
                break;
            case MonitorState.MatchFound:
                _monitoringHud?.HideHud();
                StartButton.IsEnabled = true;
                StopButton.IsEnabled = false;
                SetProfileSelectorsEnabled(false);
                break;
            case MonitorState.Faulted:
                Interlocked.Exchange(ref _activeMonitoringSession, 0);
                ShowIdleHudIfEnabled();
                StartButton.IsEnabled = true;
                StopButton.IsEnabled = false;
                SetProfileSelectorsEnabled(true);
                SetStatus(
                    UiText.Current.MonitorStoppedWithDetail(snapshot.Detail ?? string.Empty),
                    UiStatusKind.Warning);
                SystemSounds.Hand.Play();
                WindowState = WindowState.Normal;
                Show();
                Activate();
                break;
            case MonitorState.Idle:
                Interlocked.Exchange(ref _activeMonitoringSession, 0);
                ShowIdleHudIfEnabled();
                StartButton.IsEnabled = true;
                StopButton.IsEnabled = false;
                SetProfileSelectorsEnabled(true);
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

        // Claim this session and latch the alert before returning to AffixMonitor. This preserves
        // the already blocking Chinese low-level guard until the real red overlay is visible and
        // can take ownership. If Stop won the CAS, no alert is accepted and the monitor's finally
        // path safely releases the guard.
        if (Interlocked.CompareExchange(ref _activeMonitoringSession, 0, session) != session)
        {
            return;
        }

        var guardedMatch = _monitoringPolicy == MonitoringPolicyId.Guarded;
        var handoffAccepted = guardedMatch
            ? _alertService.TriggerGuardedMatch(e.DetectedText, _captureRegion)
            : TriggerFastAlert(e.DetectedText);
        if (!handoffAccepted)
        {
            _monitor?.FailGuardedSession(
                "The Guarded match alert could not take ownership of input.",
                GuardedFaultReason.InputGuardUnavailable);
            return;
        }
        _ = Dispatcher.BeginInvoke(DispatcherPriority.Send, new Action(() =>
        {
            if (_isClosing || _monitor?.State != MonitorState.MatchFound)
            {
                return;
            }

            _monitoringHud?.HideHud();
            SetStatus(
                e.RuleEvaluation is null
                    ? UiText.Current.TargetMatched
                    : UiText.Current.StructuredTargetMatched,
                UiStatusKind.Alert);
            StartButton.IsEnabled = true;
            StopButton.IsEnabled = false;
            AcknowledgeButton.IsEnabled = true;
        }));

        bool TriggerFastAlert(string detectedText)
        {
            _alertService.Trigger(detectedText, _captureRegion);
            return true;
        }
    }

    private void OnInputGuardRequested(object? sender, EventArgs e)
    {
        if (!_isClosing && Interlocked.Read(ref _activeMonitoringSession) != 0)
        {
            if (_monitoringPolicy == MonitoringPolicyId.Guarded)
            {
                _alertService.BeginGuardedInputGuard(
                    new GuardedPolicyOptions().DecisionTimeout +
                    TimeSpan.FromMilliseconds(250),
                    _captureRegion);
            }
            else
            {
                _alertService.BeginInputGuard(_captureRegion);
            }
        }
    }

    private void OnInputGuardReleased(object? sender, EventArgs e)
    {
        if (_monitoringPolicy == MonitoringPolicyId.Guarded)
        {
            _alertService.ReleaseGuardedInputGuard();
        }
        else
        {
            _alertService.ReleaseInputGuard();
        }
    }

    private void OnGuardedPassCycleRequested(object? sender, EventArgs e)
    {
        if (_isClosing || Interlocked.Read(ref _activeMonitoringSession) == 0)
        {
            return;
        }

        var options = new GuardedPolicyOptions();
        if (!_alertService.AwaitNextGuardedClick(
                options.DecisionTimeout + TimeSpan.FromMilliseconds(250),
                _captureRegion))
        {
            _guardedInputProtectionFailed = true;
        }
    }

    private void OnGuardedDecisionChanged(object? sender, GuardedDecisionEventArgs e)
    {
        if (_isClosing || Interlocked.Read(ref _activeMonitoringSession) == 0)
        {
            return;
        }

        if (e.Decision == GuardedDecisionKind.NoMatch)
        {
            return;
        }

        if (e.Decision == GuardedDecisionKind.Uncertain)
        {
            var reason = UiText.Current.GuardedUncertaintyReason(e.UncertaintyReason);
            _alertService.ShowGuardedUncertain(
                UiText.Current.GuardedUncertainTitle,
                reason,
                UiText.Current.GuardedUncertainInstruction,
                UiText.Current.GuardedContinue,
                UiText.Current.GuardedStop,
                _captureRegion);
            _ = Dispatcher.BeginInvoke(DispatcherPriority.Send, new Action(() =>
            {
                _monitoringHud?.HideHud();
                SetStatus(reason, UiStatusKind.Warning);
                StartButton.IsEnabled = false;
                StopButton.IsEnabled = true;
            }));
            return;
        }

        if (e.Decision == GuardedDecisionKind.Faulted)
        {
            Interlocked.Exchange(ref _activeMonitoringSession, 0);
            _ = Dispatcher.BeginInvoke(DispatcherPriority.Send, new Action(() =>
            {
                _monitoringHud?.HideHud();
                StartButton.IsEnabled = true;
                StopButton.IsEnabled = false;
                SetProfileSelectorsEnabled(true);
                SetStatus(UiText.Current.GuardedProtectionFailed(e.Detail), UiStatusKind.Warning);
                WindowState = WindowState.Normal;
                Show();
                Activate();
            }));
        }
    }

    private void OnGuardedContinueRequested(object? sender, EventArgs e)
    {
        if (_isClosing || _monitor?.ContinueGuarded() != true)
        {
            return;
        }

        _ = Dispatcher.BeginInvoke(() =>
        {
            if (_monitor?.State == MonitorState.Running)
            {
                ShowMonitoringHud(ActiveRuleDisplayText, _captureRegion!.Value);
                SetStatus(UiText.Current.GuardedContinued, UiStatusKind.Monitoring);
            }
        });
    }

    private void OnGuardedStopRequested(object? sender, EventArgs e)
    {
        if (_isClosing)
        {
            return;
        }

        _ = StopGuardedFromReviewAsync();
    }

    private async Task StopGuardedFromReviewAsync()
    {
        await StopMonitorAsync(showIdleStatus: false);
        if (!_isClosing)
        {
            SetStatus(UiText.Current.GuardedStoppedForReview, UiStatusKind.Warning);
            WindowState = WindowState.Normal;
            Show();
            Activate();
        }
    }

    private void OnGuardedFailOpen(object? sender, GuardedAlertFailOpenEventArgs e)
    {
        _guardedInputProtectionFailed = true;
        var reason = e.Reason == GuardedAlertFailOpenReason.InputGuardTimedOut
            ? GuardedFaultReason.DecisionTimedOut
            : GuardedFaultReason.InputGuardUnavailable;
        _monitor?.FailGuardedSession(e.Detail, reason);
    }

    private async Task StopMonitorAsync(bool showIdleStatus)
    {
        Interlocked.Exchange(ref _activeMonitoringSession, 0);
        // Release a guard immediately instead of waiting for an in-flight OCR call or its
        // fail-open watchdog. A real red alert ignores this guard-only release.
        _alertService.ReleaseInputGuard();
        _alertService.ReleaseGuardedInputGuard();
        if (!_alertService.IsActive && !_alertService.IsGuardedAlertActive)
        {
            ShowIdleHudIfEnabled();
        }
        if (_monitor is not null)
        {
            await _monitor.StopAsync();
        }

        StartButton.IsEnabled = true;
        StopButton.IsEnabled = false;
        SetProfileSelectorsEnabled(
            !_alertService.IsActive && !_alertService.IsGuardedAlertActive);
        if (showIdleStatus)
        {
            SetStatus(UiText.Current.MonitoringStopped, UiStatusKind.Idle);
        }
    }

    private void ShowMonitoringHud(string targetAffix, ScreenRegion region)
    {
        if (_isClosing || !_keepHudVisible ||
            _monitor?.State != MonitorState.Running || _alertService.IsActive ||
            _alertService.IsGuardedAlertActive)
        {
            if (!_keepHudVisible)
            {
                _monitoringHud?.HideHud();
            }

            return;
        }

        try
        {
            var hud = EnsureHudCreated();
            hud.ShowMonitoring(targetAffix, region);
        }
        catch (InvalidOperationException)
        {
            ReplaceHud();
            _monitoringHud!.ShowMonitoring(targetAffix, region);
        }
    }

    private MonitoringHudWindow EnsureHudCreated()
    {
        if (_monitoringHud is not null)
        {
            return _monitoringHud;
        }

        _monitoringHud = new MonitoringHudWindow(
            _hudPlacement,
            excludeFromCapture: !_allowOverlayCapture);
        _monitoringHud.PlacementChanged += OnHudPlacementChanged;
        return _monitoringHud;
    }

    private void ReplaceHud()
    {
        if (_monitoringHud is not null)
        {
            _monitoringHud.PlacementChanged -= OnHudPlacementChanged;
            _monitoringHud.CloseFromOwner();
        }

        _monitoringHud = null;
        _ = EnsureHudCreated();
    }

    private async void OnHudPlacementChanged(object? sender, HudPlacementChangedEventArgs e)
    {
        if (_isClosing)
        {
            return;
        }

        _hudPlacement = e.Placement.Sanitize();
        _isHudPlacementMode = false;
        PositionHudButton.Content = UiText.Current.PositionHud;
        SetStatus(UiText.Current.HudPlacementFinished, UiStatusKind.Neutral);

        if (!_keepHudVisible && _monitor?.State != MonitorState.Running)
        {
            _monitoringHud?.HideHud();
        }

        await SaveSettingsAsync();
    }

    private void ShowIdleHudIfEnabled()
    {
        if (_isClosing || _alertService.IsActive || _alertService.IsGuardedAlertActive ||
            _isHudPlacementMode)
        {
            return;
        }

        if (!_keepHudVisible)
        {
            _monitoringHud?.HideHud();
            return;
        }

        try
        {
            EnsureHudCreated().ShowIdle(ActiveRuleDisplayText, _captureRegion);
        }
        catch (InvalidOperationException)
        {
            ReplaceHud();
            _monitoringHud!.ShowIdle(ActiveRuleDisplayText, _captureRegion);
        }
    }

    private void UpdateCustomSoundText()
    {
        if (_customAlertSoundPath is null)
        {
            CustomSoundText.Text = UiText.Current.BuiltInAlertSound;
            CustomSoundText.ToolTip = null;
            ResetSoundButton.IsEnabled = false;
            return;
        }

        CustomSoundText.Text = Path.GetFileName(_customAlertSoundPath);
        CustomSoundText.ToolTip = _customAlertSoundPath;
        ResetSoundButton.IsEnabled = true;
    }

    private void ApplyUiStrings(UiStrings text)
    {
        Title = text.WindowTitle;
        AppTitleText.Text = text.AppTitle;
        AppSubtitleText.Text = text.AppSubtitle;
        HelpButton.Content = text.HelpButton;
        GameProfileLabelText.Text = text.GameProfileLabel;
        GameProfileComboBox.ToolTip = text.GameProfileToolTip;
        Poe1GameProfileOption.Content = text.Poe1GameProfile;
        Poe2GameProfileOption.Content = text.Poe2GameProfile;
        TargetSectionTitleText.Text = text.TargetSectionTitle.Replace("1  ", string.Empty);
        PasteButton.Content = text.PasteFromClipboard;
        TargetAffixTextBox.ToolTip = text.TargetTextToolTip;
        RuleModeLabelText.Text = text.RuleModeLabel;
        RuleModeComboBox.ToolTip = text.RuleModeToolTip;
        QuickRuleModeOption.Content = text.QuickRuleMode;
        StructuredRuleModeOption.Content = text.StructuredRuleMode;
        EditStructuredRulesButton.Content = text.EditStructuredRules;
        StructuredRuleHintText.Text = text.StructuredRuleHint;
        MonitoringPolicyLabelText.Text = text.MonitoringPolicyLabel;
        MonitoringPolicyComboBox.ToolTip = text.MonitoringPolicyToolTip;
        FastMonitoringPolicyOption.Content = text.FastMonitoringPolicy;
        GuardedMonitoringPolicyOption.Content = text.GuardedMonitoringPolicy;
        OcrLanguageLabelText.Text = text.OcrLanguageLabel;
        OcrLanguageComboBox.ToolTip = text.OcrLanguageToolTip;
        EnglishOcrOption.Content = text.OcrEnglishOption;
        TraditionalChineseOcrOption.Content = text.OcrTraditionalChineseOption;
        RegionSectionTitleText.Text = text.RegionSectionTitle.Replace("2  ", string.Empty);
        SelectRegionButton.Content = text.SelectRegion;
        RunSectionTitleText.Text = text.RunSectionTitle.Replace("3  ", string.Empty);
        StartButton.Content = text.StartMonitoring;
        StopButton.Content = text.StopMonitoring;
        AnalyzeScreenshotButton.Content = text.AnalyzeScreenshot;
        TestAlertButton.Content = text.TestAlert;
        AcknowledgeButton.Content = text.AcknowledgeAlert;
        DiagnosticToolsHeaderText.Text = text.DiagnosticTools;
        StatusSectionTitleText.Text = text.StatusSectionTitle;
        OcrDetailsHeaderText.Text = text.OcrDetails;
        ExperienceSettingsHeaderText.Text = text.ExperienceSettings;
        UiLanguageLabelText.Text = text.UiLanguageLabel;
        StartHotKeyLabelText.Text = text.StartHotKeyLabel;
        SimplifiedChineseUiOption.Content = text.UiLanguageChinese;
        EnglishUiOption.Content = text.UiLanguageEnglish;
        MonitoringHudLabelText.Text = text.LanguageCode == "en"
            ? "Status HUD"
            : "状态浮窗";
        KeepHudVisibleCheckBox.Content = text.KeepHudVisible;
        MonitoringHudDescriptionText.Text = text.HudPreferenceDescription;
        PositionHudButton.Content = _isHudPlacementMode
            ? text.FinishHudPosition
            : text.PositionHud;
        AlertSoundLabelText.Text = text.AlertSoundLabel;
        ChooseSoundButton.Content = text.ChooseWave;
        ResetSoundButton.Content = text.ResetSound;
        ScreenRecordingLabelText.Text = text.ScreenRecordingLabel;
        AllowScreenRecordingCheckBox.Content = text.AllowScreenRecording;
        ScreenRecordingDescriptionText.Text = text.ScreenRecordingDescription;
        ShortcutHintText.Text = text.FooterHint;
        UpdateGameProfileUi();

        UpdateCanonicalPreview();
        UpdateRuleEditorUi();
        UpdateMonitoringPolicyUi();
        UpdateRegionText();
        UpdateCustomSoundText();
        if (!_isLoaded)
        {
            StatusText.Text = text.IdleStatus;
            LatencyText.Text = text.NoScanData;
            OcrOutputTextBox.Text = text.OcrOutputPlaceholder;
            SetStatus(text.IdleStatus, UiStatusKind.Idle);
        }
        else if (_alertService.IsActive)
        {
            SetStatus(
                _ruleEditorMode == RuleEditorMode.Structured
                    ? text.StructuredTargetMatched
                    : text.TargetMatched,
                UiStatusKind.Alert);
        }
        else if (_monitor?.State == MonitorState.Running)
        {
            SetStatus(text.MonitoringWithOutput, UiStatusKind.Monitoring);
        }
        else
        {
            SetStatus(text.IdleStatus, UiStatusKind.Idle);
        }
    }

    private void SetStatus(string message, UiStatusKind kind = UiStatusKind.Neutral)
    {
        StatusText.Text = message;
        var (textBrush, indicatorBrush) = kind switch
        {
            UiStatusKind.Monitoring => (FindBrush("TextPrimaryBrush"), FindBrush("SuccessBrush")),
            UiStatusKind.Alert => (FindBrush("DangerBrush"), FindBrush("DangerBrush")),
            UiStatusKind.Warning => (FindBrush("TextPrimaryBrush"), FindBrush("WarningBrush")),
            UiStatusKind.Idle => (FindBrush("TextPrimaryBrush"), FindBrush("TextTertiaryBrush")),
            _ => (FindBrush("TextPrimaryBrush"), FindBrush("AccentBrush")),
        };
        StatusText.Foreground = textBrush;
        StatusIndicator.Fill = indicatorBrush;
    }

    private Brush FindBrush(string key) =>
        TryFindResource(key) as Brush ?? Brushes.Gray;

    private enum UiStatusKind
    {
        Idle,
        Neutral,
        Monitoring,
        Warning,
        Alert,
    }

    private OcrRecognitionLanguage SelectedOcrLanguage =>
        string.Equals(
            OcrLanguageComboBox.SelectedValue?.ToString(),
            "zh-TW",
            StringComparison.OrdinalIgnoreCase)
            ? OcrRecognitionLanguage.TraditionalChinese
            : OcrRecognitionLanguage.English;

    internal GameProfile SelectedGameProfile => _selectedGameProfile;

    private GameProfileSettings CurrentProfileSettings =>
        (_settings.Profiles ?? new GameProfileSettingsSet()).Get(_selectedGameProfile);

    private int SelectedMaximumLineSpan =>
        _selectedGameProfile == GameProfile.Poe2
            ? FullLineAffixMatcher.MaximumSupportedPhysicalLineSpan
            : FullLineAffixMatcher.MaximumPhysicalLineSpan;

    private static CompiledRuleSet CompileGuardedQuickRule(
        string template,
        int maximumPhysicalLineSpan) =>
        RuleCompiler.Compile(
            new RuleSetDefinition(
                "Quick target",
                [
                    new AcceptableResultGroup(
                        "Quick target",
                        ResultGroupMode.Any,
                        [new AffixCondition("Quick target", template)]),
                ]),
            maximumPhysicalLineSpan);

    private string ActiveRuleDisplayText => _ruleEditorMode == RuleEditorMode.Structured
        ? CreateStructuredRuleSummary(_structuredRuleSet)
        : TargetAffixTextBox.Text.Trim();

    private IOcrRecognizer CreateOcrRecognizer() =>
        SelectedOcrLanguage == OcrRecognitionLanguage.TraditionalChinese
            ? CreateTraditionalChineseRecognizer()
            : _selectedGameProfile == GameProfile.Poe2
                ? new Poe2EnglishRecognizer()
                : new WindowsOcrRecognizer(
                    scale: 1,
                    language: OcrRecognitionLanguage.English);

    private static IOcrRecognizer CreateTraditionalChineseRecognizer()
    {
        try
        {
            return new WindowsChineseOcrRecognizer();
        }
        catch (InvalidOperationException)
        {
            // Keep the application usable on Windows installations without a Chinese OCR
            // capability. The packaged Paddle recognizer remains an offline compatibility path;
            // supported systems never pay its latency or progressive-recovery cost.
            return new PaddleOcrRecognizer();
        }
    }

    private async Task ReleaseMonitorAsync()
    {
        var monitor = _monitor;
        _monitor = null;
        if (monitor is null)
        {
            return;
        }

        monitor.SnapshotChanged -= OnMonitorSnapshotChanged;
        monitor.AffixDetected -= OnAffixDetected;
        monitor.GuardedDecisionChanged -= OnGuardedDecisionChanged;
        monitor.InputGuardRequested -= OnInputGuardRequested;
        monitor.InputGuardReleased -= OnInputGuardReleased;
        monitor.GuardedPassCycleRequested -= OnGuardedPassCycleRequested;
        await monitor.DisposeAsync();
    }

    private void UpdateCanonicalPreview()
    {
        if (!IsInitialized || CanonicalPreviewText is null || TargetAffixTextBox is null)
        {
            return;
        }

        var canonical = AffixCanonicalizer.Normalize(TargetAffixTextBox.Text);
        CanonicalPreviewText.Text = canonical.HasWords
            ? UiText.Current.CanonicalMatch(canonical.Text)
            : UiText.Current.CanonicalEmpty;
    }

    private void UpdateRuleEditorUi()
    {
        if (!IsInitialized || RuleModeComboBox is null || QuickRulePanel is null ||
            StructuredRulePanel is null)
        {
            return;
        }

        var isStructured = _ruleEditorMode == RuleEditorMode.Structured;
        RuleModeComboBox.SelectedValue = _ruleEditorMode.ToString();
        QuickRulePanel.Visibility = isStructured ? Visibility.Collapsed : Visibility.Visible;
        StructuredRulePanel.Visibility = isStructured ? Visibility.Visible : Visibility.Collapsed;
        PasteButton.Visibility = isStructured ? Visibility.Collapsed : Visibility.Visible;
        StructuredRuleSummaryText.Text = CreateStructuredRuleSummary(_structuredRuleSet);
        var structuredHint = UiText.Current.StructuredRuleHint;
        StructuredRuleHintText.Text = structuredHint;
        StructuredRuleHintText.ToolTip = structuredHint;
    }

    private void UpdateMonitoringPolicyUi()
    {
        if (!IsInitialized || MonitoringPolicyComboBox is null ||
            MonitoringPolicyHintText is null)
        {
            return;
        }

        MonitoringPolicyComboBox.SelectedValue = _monitoringPolicy.ToString();
        var isGuarded = _monitoringPolicy == MonitoringPolicyId.Guarded;
        MonitoringPolicyHintText.Text = isGuarded
            ? UiText.Current.GuardedMonitoringHint
            : UiText.Current.FastMonitoringHint;
        MonitoringPolicyHintText.ToolTip = MonitoringPolicyHintText.Text;
        MonitoringPolicyHintText.Foreground = isGuarded
            ? FindBrush("WarningBrush")
            : FindBrush("TextSecondaryBrush");
    }

    private string CreateStructuredRuleSummary(RuleSetDefinition? definition)
    {
        if (definition?.Groups is not { Count: > 0 } groups)
        {
            return UiText.Current.NoStructuredRules;
        }

        var name = string.IsNullOrWhiteSpace(definition.Name)
            ? UiText.Current.StructuredRuleMode
            : definition.Name.Trim();
        var conditionCount = groups.Sum(static group => group?.Conditions?.Count ?? 0);
        return UiText.Current.StructuredRuleSummary(name, groups.Count, conditionCount);
    }

    private void UpdateRegionText()
    {
        RegionText.Text = _captureRegion is { IsValid: true } region
            ? UiText.Current.SelectedRegion(region.ToString())
            : UiText.Current.RegionEmpty;
    }

    private void ApplyGameProfileSettings(GameProfileSettings profile)
    {
        _isApplyingProfileSettings = true;
        try
        {
            TargetAffixTextBox.Text = profile.TargetAffix;
            _ruleEditorMode = profile.RuleEditorMode;
            _monitoringPolicy = profile.MonitoringPolicy;
            _structuredRuleSet = profile.StructuredRuleSet;
            _captureRegion = profile.CaptureRegion is { IsValid: true } region ? region : null;
            OcrLanguageComboBox.SelectedValue = profile.OcrLanguage;
            OcrOutputTextBox.Text = UiText.Current.OcrOutputPlaceholder;
            LatencyText.Text = UiText.Current.NoScanData;
            UpdateCanonicalPreview();
            UpdateRuleEditorUi();
            UpdateMonitoringPolicyUi();
            UpdateRegionText();
        }
        finally
        {
            _isApplyingProfileSettings = false;
        }
    }

    private void TryRegisterStartMonitoringHotKey(
        StartMonitoringHotKey hotKey,
        bool reportFailure)
    {
        _startMonitoringHotKeyRegistration?.Dispose();
        _startMonitoringHotKeyRegistration = null;

        try
        {
            var registration = new GlobalHotKeyRegistration(
                this,
                identifier: 0x504F47,
                hotKey.Modifiers,
                hotKey.Key);
            registration.Pressed += OnStartMonitoringHotKeyPressed;
            _startMonitoringHotKeyRegistration = registration;
        }
        catch (Win32Exception exception)
        {
            if (reportFailure)
            {
                SetStatus(
                    UiText.Current.GlobalStartHotkeyUnavailable(
                        hotKey.SettingValue,
                        exception.Message),
                    UiStatusKind.Warning);
            }
        }
    }

    private async void OnStartMonitoringHotKeyPressed(object? sender, EventArgs e)
    {
        if (_isClosing || _isSelectingRegion || _alertService.IsActive ||
            _alertService.IsInputGuardActive || _alertService.IsGuardedAlertActive ||
            _monitor?.State == MonitorState.Running)
        {
            return;
        }

        await StartMonitoringAsync();
    }

    private async void OnStartHotKeyChanged(
        object sender,
        System.Windows.Controls.SelectionChangedEventArgs e)
    {
        if (!_isLoaded || _isClosing)
        {
            return;
        }

        var requested = StartMonitoringHotKey.Parse(
            StartHotKeyComboBox.SelectedValue?.ToString());
        if (requested == _startMonitoringHotKey)
        {
            return;
        }

        var previous = _startMonitoringHotKey;
        TryRegisterStartMonitoringHotKey(requested, reportFailure: false);
        if (_startMonitoringHotKeyRegistration is null)
        {
            TryRegisterStartMonitoringHotKey(previous, reportFailure: false);
            StartHotKeyComboBox.SelectedValue = previous.SettingValue;
            SetStatus(
                UiText.Current.GlobalStartHotkeyUnavailable(
                    requested.SettingValue,
                    UiText.Current.HotkeyAlreadyInUse),
                UiStatusKind.Warning);
            return;
        }

        _startMonitoringHotKey = requested;
        SetStatus(
            UiText.Current.StartHotkeyUpdated(requested.SettingValue),
            UiStatusKind.Neutral);
        await SaveSettingsAsync();
    }

    private void CaptureCurrentProfileSettings()
    {
        // Keep vNext structured rules intact while the 0.6.1 quick editor is still the only
        // editor rendered by this window. Editing a target, region, or OCR language must never
        // erase a rule set that was loaded from the versioned settings schema.
        var profile = CurrentProfileSettings with
        {
            TargetAffix = TargetAffixTextBox.Text.Trim(),
            CaptureRegion = _captureRegion,
            RuleEditorMode = _ruleEditorMode,
            MonitoringPolicy = _monitoringPolicy,
            StructuredRuleSet = _structuredRuleSet,
            OcrLanguage = SelectedOcrLanguage == OcrRecognitionLanguage.TraditionalChinese
                ? "zh-TW"
                : "en",
        };
        var profiles = (_settings.Profiles ?? new GameProfileSettingsSet())
            .With(_selectedGameProfile, profile);
        _settings = _settings with { Profiles = profiles };
    }

    private void UpdateGameProfileUi()
    {
        // The game selector is the only matching profile control. POE2 simply supports longer
        // wrapped modifiers, so users never need to remember a separate item-type mode.
    }

    private void SetProfileSelectorsEnabled(bool enabled)
    {
        GameProfileComboBox.IsEnabled = enabled;
        RuleModeComboBox.IsEnabled = enabled;
        EditStructuredRulesButton.IsEnabled = enabled;
        MonitoringPolicyComboBox.IsEnabled = enabled;
    }

    private static RuleEditorMode ParseRuleEditorMode(string? value) =>
        Enum.TryParse<RuleEditorMode>(value, ignoreCase: true, out var mode) &&
        Enum.IsDefined(mode)
            ? mode
            : RuleEditorMode.Quick;

    private static MonitoringPolicyId ParseMonitoringPolicy(string? value) =>
        Enum.TryParse<MonitoringPolicyId>(value, ignoreCase: true, out var policy) &&
        Enum.IsDefined(policy)
            ? policy
            : MonitoringPolicyId.Fast;

    private static GameProfile ParseGameProfile(string? value) =>
        Enum.TryParse<GameProfile>(value, ignoreCase: true, out var profile) &&
        Enum.IsDefined(profile)
            ? profile
            : GameProfile.Poe1;

    private void ShowValidationMessage(string message)
    {
        SetStatus(message, UiStatusKind.Warning);
        SystemSounds.Exclamation.Play();
    }

    private async Task AnalyzeScreenshotAsync(string path)
    {
        if (_ruleEditorMode == RuleEditorMode.Structured)
        {
            await AnalyzeStructuredScreenshotAsync(path);
            return;
        }

        var template = TargetAffixTextBox.Text.Trim();
        if (string.IsNullOrWhiteSpace(template))
        {
            throw new InvalidOperationException(UiText.Current.NeedTarget);
        }

        using var recognizer = CreateOcrRecognizer();
        var analyzer = new ScreenshotAffixAnalyzer(recognizer);
        var maximumLineSpan = SelectedMaximumLineSpan;
        ScreenshotAffixAnalysis analysis;
        try
        {
            analysis = await analyzer.AnalyzeAsync(
                path,
                template,
                _captureRegion,
                maximumLineSpan);
        }
        catch (ArgumentOutOfRangeException) when (_captureRegion is not null)
        {
            // A crop selected on a different monitor layout may not fit an archived image.
            // Falling back keeps screenshot replay useful; the OCR output makes misses visible.
            analysis = await analyzer.AnalyzeAsync(
                path,
                template,
                cropRegion: null,
                maximumLineSpan);
        }

        OcrOutputTextBox.Text = analysis.Lines.Count == 0
            ? UiText.Current.ScreenshotNoBlueText
            : string.Join(Environment.NewLine, analysis.Lines);
        LatencyText.Text = UiText.Current.ScreenshotMetrics(
            analysis.ImageLoadElapsed.TotalMilliseconds,
            analysis.PreprocessingElapsed.TotalMilliseconds,
            analysis.RecognitionElapsed.TotalMilliseconds);

        if (analysis.Match is { } match)
        {
            _monitoringHud?.HideHud();
            _alertService.Trigger(match.OriginalText, _captureRegion);
            SetProfileSelectorsEnabled(false);
            AcknowledgeButton.IsEnabled = true;
            SetStatus(UiText.Current.ScreenshotMatched, UiStatusKind.Alert);
        }
        else
        {
            SetStatus(UiText.Current.ScreenshotNotMatched, UiStatusKind.Neutral);
            ShowIdleHudIfEnabled();
        }
    }

    private async Task AnalyzeStructuredScreenshotAsync(string path)
    {
        if (_structuredRuleSet is null)
        {
            throw new InvalidOperationException(UiText.Current.NeedStructuredRules);
        }

        var rules = RuleCompiler.Compile(_structuredRuleSet, SelectedMaximumLineSpan);
        using var recognizer = CreateOcrRecognizer();
        if (recognizer.StructuredRuleSupport == StructuredRuleOcrSupport.Unsupported)
        {
            throw new NotSupportedException(UiText.Current.StructuredOcrUnsupported);
        }

        var analyzer = new ScreenshotAffixAnalyzer(recognizer);
        ScreenshotRuleAnalysis analysis;
        try
        {
            analysis = await analyzer.AnalyzeRulesAsync(path, rules, _captureRegion);
        }
        catch (ArgumentOutOfRangeException) when (_captureRegion is not null)
        {
            analysis = await analyzer.AnalyzeRulesAsync(path, rules, cropRegion: null);
        }

        OcrOutputTextBox.Text = analysis.Lines.Count == 0
            ? UiText.Current.ScreenshotNoBlueText
            : string.Join(Environment.NewLine, analysis.Lines);
        LatencyText.Text = UiText.Current.ScreenshotMetrics(
            analysis.ImageLoadElapsed.TotalMilliseconds,
            analysis.PreprocessingElapsed.TotalMilliseconds,
            analysis.RecognitionElapsed.TotalMilliseconds);

        if (analysis.IsMatch)
        {
            var detail = CreateStructuredMatchDetail(analysis.Evaluation);
            _monitoringHud?.HideHud();
            _alertService.Trigger(detail, _captureRegion);
            SetProfileSelectorsEnabled(false);
            AcknowledgeButton.IsEnabled = true;
            SetStatus(UiText.Current.ScreenshotStructuredMatched, UiStatusKind.Alert);
        }
        else
        {
            SetStatus(UiText.Current.ScreenshotStructuredNotMatched, UiStatusKind.Neutral);
            ShowIdleHudIfEnabled();
        }
    }

    private static string CreateStructuredMatchDetail(RuleEvaluationResult evaluation)
    {
        var group = evaluation.MatchedGroup;
        if (group is null)
        {
            return UiText.Current.DefaultDetectedText;
        }

        var observations = group.Conditions
            .Where(static condition => condition.IsMatched && condition.Observation is not null)
            .Select(static condition => condition.Observation!.OriginalText)
            .Distinct(StringComparer.Ordinal)
            .ToArray();
        return observations.Length == 0
            ? group.Name
            : $"{group.Name}: {string.Join(" + ", observations)}";
    }

    private async Task SaveSettingsAsync(long? expectedMonitoringSession = null)
    {
        if (!_isLoaded)
        {
            return;
        }

        try
        {
            CaptureCurrentProfileSettings();
            _settings = _settings with
            {
                SelectedGameProfile = _selectedGameProfile,
                UiLanguage = _uiLanguage,
                KeepHudVisible = _keepHudVisible,
                HudPlacement = _hudPlacement.Sanitize(),
                AllowOverlayCapture = _allowOverlayCapture,
                CustomAlertSoundPath = _customAlertSoundPath,
                StartMonitoringHotKey = _startMonitoringHotKey.SettingValue,
            };
            await _settingsStore.SaveAsync(_settings);
        }
        catch (SettingsSchemaTooNewException exception)
        {
            SetStatus(
                UiText.Current.FutureSettingsReadOnly(exception.DetectedSchemaVersion),
                UiStatusKind.Warning);
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
        {
            if (expectedMonitoringSession is { } expectedSession &&
                (Interlocked.Read(ref _activeMonitoringSession) != expectedSession ||
                 _monitor?.State != MonitorState.Running))
            {
                return;
            }

            SetStatus(UiText.Current.SettingsSaveFailed(exception.Message), UiStatusKind.Warning);
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
        _alertService.GuardedContinueRequested -= OnGuardedContinueRequested;
        _alertService.GuardedStopRequested -= OnGuardedStopRequested;
        _alertService.GuardedFailOpen -= OnGuardedFailOpen;
        _alertService.Dispose();
        if (_monitoringHud is not null)
        {
            _monitoringHud.PlacementChanged -= OnHudPlacementChanged;
            _monitoringHud.CloseFromOwner();
        }
        _monitoringHud = null;
        _acknowledgementHotKey?.Dispose();
        _acknowledgementHotKey = null;
        _selectionHotKey?.Dispose();
        _selectionHotKey = null;
        _startMonitoringHotKeyRegistration?.Dispose();
        _startMonitoringHotKeyRegistration = null;
        try
        {
            await SaveSettingsAsync();
            await ReleaseMonitorAsync();
        }
        catch (Exception exception)
        {
            Debug.WriteLine($"POE Alarm shutdown cleanup failed: {exception}");
        }
        finally
        {
            _closeReady = true;
            // Always leave the current WPF Closing call stack before issuing the one real close.
            // File/monitor cleanup can occasionally complete synchronously; calling Close again
            // in that case is a forbidden re-entrant close and can surface as CLR 0xe0434352.
            _ = Dispatcher.BeginInvoke(Close, DispatcherPriority.Normal);
        }
    }
}
