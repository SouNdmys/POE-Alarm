using System.Globalization;

namespace PoeAlarm.App.Localization;

/// <summary>
/// Provides UI-only text catalogs. This language is deliberately independent from the OCR
/// language selected for Path of Exile: changing it must never recreate a recognizer, alter a
/// target affix, or change matching behavior.
/// </summary>
internal static class UiText
{
    public const string DefaultLanguageCode = "zh-CN";

    private static readonly UiStrings SimplifiedChinese = CreateSimplifiedChinese();
    private static readonly UiStrings English = CreateEnglish();
    private static UiStrings current = SimplifiedChinese;

    public static IReadOnlyList<string> SupportedLanguageCodes { get; } =
        Array.AsReadOnly([DefaultLanguageCode, "en"]);

    public static UiStrings Current => Volatile.Read(ref current);

    /// <summary>Returns an immutable catalog without changing the active UI language.</summary>
    public static UiStrings For(string? languageCode) =>
        IsEnglish(languageCode) ? English : SimplifiedChinese;

    /// <summary>
    /// Changes the active UI catalog and returns it. Callers remain responsible for refreshing
    /// existing controls; OCR and matching services are intentionally not notified.
    /// </summary>
    public static UiStrings Use(string? languageCode)
    {
        var selected = For(languageCode);
        Interlocked.Exchange(ref current, selected);
        return selected;
    }

    public static string NormalizeLanguageCode(string? languageCode) =>
        For(languageCode).LanguageCode;

    private static bool IsEnglish(string? languageCode) =>
        !string.IsNullOrWhiteSpace(languageCode) &&
        languageCode.Trim().StartsWith("en", StringComparison.OrdinalIgnoreCase);

    private static UiStrings CreateSimplifiedChinese() => new()
    {
        LanguageCode = "zh-CN",
        Culture = CultureInfo.GetCultureInfo("zh-CN"),
        LanguageDisplayName = "简体中文",
        WindowTitle = "POE Alarm",
        AppTitle = "POE Alarm",
        AppSubtitle = "整句词缀识别 · 命中提醒 · 防止点过头",
        HelpButton = "使用说明",
        UiLanguageLabel = "界面语言",
        UiLanguageChinese = "简体中文",
        UiLanguageEnglish = "English",
        GameProfileLabel = "游戏版本",
        GameProfileToolTip = "POE1 与 POE2 会分别保存目标词缀、框选区域和识别语言。",
        Poe1GameProfile = "Path of Exile 1",
        Poe2GameProfile = "Path of Exile 2",
        TargetSectionTitle = "1  目标整句",
        PasteFromClipboard = "从剪贴板粘贴",
        TargetTextToolTip = "粘贴 PoEDB 中的完整词缀；数值大小会自动忽略。",
        CanonicalEmpty = "等待输入完整词缀；文字必须完整一致，数值大小会自动忽略。",
        RuleModeLabel = "目标模式",
        RuleModeToolTip = "快速模式保留原有整句匹配；多词缀模式支持多个可接受结果与数值约束。",
        QuickRuleMode = "快速模式（单条整句）",
        StructuredRuleMode = "多词缀命中模式",
        EditStructuredRules = "编辑规则",
        NoStructuredRules = "尚未配置多词缀规则",
        StructuredRuleHint = "命中任意一种可接受结果即报警；同一条实际词缀在结果内最多计数一次。",
        StructuredRuleSummaryTemplate = "{0} · {1} 个可接受结果 / {2} 条条件（开发预览）",
        NeedStructuredRules = "请先编辑并保存至少一种可接受结果。",
        StructuredOcrUnsupported = "当前游戏语言的批量目标 OCR 尚未通过发布门禁。已保留规则，但不会降级到缺少局部复核的识别路径。请先切换回快速模式。",
        StructuredTargetMatched = "已命中可接受结果；监控已锁停，先检查装备再确认。",
        ScreenshotStructuredMatched = "截图中命中可接受结果；已触发与实战相同的锁定告警。",
        ScreenshotStructuredNotMatched = "截图分析完成：未命中任何可接受结果。可在下方检查 OCR 物理行。",
        OcrLanguageLabel = "游戏语言",
        OcrLanguageToolTip = "请选择游戏内词缀使用的语言。这与程序界面语言相互独立。",
        OcrEnglishOption = "English（稳定）",
        OcrTraditionalChineseOption = "繁體中文（Windows 快速识别）",
        RegionSectionTitle = "2  监控区域",
        SelectRegion = "框选装备提示区域",
        RegionEmpty = "尚未选择。游戏内悬停装备后按 Ctrl + Shift + F11 最方便。",
        RunSectionTitle = "3  运行",
        StartMonitoring = "开始监控",
        StopMonitoring = "停止",
        AnalyzeScreenshot = "用截图测试",
        TestAlert = "测试阻挡告警",
        AcknowledgeAlert = "确认并关闭告警",
        DiagnosticTools = "诊断工具",
        StatusSectionTitle = "状态",
        OcrDetails = "识别详情",
        IdleStatus = "待机",
        NoScanData = "尚无扫描数据",
        OcrOutputPlaceholder = "OCR 识别到的物理行会显示在这里。",
        FooterHint = "Ctrl + Shift + F10 启动监控  ·  F11 框选区域  ·  F12 确认告警（均需 Ctrl + Shift）",
        ExperienceSettings = "体验设置",
        StartHotKeyLabel = "启动监控热键",
        HotkeyAlreadyInUse = "快捷键已被其他程序占用",
        KeepHudVisible = "常驻显示状态浮窗",
        HudPreferenceDescription = "绿色表示正在监控；冷灰色表示下一轮尚未开始。正常状态始终鼠标穿透。",
        PositionHud = "调整位置",
        FinishHudPosition = "完成定位",
        AlertSoundLabel = "命中提示音",
        BuiltInAlertSound = "内置原创稀有掉落提示音",
        ChooseWave = "选择 WAV",
        ResetSound = "恢复默认",
        ScreenRecordingLabel = "录屏兼容",
        AllowScreenRecording = "允许录屏捕获程序浮层（推荐）",
        ScreenRecordingDescription = "开启后，教程录制会包含状态浮窗和命中红窗；不会改变鼠标拦截。",
        HelpTitle = "使用说明与关于",
        HelpIntroduction = "三步开始监控：",
        HelpStep1Title = "粘贴完整词缀",
        HelpStep1Body = "先选择 POE1 或 POE2，再从 PoEDB 复制并粘贴整条词缀。不要只填关键词；数值大小会自动忽略。",
        HelpStep2Title = "框选可能出现词缀的区域",
        HelpStep2Body = "在游戏中显示装备提示框，再点击“框选装备提示区域”。框选范围只需覆盖词缀可能出现的位置。",
        HelpStep3Title = "开始并在命中后重新启动",
        HelpStep3Body = "点击“开始监控”或按 Ctrl + Shift + F10（可在体验设置中修改）。命中后检查装备并确认；每次成功拦截后都要重新启动监控。",
        HelpTip = "每次命中后自动停止监控，是防止连续误点的安全设计。继续制作前请再次点击“开始监控”。",
        HelpOcrTitle = "繁中用户建议安装 Windows OCR",
        HelpOcrBody = "安装 zh-TW OCR 通常能提高繁中识别的直接命中率和稳定性。请以管理员身份打开 PowerShell，依次运行下面两条命令；看到 State : Installed 后重启 POE Alarm。英文识别无需安装。",
        HelpOcrInstallCommand = "Add-WindowsCapability -Online -Name \"Language.OCR~~~zh-TW~0.0.1.0\"",
        HelpOcrVerifyCommand = "Get-WindowsCapability -Online -Name \"Language.OCR~~~zh-TW~0.0.1.0\"",
        HelpOcrSuccess = "验证结果出现 State : Installed 即安装成功。",
        HelpAuthorLabel = "作者",
        HelpContactLabel = "联系",
        HelpSupportTitle = "支持作者",
        HelpSupportBody = "暂未放置收款链接；如果它帮你少点过一次好词缀，反馈和分享就是很大的支持。",
        Close = "关闭",
        SwitchedChineseOcr = "已切换到繁體中文快速识别；优先使用 Windows 中文 OCR，缺少语言能力时使用内置兼容引擎。",
        SwitchedEnglishOcr = "已切换到 English 稳定识别。",
        SwitchedGameProfileTemplate = "已切换到 {0}；目标、框选区域和游戏语言均已载入该版本的独立配置。",
        RegionUpdated = "监控区域已更新。",
        RegionSelectionCancelled = "已取消框选；监控保持停止。",
        MonitoringStarted = "正在监控；切回游戏后保持装备提示框位于所选区域。",
        ScreenshotCancelled = "已取消截图分析；监控保持停止。",
        ScreenshotAnalyzing = "正在分析截图……",
        TestAlertDetectedText = "告警自检：这是红色命中提示",
        TestAlertTriggered = "告警自检已触发；Ctrl + Shift + F12 可关闭。",
        EmergencyGuardReleased = "已紧急解除识别保护并停止监控。",
        AlertAcknowledged = "告警已确认，监控保持停止；检查装备后可重新开始。",
        MonitoringNoBlueText = "正在监控；当前没有蓝色词缀，淡绿色浮窗会保持显示直到你手动停止。",
        MonitoringWithOutput = "正在监控；淡绿色浮窗显示当前目标和运行时间。",
        TargetMatched = "目标整句已命中；监控已锁停，先检查装备再确认。",
        MonitoringStopped = "监控已停止。",
        HudPlacementStarted = "浮窗定位已开启：拖动状态条到顺眼的位置，再点击“完成定位”。",
        HudPlacementFinished = "浮窗位置已保存；正常状态已恢复鼠标穿透。",
        SoundReset = "已恢复内置原创提示音。可在“诊断工具”中测试告警。",
        SoundSelected = "已选择自定义 WAV。可在“诊断工具”中测试告警。",
        InvalidWave = "这个 WAV 无法使用；请选择标准 PCM WAV 文件。",
        ScreenshotNoBlueText = "没有识别到蓝色词缀文字。",
        ScreenshotMatched = "截图中命中目标整句；已触发与实战相同的锁定告警。",
        ScreenshotNotMatched = "截图分析完成：未命中目标整句。可在下方检查 OCR 物理行。",
        NeedTarget = "请先粘贴 PoEDB 的完整目标词缀。",
        NeedRegion = "请先框选装备提示框所在区域。",
        ScreenshotDialogTitle = "选择 POE 截图",
        ScreenshotDialogFilter = "图片 (*.png;*.jpg;*.jpeg)|*.png;*.jpg;*.jpeg|所有文件 (*.*)|*.*",
        HudTitle = "POE Alarm · 监控中",
        HudIdleTitle = "POE Alarm · 未监控",
        HudIdleHint = "下一轮请点击“开始监控”",
        HudPlacementTitle = "拖动浮窗调整位置",
        HudPlacementHint = "放好后回到主程序点击“完成定位”",
        HudEmptyTarget = "尚未设置目标词缀",
        HudTargetLabel = "当前目标",
        AlertTitle = "目标词缀命中",
        AlertBlockingMessage = "后续鼠标点击已被此窗口阻挡，请先检查装备",
        AlertShortcut = "也可按 Ctrl + Shift + F12 解除锁定",
        AlertAcknowledge = "我已检查，解除鼠标锁定",
        AlertReleasing = "正在解除锁定……",
        DefaultDetectedText = "目标词缀已命中",
        SelectionInstruction = "拖出只包含装备提示框的区域 · Esc 取消",
        PendingGuardInstallFailed = "无法安装高速点击保护；为避免无保护洗装，监控未启动。",
        PendingGuardArmFailed = "高速点击保护未能武装；为避免跳过目标词缀，监控已停止。",
        WindowsEnglishOcrUnavailable = "Windows English OCR 不可用。请在 Windows 设置中安装 English OCR 语言功能。",
        WindowsChineseOcrUnavailable = "Windows 中文 OCR 不可用。请在 Windows 语言设置中安装繁體中文（台湾）或简体中文 OCR 语言功能。",
        RegionSelectionFailedTemplate = "无法框选监控区域：{0}",
        CannotStartMonitoringTemplate = "无法开始监控：{0}",
        ScreenshotFailedTemplate = "截图分析失败：{0}",
        SettingsSaveFailedTemplate = "设置无法保存，但本次仍可使用：{0}",
        GlobalAcknowledgeHotkeyUnavailableTemplate = "全局确认快捷键不可用：{0}",
        GlobalSelectionHotkeyUnavailableTemplate = "全局框选快捷键不可用：{0}",
        GlobalStartHotkeyUnavailableTemplate = "全局启动快捷键 {0} 不可用：{1}",
        StartHotkeyUpdatedTemplate = "启动热键已改为 {0}。",
        MonitorStoppedWithDetailTemplate = "监控已停止：{0}",
        CanonicalMatchTemplate = "实际匹配：{0}",
        SelectedRegionTemplate = "已选择：{0}",
        ScanMetricsTemplate = "扫描 #{0}   截屏 {1:F1} ms   OCR {2:F1} ms{3}",
        CachedFrameSuffix = "（画面未变）",
        ScreenshotMetricsTemplate = "读取 {0:F1} ms   预处理 {1:F1} ms   OCR {2:F1} ms",
        WindowsChineseInitializationFailedTemplate = "Windows 无法初始化中文 OCR（{0}）。",
        WindowsOcrImageTooWideTemplate = "识别区域超过 Windows OCR 的 {0}px 限制；请框选更小的装备提示区域。",
    };

    private static UiStrings CreateEnglish() => new()
    {
        LanguageCode = "en",
        Culture = CultureInfo.GetCultureInfo("en-US"),
        LanguageDisplayName = "English",
        WindowTitle = "POE Alarm",
        AppTitle = "POE Alarm",
        AppSubtitle = "Full-affix OCR · instant alerts · protection against over-clicking",
        HelpButton = "How to use",
        UiLanguageLabel = "Interface language",
        UiLanguageChinese = "简体中文",
        UiLanguageEnglish = "English",
        GameProfileLabel = "Game version",
        GameProfileToolTip = "POE1 and POE2 keep separate target affixes, regions, and OCR languages.",
        Poe1GameProfile = "Path of Exile 1",
        Poe2GameProfile = "Path of Exile 2",
        TargetSectionTitle = "1  Target affix",
        PasteFromClipboard = "Paste from clipboard",
        TargetTextToolTip = "Paste the complete affix from PoEDB. Numeric magnitudes are ignored.",
        CanonicalEmpty = "Paste a complete affix. All wording must match; numeric magnitudes are ignored.",
        RuleModeLabel = "Target mode",
        RuleModeToolTip = "Quick mode retains the original complete-affix matcher. Multi-affix mode adds acceptable results and numeric constraints.",
        QuickRuleMode = "Quick (one complete affix)",
        StructuredRuleMode = "Multi-affix rules",
        EditStructuredRules = "Edit rules",
        NoStructuredRules = "No multi-affix rule configured",
        StructuredRuleHint = "Matching any acceptable result triggers the alarm. One observed modifier counts at most once within a result.",
        StructuredRuleSummaryTemplate = "{0} · {1} acceptable result(s) / {2} condition(s) (preview)",
        NeedStructuredRules = "Edit and save at least one acceptable result first.",
        StructuredOcrUnsupported = "Batch target OCR for the current game language has not passed the release gate. Your rules are preserved, but the app will not fall back to recognition without its verified recovery path. Use Quick mode for now.",
        StructuredTargetMatched = "An acceptable result matched. Monitoring is locked and stopped; inspect the item before acknowledging.",
        ScreenshotStructuredMatched = "The screenshot matches an acceptable result; the same blocking alert used in live monitoring was triggered.",
        ScreenshotStructuredNotMatched = "Screenshot analysis complete: no acceptable result matched. Review the OCR lines below.",
        OcrLanguageLabel = "Game language",
        OcrLanguageToolTip = "Select the language used for affixes in the game. This is independent of the app's interface language.",
        OcrEnglishOption = "English (stable)",
        OcrTraditionalChineseOption = "Traditional Chinese (fast Windows OCR)",
        RegionSectionTitle = "2  Monitoring region",
        SelectRegion = "Select tooltip region",
        RegionEmpty = "Not selected. Hover the item in-game, then press Ctrl + Shift + F11.",
        RunSectionTitle = "3  Run",
        StartMonitoring = "Start monitoring",
        StopMonitoring = "Stop",
        AnalyzeScreenshot = "Test a screenshot",
        TestAlert = "Test blocking alert",
        AcknowledgeAlert = "Acknowledge and close alert",
        DiagnosticTools = "Diagnostic tools",
        StatusSectionTitle = "Status",
        OcrDetails = "Recognition details",
        IdleStatus = "Idle",
        NoScanData = "No scan data yet",
        OcrOutputPlaceholder = "Physical lines detected by OCR will appear here.",
        FooterHint = "Ctrl + Shift + F10 starts monitoring  ·  F11 selects the region  ·  F12 acknowledges the alert",
        ExperienceSettings = "Experience settings",
        StartHotKeyLabel = "Start shortcut",
        HotkeyAlreadyInUse = "The shortcut is already in use",
        KeepHudVisible = "Keep the status HUD visible",
        HudPreferenceDescription = "Green means monitoring is active; cool gray means the next round has not started. The HUD is always click-through in normal use.",
        PositionHud = "Position HUD",
        FinishHudPosition = "Finish positioning",
        AlertSoundLabel = "Match alert sound",
        BuiltInAlertSound = "Built-in original rare-drop chime",
        ChooseWave = "Choose WAV",
        ResetSound = "Use default",
        ScreenRecordingLabel = "Screen recording",
        AllowScreenRecording = "Include app overlays in recordings (recommended)",
        ScreenRecordingDescription = "Tutorial recordings will include the status HUD and red match alert. Mouse blocking is unchanged.",
        HelpTitle = "POE Alarm — Help & About",
        HelpIntroduction = "Start monitoring in three steps:",
        HelpStep1Title = "Paste the complete affix",
        HelpStep1Body = "Select POE1 or POE2, then copy the entire affix from PoEDB. Do not enter keywords only; numeric magnitudes are ignored.",
        HelpStep2Title = "Select where the affix may appear",
        HelpStep2Body = "Show the item tooltip in-game, then choose “Select tooltip region.” The selected area only needs to cover every position where affixes may appear.",
        HelpStep3Title = "Start again after each match",
        HelpStep3Body = "Choose “Start monitoring” or press Ctrl + Shift + F10 (customizable under Experience settings). Inspect and acknowledge each hit, then start monitoring again for the next item.",
        HelpTip = "Monitoring stops after every match by design, preventing accidental extra clicks. Start monitoring again before continuing.",
        HelpOcrTitle = "Recommended Windows OCR component for Traditional Chinese",
        HelpOcrBody = "Installing zh-TW OCR usually improves direct-hit coverage and stability for Traditional Chinese. Open PowerShell as Administrator, run both commands below, then restart POE Alarm after State : Installed appears. English recognition does not require it.",
        HelpOcrInstallCommand = "Add-WindowsCapability -Online -Name \"Language.OCR~~~zh-TW~0.0.1.0\"",
        HelpOcrVerifyCommand = "Get-WindowsCapability -Online -Name \"Language.OCR~~~zh-TW~0.0.1.0\"",
        HelpOcrSuccess = "The installation is complete when the verification output shows State : Installed.",
        HelpAuthorLabel = "Author",
        HelpContactLabel = "Contact",
        HelpSupportTitle = "Support the author",
        HelpSupportBody = "No donation link is enabled yet. Feedback and sharing are already a great way to support continued development.",
        Close = "Close",
        SwitchedChineseOcr = "Switched to fast Traditional Chinese recognition. Windows Chinese OCR is preferred; the bundled compatibility engine is used when it is unavailable.",
        SwitchedEnglishOcr = "Switched to stable English recognition.",
        SwitchedGameProfileTemplate = "Switched to {0}. Its target, region, and game language were loaded independently.",
        RegionUpdated = "Monitoring region updated.",
        RegionSelectionCancelled = "Region selection cancelled; monitoring remains stopped.",
        MonitoringStarted = "Monitoring. Return to the game and keep the item tooltip inside the selected region.",
        ScreenshotCancelled = "Screenshot analysis cancelled; monitoring remains stopped.",
        ScreenshotAnalyzing = "Analyzing screenshot…",
        TestAlertDetectedText = "Alert test: this is the red match notification",
        TestAlertTriggered = "Test alert triggered. Press Ctrl + Shift + F12 to close it.",
        EmergencyGuardReleased = "Recognition protection was released and monitoring was stopped.",
        AlertAcknowledged = "Alert acknowledged. Monitoring remains stopped; inspect the item, then start again.",
        MonitoringNoBlueText = "Monitoring. No blue affix text is visible; the pale-green HUD remains until you stop manually.",
        MonitoringWithOutput = "Monitoring. The pale-green HUD shows the current target and elapsed time.",
        TargetMatched = "Target affix matched. Monitoring is locked and stopped; inspect the item before acknowledging.",
        MonitoringStopped = "Monitoring stopped.",
        HudPlacementStarted = "HUD positioning is active. Drag the status bar, then select “Finish positioning.”",
        HudPlacementFinished = "HUD position saved; normal click-through behavior was restored.",
        SoundReset = "Restored the built-in original alert. Test it under Diagnostic tools.",
        SoundSelected = "Custom WAV selected. Test it under Diagnostic tools.",
        InvalidWave = "That WAV cannot be used. Choose a standard PCM WAV file.",
        ScreenshotNoBlueText = "No blue affix text was detected.",
        ScreenshotMatched = "The screenshot contains the target affix; the same blocking alert used in live monitoring was triggered.",
        ScreenshotNotMatched = "Screenshot analysis complete: target affix not found. Review the OCR lines below.",
        NeedTarget = "Paste the complete target affix from PoEDB first.",
        NeedRegion = "Select the area containing the item tooltip first.",
        ScreenshotDialogTitle = "Select a POE screenshot",
        ScreenshotDialogFilter = "Images (*.png;*.jpg;*.jpeg)|*.png;*.jpg;*.jpeg|All files (*.*)|*.*",
        HudTitle = "POE Alarm · Monitoring",
        HudIdleTitle = "POE Alarm · Not monitoring",
        HudIdleHint = "Select Start monitoring for the next round",
        HudPlacementTitle = "Drag to position the HUD",
        HudPlacementHint = "Return to the app and select Finish positioning",
        HudEmptyTarget = "No target affix yet",
        HudTargetLabel = "Current target",
        AlertTitle = "Target affix found",
        AlertBlockingMessage = "Further mouse clicks are blocked by this window. Inspect the item first.",
        AlertShortcut = "You can also press Ctrl + Shift + F12 to release the lock",
        AlertAcknowledge = "I checked the item — release mouse lock",
        AlertReleasing = "Releasing lock…",
        DefaultDetectedText = "Target affix found",
        SelectionInstruction = "Drag around the item tooltip only · Esc to cancel",
        PendingGuardInstallFailed = "Could not install high-speed click protection. Monitoring was not started because crafting would be unprotected.",
        PendingGuardArmFailed = "Could not arm high-speed click protection. Monitoring stopped to avoid skipping the target affix.",
        WindowsEnglishOcrUnavailable = "Windows English OCR is unavailable. Install the English OCR language capability in Windows Settings.",
        WindowsChineseOcrUnavailable = "Windows Chinese OCR is unavailable. Install the Traditional Chinese (Taiwan) or Simplified Chinese OCR language capability in Windows Settings.",
        RegionSelectionFailedTemplate = "Could not select the monitoring region: {0}",
        CannotStartMonitoringTemplate = "Could not start monitoring: {0}",
        ScreenshotFailedTemplate = "Screenshot analysis failed: {0}",
        SettingsSaveFailedTemplate = "Settings could not be saved, but this session can continue: {0}",
        GlobalAcknowledgeHotkeyUnavailableTemplate = "The global acknowledge shortcut is unavailable: {0}",
        GlobalSelectionHotkeyUnavailableTemplate = "The global region-selection shortcut is unavailable: {0}",
        GlobalStartHotkeyUnavailableTemplate = "Global start shortcut {0} is unavailable: {1}",
        StartHotkeyUpdatedTemplate = "Start shortcut changed to {0}.",
        MonitorStoppedWithDetailTemplate = "Monitoring stopped: {0}",
        CanonicalMatchTemplate = "Actual match: {0}",
        SelectedRegionTemplate = "Selected: {0}",
        ScanMetricsTemplate = "Scan #{0}   Capture {1:F1} ms   OCR {2:F1} ms{3}",
        CachedFrameSuffix = " (unchanged frame)",
        ScreenshotMetricsTemplate = "Load {0:F1} ms   Preprocess {1:F1} ms   OCR {2:F1} ms",
        WindowsChineseInitializationFailedTemplate = "Windows could not initialize Chinese OCR ({0}).",
        WindowsOcrImageTooWideTemplate = "The recognition region exceeds Windows OCR's {0}px limit. Select a smaller item-tooltip region.",
    };
}

/// <summary>An immutable set of strings for one interface language.</summary>
internal sealed record UiStrings
{
    public required string LanguageCode { get; init; }
    public required CultureInfo Culture { get; init; }
    public required string LanguageDisplayName { get; init; }
    public required string WindowTitle { get; init; }
    public required string AppTitle { get; init; }
    public required string AppSubtitle { get; init; }
    public required string HelpButton { get; init; }
    public required string UiLanguageLabel { get; init; }
    public required string UiLanguageChinese { get; init; }
    public required string UiLanguageEnglish { get; init; }
    public required string GameProfileLabel { get; init; }
    public required string GameProfileToolTip { get; init; }
    public required string Poe1GameProfile { get; init; }
    public required string Poe2GameProfile { get; init; }
    public required string TargetSectionTitle { get; init; }
    public required string PasteFromClipboard { get; init; }
    public required string TargetTextToolTip { get; init; }
    public required string CanonicalEmpty { get; init; }
    public required string RuleModeLabel { get; init; }
    public required string RuleModeToolTip { get; init; }
    public required string QuickRuleMode { get; init; }
    public required string StructuredRuleMode { get; init; }
    public required string EditStructuredRules { get; init; }
    public required string NoStructuredRules { get; init; }
    public required string StructuredRuleHint { get; init; }
    public required string StructuredRuleSummaryTemplate { get; init; }
    public required string NeedStructuredRules { get; init; }
    public required string StructuredOcrUnsupported { get; init; }
    public required string StructuredTargetMatched { get; init; }
    public required string ScreenshotStructuredMatched { get; init; }
    public required string ScreenshotStructuredNotMatched { get; init; }
    public required string OcrLanguageLabel { get; init; }
    public required string OcrLanguageToolTip { get; init; }
    public required string OcrEnglishOption { get; init; }
    public required string OcrTraditionalChineseOption { get; init; }
    public required string RegionSectionTitle { get; init; }
    public required string SelectRegion { get; init; }
    public required string RegionEmpty { get; init; }
    public required string RunSectionTitle { get; init; }
    public required string StartMonitoring { get; init; }
    public required string StopMonitoring { get; init; }
    public required string AnalyzeScreenshot { get; init; }
    public required string TestAlert { get; init; }
    public required string AcknowledgeAlert { get; init; }
    public required string DiagnosticTools { get; init; }
    public required string StatusSectionTitle { get; init; }
    public required string OcrDetails { get; init; }
    public required string IdleStatus { get; init; }
    public required string NoScanData { get; init; }
    public required string OcrOutputPlaceholder { get; init; }
    public required string FooterHint { get; init; }
    public required string ExperienceSettings { get; init; }
    public required string StartHotKeyLabel { get; init; }
    public required string HotkeyAlreadyInUse { get; init; }
    public required string KeepHudVisible { get; init; }
    public required string HudPreferenceDescription { get; init; }
    public required string PositionHud { get; init; }
    public required string FinishHudPosition { get; init; }
    public required string AlertSoundLabel { get; init; }
    public required string BuiltInAlertSound { get; init; }
    public required string ChooseWave { get; init; }
    public required string ResetSound { get; init; }
    public required string ScreenRecordingLabel { get; init; }
    public required string AllowScreenRecording { get; init; }
    public required string ScreenRecordingDescription { get; init; }
    public required string HelpTitle { get; init; }
    public required string HelpIntroduction { get; init; }
    public required string HelpStep1Title { get; init; }
    public required string HelpStep1Body { get; init; }
    public required string HelpStep2Title { get; init; }
    public required string HelpStep2Body { get; init; }
    public required string HelpStep3Title { get; init; }
    public required string HelpStep3Body { get; init; }
    public required string HelpTip { get; init; }
    public required string HelpOcrTitle { get; init; }
    public required string HelpOcrBody { get; init; }
    public required string HelpOcrInstallCommand { get; init; }
    public required string HelpOcrVerifyCommand { get; init; }
    public required string HelpOcrSuccess { get; init; }
    public required string HelpAuthorLabel { get; init; }
    public required string HelpContactLabel { get; init; }
    public required string HelpSupportTitle { get; init; }
    public required string HelpSupportBody { get; init; }
    public required string Close { get; init; }
    public required string SwitchedChineseOcr { get; init; }
    public required string SwitchedEnglishOcr { get; init; }
    public required string SwitchedGameProfileTemplate { get; init; }
    public required string RegionUpdated { get; init; }
    public required string RegionSelectionCancelled { get; init; }
    public required string MonitoringStarted { get; init; }
    public required string ScreenshotCancelled { get; init; }
    public required string ScreenshotAnalyzing { get; init; }
    public required string TestAlertDetectedText { get; init; }
    public required string TestAlertTriggered { get; init; }
    public required string EmergencyGuardReleased { get; init; }
    public required string AlertAcknowledged { get; init; }
    public required string MonitoringNoBlueText { get; init; }
    public required string MonitoringWithOutput { get; init; }
    public required string TargetMatched { get; init; }
    public required string MonitoringStopped { get; init; }
    public required string HudPlacementStarted { get; init; }
    public required string HudPlacementFinished { get; init; }
    public required string SoundReset { get; init; }
    public required string SoundSelected { get; init; }
    public required string InvalidWave { get; init; }
    public required string ScreenshotNoBlueText { get; init; }
    public required string ScreenshotMatched { get; init; }
    public required string ScreenshotNotMatched { get; init; }
    public required string NeedTarget { get; init; }
    public required string NeedRegion { get; init; }
    public required string ScreenshotDialogTitle { get; init; }
    public required string ScreenshotDialogFilter { get; init; }
    public required string HudTitle { get; init; }
    public required string HudIdleTitle { get; init; }
    public required string HudIdleHint { get; init; }
    public required string HudPlacementTitle { get; init; }
    public required string HudPlacementHint { get; init; }
    public required string HudEmptyTarget { get; init; }
    public required string HudTargetLabel { get; init; }
    public required string AlertTitle { get; init; }
    public required string AlertBlockingMessage { get; init; }
    public required string AlertShortcut { get; init; }
    public required string AlertAcknowledge { get; init; }
    public required string AlertReleasing { get; init; }
    public required string DefaultDetectedText { get; init; }
    public required string SelectionInstruction { get; init; }
    public required string PendingGuardInstallFailed { get; init; }
    public required string PendingGuardArmFailed { get; init; }
    public required string WindowsEnglishOcrUnavailable { get; init; }
    public required string WindowsChineseOcrUnavailable { get; init; }
    public required string RegionSelectionFailedTemplate { get; init; }
    public required string CannotStartMonitoringTemplate { get; init; }
    public required string ScreenshotFailedTemplate { get; init; }
    public required string SettingsSaveFailedTemplate { get; init; }
    public required string GlobalAcknowledgeHotkeyUnavailableTemplate { get; init; }
    public required string GlobalSelectionHotkeyUnavailableTemplate { get; init; }
    public required string GlobalStartHotkeyUnavailableTemplate { get; init; }
    public required string StartHotkeyUpdatedTemplate { get; init; }
    public required string MonitorStoppedWithDetailTemplate { get; init; }
    public required string CanonicalMatchTemplate { get; init; }
    public required string SelectedRegionTemplate { get; init; }
    public required string ScanMetricsTemplate { get; init; }
    public required string CachedFrameSuffix { get; init; }
    public required string ScreenshotMetricsTemplate { get; init; }
    public required string WindowsChineseInitializationFailedTemplate { get; init; }
    public required string WindowsOcrImageTooWideTemplate { get; init; }

    public string GlobalAcknowledgeHotkeyUnavailable(string detail) =>
        Format(GlobalAcknowledgeHotkeyUnavailableTemplate, detail);

    public string GlobalSelectionHotkeyUnavailable(string detail) =>
        Format(GlobalSelectionHotkeyUnavailableTemplate, detail);

    public string GlobalStartHotkeyUnavailable(string key, string detail) =>
        Format(GlobalStartHotkeyUnavailableTemplate, key, detail);

    public string StartHotkeyUpdated(string key) =>
        Format(StartHotkeyUpdatedTemplate, key);

    public string RegionSelectionFailed(string detail) =>
        Format(RegionSelectionFailedTemplate, detail);

    public string CannotStartMonitoring(string detail) =>
        Format(CannotStartMonitoringTemplate, detail);

    public string ScreenshotFailed(string detail) =>
        Format(ScreenshotFailedTemplate, detail);

    public string SettingsSaveFailed(string detail) =>
        Format(SettingsSaveFailedTemplate, detail);

    public string SwitchedGameProfile(string profileName) =>
        Format(SwitchedGameProfileTemplate, profileName);

    public string MonitorStoppedWithDetail(string detail) =>
        Format(MonitorStoppedWithDetailTemplate, detail);

    public string CanonicalMatch(string canonicalText) =>
        Format(CanonicalMatchTemplate, canonicalText);

    public string StructuredRuleSummary(string name, int groupCount, int conditionCount) =>
        Format(StructuredRuleSummaryTemplate, name, groupCount, conditionCount);

    public string SelectedRegion(string region) =>
        Format(SelectedRegionTemplate, region);

    public string ScanMetrics(long scanCount, double captureMilliseconds, double ocrMilliseconds,
        bool wasCached) =>
        Format(
            ScanMetricsTemplate,
            scanCount,
            captureMilliseconds,
            ocrMilliseconds,
            wasCached ? CachedFrameSuffix : string.Empty);

    public string ScreenshotMetrics(double loadMilliseconds, double preprocessingMilliseconds,
        double ocrMilliseconds) =>
        Format(
            ScreenshotMetricsTemplate,
            loadMilliseconds,
            preprocessingMilliseconds,
            ocrMilliseconds);

    public string WindowsChineseInitializationFailed(string languageTag) =>
        Format(WindowsChineseInitializationFailedTemplate, languageTag);

    public string WindowsOcrImageTooWide(int maximumPixels) =>
        Format(WindowsOcrImageTooWideTemplate, maximumPixels);

    private string Format(string format, params object?[] arguments) =>
        string.Format(Culture, format, arguments);
}
