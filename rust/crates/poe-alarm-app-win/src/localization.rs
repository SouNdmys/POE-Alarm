use crate::{
    EditorError, Game, MonitorStatus, NumericConstraintMode, OcrLanguage, ResultGroupMode,
    RuleMode, UiLanguage, UiNotice,
};

/// Every user-facing sentence lives here so Chinese and English never leak into each other.
#[derive(Clone, Copy, Debug)]
pub struct TextCatalog {
    language: UiLanguage,
    pub window_title: &'static str,
    pub heading: &'static str,
    pub subtitle: &'static str,
    pub game_label: &'static str,
    pub language_label: &'static str,
    pub rule_mode_label: &'static str,
    pub save_button: &'static str,
    pub game_poe1: &'static str,
    pub game_poe2: &'static str,
    pub language_zh: &'static str,
    pub language_en: &'static str,
    pub mode_quick: &'static str,
    pub mode_multiple: &'static str,
    pub quick_help: &'static str,
    pub quick_template_label: &'static str,
    pub ocr_language_label: &'static str,
    pub ocr_en: &'static str,
    pub ocr_zh: &'static str,
    pub capture_region_label: &'static str,
    pub region_x: &'static str,
    pub region_y: &'static str,
    pub region_width: &'static str,
    pub region_height: &'static str,
    pub select_region_button: &'static str,
    pub structured_single_help: &'static str,
    pub structured_help: &'static str,
    pub results_label: &'static str,
    pub add_result: &'static str,
    pub delete_result: &'static str,
    pub result_name: &'static str,
    pub group_rule: &'static str,
    pub group_any: &'static str,
    pub group_all: &'static str,
    pub group_at_least: &'static str,
    pub required_count: &'static str,
    pub conditions_label: &'static str,
    pub add_condition: &'static str,
    pub delete_condition: &'static str,
    pub condition_name: &'static str,
    pub condition_template: &'static str,
    pub numeric_rules_label: &'static str,
    pub numeric_comparison: &'static str,
    pub numeric_value_help: &'static str,
    pub add_numeric_rule: &'static str,
    pub delete_numeric_rule: &'static str,
    pub numeric_ignore: &'static str,
    pub numeric_range: &'static str,
    pub numeric_at_least: &'static str,
    pub numeric_at_most: &'static str,
    pub numeric_exact: &'static str,
    pub first_value: &'static str,
    pub second_value: &'static str,
    pub general_settings_label: &'static str,
    pub keep_hud_visible: &'static str,
    pub allow_overlay_capture: &'static str,
    pub hud_monitor: &'static str,
    pub hud_x: &'static str,
    pub hud_y: &'static str,
    pub hud_position_help: &'static str,
    pub place_hud_button: &'static str,
    pub save_hud_position_button: &'static str,
    pub alert_sound: &'static str,
    pub browse_sound_button: &'static str,
    pub default_sound_button: &'static str,
    pub hotkey: &'static str,
    pub status_label: &'static str,
    pub screenshot_results_label: &'static str,
    pub start_button: &'static str,
    pub stop_button: &'static str,
    pub screenshot_button: &'static str,
}

pub trait UiText {
    fn text(self) -> &'static TextCatalog;
}

impl UiText for UiLanguage {
    fn text(self) -> &'static TextCatalog {
        match self {
            UiLanguage::SimplifiedChinese => &ZH,
            UiLanguage::English => &EN,
        }
    }
}

impl TextCatalog {
    pub fn game(self, game: Game) -> &'static str {
        match game {
            Game::Poe1 => self.game_poe1,
            Game::Poe2 => self.game_poe2,
        }
    }

    pub fn rule_mode(self, mode: RuleMode) -> &'static str {
        match mode {
            RuleMode::Quick => self.mode_quick,
            RuleMode::MultipleAffixes => self.mode_multiple,
        }
    }

    pub fn ocr_language(self, language: OcrLanguage) -> &'static str {
        match language {
            OcrLanguage::English => self.ocr_en,
            OcrLanguage::TraditionalChinese => self.ocr_zh,
        }
    }

    pub fn group_mode(self, mode: ResultGroupMode) -> &'static str {
        match mode {
            ResultGroupMode::Any => self.group_any,
            ResultGroupMode::All => self.group_all,
            ResultGroupMode::AtLeast => self.group_at_least,
        }
    }

    pub fn numeric_mode(self, mode: NumericConstraintMode) -> &'static str {
        match mode {
            NumericConstraintMode::Ignore => self.numeric_ignore,
            NumericConstraintMode::RangeInclusive => self.numeric_range,
            NumericConstraintMode::AtLeast => self.numeric_at_least,
            NumericConstraintMode::AtMost => self.numeric_at_most,
            NumericConstraintMode::Exactly => self.numeric_exact,
        }
    }

    pub fn result_item(self, index: usize, name: &str) -> String {
        let name = name.trim();
        if name.is_empty() {
            match self.language {
                UiLanguage::SimplifiedChinese => format!("命中方案 {}", index + 1),
                UiLanguage::English => format!("Match option {}", index + 1),
            }
        } else {
            format!("{} — {name}", index + 1)
        }
    }

    pub fn condition_item(self, index: usize, name: &str) -> String {
        let name = name.trim();
        if name.is_empty() {
            match self.language {
                UiLanguage::SimplifiedChinese => format!("词缀 {}", index + 1),
                UiLanguage::English => format!("Affix {}", index + 1),
            }
        } else {
            format!("{} — {name}", index + 1)
        }
    }

    pub fn numeric_item(self, index: usize) -> String {
        match self.language {
            UiLanguage::SimplifiedChinese => format!("数值 {}", index + 1),
            UiLanguage::English => format!("Value {}", index + 1),
        }
    }

    pub fn status(self, status: MonitorStatus, notice: &UiNotice) -> String {
        match notice {
            UiNotice::Validation(error) => return self.editor_error(error),
            UiNotice::Saved => {
                return match self.language {
                    UiLanguage::SimplifiedChinese => "设置已保存".to_owned(),
                    UiLanguage::English => "Settings saved".to_owned(),
                };
            }
            UiNotice::SaveFailed => {
                return match self.language {
                    UiLanguage::SimplifiedChinese => "保存失败，原设置文件没有被覆盖".to_owned(),
                    UiLanguage::English => {
                        "Save failed. The existing settings file was not replaced".to_owned()
                    }
                };
            }
            UiNotice::FutureSchema {
                detected,
                supported,
            } => {
                return match self.language {
                    UiLanguage::SimplifiedChinese => format!(
                        "设置文件版本为 {detected}，本程序只支持到 {supported}。文件会保持不变，不能修改或保存"
                    ),
                    UiLanguage::English => format!(
                        "Settings version {detected} is newer than supported version {supported}. The file is unchanged, and editing and saving are disabled"
                    ),
                };
            }
            UiNotice::BackgroundFailed => {
                return match self.language {
                    UiLanguage::SimplifiedChinese => "操作失败，请重试".to_owned(),
                    UiLanguage::English => "The action failed. Please try again".to_owned(),
                };
            }
            UiNotice::None => {}
        }

        match (self.language, status) {
            (UiLanguage::SimplifiedChinese, MonitorStatus::Ready) => "可以开始".to_owned(),
            (UiLanguage::SimplifiedChinese, MonitorStatus::Starting) => "正在启动监控…".to_owned(),
            (UiLanguage::SimplifiedChinese, MonitorStatus::Monitoring) => "正在监控词缀".to_owned(),
            (UiLanguage::SimplifiedChinese, MonitorStatus::MatchFound) => {
                "已命中目标，请确认提醒".to_owned()
            }
            (UiLanguage::SimplifiedChinese, MonitorStatus::Stopping) => "正在停止监控…".to_owned(),
            (UiLanguage::SimplifiedChinese, MonitorStatus::TestingScreenshot) => {
                "正在识别截图…".to_owned()
            }
            (UiLanguage::SimplifiedChinese, MonitorStatus::ScreenshotComplete) => {
                "截图识别完成".to_owned()
            }
            (UiLanguage::SimplifiedChinese, MonitorStatus::SavingSettings) => {
                "正在保存设置…".to_owned()
            }
            (UiLanguage::SimplifiedChinese, MonitorStatus::SettingsSaved) => {
                "设置已保存".to_owned()
            }
            (UiLanguage::SimplifiedChinese, MonitorStatus::ValidationError) => {
                "请检查填写内容".to_owned()
            }
            (UiLanguage::SimplifiedChinese, MonitorStatus::ReadOnly) => {
                "设置只能查看，不能保存".to_owned()
            }
            (UiLanguage::SimplifiedChinese, MonitorStatus::Error) => "操作失败，请重试".to_owned(),
            (UiLanguage::English, MonitorStatus::Ready) => "Ready".to_owned(),
            (UiLanguage::English, MonitorStatus::Starting) => "Starting monitoring…".to_owned(),
            (UiLanguage::English, MonitorStatus::Monitoring) => "Monitoring affixes".to_owned(),
            (UiLanguage::English, MonitorStatus::MatchFound) => {
                "Target found. Acknowledge the alert".to_owned()
            }
            (UiLanguage::English, MonitorStatus::Stopping) => "Stopping monitoring…".to_owned(),
            (UiLanguage::English, MonitorStatus::TestingScreenshot) => {
                "Analyzing screenshot…".to_owned()
            }
            (UiLanguage::English, MonitorStatus::ScreenshotComplete) => {
                "Screenshot analysis complete".to_owned()
            }
            (UiLanguage::English, MonitorStatus::SavingSettings) => "Saving settings…".to_owned(),
            (UiLanguage::English, MonitorStatus::SettingsSaved) => "Settings saved".to_owned(),
            (UiLanguage::English, MonitorStatus::ValidationError) => {
                "Check the highlighted settings".to_owned()
            }
            (UiLanguage::English, MonitorStatus::ReadOnly) => "Settings are read-only".to_owned(),
            (UiLanguage::English, MonitorStatus::Error) => {
                "The action failed. Please try again".to_owned()
            }
        }
    }

    pub fn editor_error(self, error: &EditorError) -> String {
        match (self.language, error) {
            (UiLanguage::SimplifiedChinese, EditorError::ReadOnlySettings { detected }) => {
                format!("设置文件版本为 {detected}，目前不能修改或保存")
            }
            (UiLanguage::English, EditorError::ReadOnlySettings { detected }) => {
                format!("Settings version {detected} cannot be edited or saved by this version")
            }
            (UiLanguage::SimplifiedChinese, EditorError::IncompleteCaptureRegion) => {
                "识别区域的四项坐标需要完整填写；也可以点击按钮直接框选".into()
            }
            (UiLanguage::English, EditorError::IncompleteCaptureRegion) => {
                "Enter all four capture coordinates, or use the selection button".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::MissingCaptureRegion) => {
                "请先点击“框选识别区域”，拖动框住装备词缀".into()
            }
            (UiLanguage::English, EditorError::MissingCaptureRegion) => {
                "Select the capture area before starting recognition".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::InvalidCaptureRegion) => {
                "识别区域的位置必须是整数".into()
            }
            (UiLanguage::English, EditorError::InvalidCaptureRegion) => {
                "Capture position must use whole numbers".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::InvalidCaptureSize) => {
                "识别区域的宽度和高度必须是大于零的整数".into()
            }
            (UiLanguage::English, EditorError::InvalidCaptureSize) => {
                "Capture width and height must be positive whole numbers".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::IncompleteHudPosition) => {
                "提示位置的横向和纵向比例必须同时填写，或同时留空".into()
            }
            (UiLanguage::English, EditorError::IncompleteHudPosition) => {
                "Enter both horizontal and vertical alert positions, or leave both blank".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::InvalidHudPosition) => {
                "提示位置必须是零到一之间的数字".into()
            }
            (UiLanguage::English, EditorError::InvalidHudPosition) => {
                "Alert position must be a number from zero to one".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::InvalidAlertSound) => {
                "提醒声音必须是可用的波形声音文件；也可以留空使用内置声音".into()
            }
            (UiLanguage::English, EditorError::InvalidAlertSound) => {
                "Choose a valid PCM WAV file, or leave the field blank to use the built-in sound"
                    .into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::InvalidRequiredCount) => {
                "命中条数必须是整数".into()
            }
            (UiLanguage::English, EditorError::InvalidRequiredCount) => {
                "Required match count must be a whole number".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::InvalidNumber) => {
                "数值条件中有无法识别的数字".into()
            }
            (UiLanguage::English, EditorError::InvalidNumber) => {
                "A numeric rule contains an invalid number".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::InvalidNumberRange) => {
                "数值范围的最小值不能大于最大值".into()
            }
            (UiLanguage::English, EditorError::InvalidNumberRange) => {
                "The minimum value cannot be greater than the maximum value".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::TooManyResults) => {
                "最多可以添加八套命中方案".into()
            }
            (UiLanguage::English, EditorError::TooManyResults) => {
                "You can add up to eight match options".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::LastResultRequired) => {
                "至少要保留一套命中方案".into()
            }
            (UiLanguage::English, EditorError::LastResultRequired) => {
                "Keep at least one match option".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::TooManyConditions) => {
                "所有方案合计最多可以添加三十二条词缀".into()
            }
            (UiLanguage::English, EditorError::TooManyConditions) => {
                "All match options together can contain up to thirty-two affixes".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::LastConditionRequired) => {
                "每套命中方案至少要保留一条词缀".into()
            }
            (UiLanguage::English, EditorError::LastConditionRequired) => {
                "Each match option needs at least one affix".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::InvalidTemplate) => {
                "完整词缀模板必须包含词语；请直接粘贴游戏里显示的整条词缀".into()
            }
            (UiLanguage::English, EditorError::InvalidTemplate) => {
                "The full affix template must contain words. Paste the complete affix shown in game"
                    .into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::NoNumericSlots) => {
                "这条词缀模板没有可设置的数值".into()
            }
            (UiLanguage::English, EditorError::NoNumericSlots) => {
                "This affix template has no configurable numeric value".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::TooManyNumericConstraints) => {
                "数值条件数量超过了词缀模板里的数值数量".into()
            }
            (UiLanguage::English, EditorError::TooManyNumericConstraints) => {
                "There are more numeric rules than values in the affix template".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::EmptyQuickTemplate) => {
                "请填写要命中的完整词缀".into()
            }
            (UiLanguage::English, EditorError::EmptyQuickTemplate) => {
                "Enter the complete affix you want to match".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::EmptyResult) => {
                "请添加至少一套命中方案".into()
            }
            (UiLanguage::English, EditorError::EmptyResult) => {
                "Add at least one match option".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::EmptyConditionTemplate) => {
                "每条词缀条件都要填写完整词缀模板".into()
            }
            (UiLanguage::English, EditorError::EmptyConditionTemplate) => {
                "Every affix condition needs a full affix template".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::DuplicateResultName) => {
                "命中方案的名称不能重复".into()
            }
            (UiLanguage::English, EditorError::DuplicateResultName) => {
                "Match option names must be unique".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::DuplicateConditionName) => {
                "同一套方案里的词缀名称不能重复".into()
            }
            (UiLanguage::English, EditorError::DuplicateConditionName) => {
                "Affix names within one match option must be unique".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::InvalidRuleThreshold) => {
                "至少命中条数必须介于一和当前词缀条件总数之间".into()
            }
            (UiLanguage::English, EditorError::InvalidRuleThreshold) => {
                "Required match count must be between one and the number of affix conditions".into()
            }
            (UiLanguage::SimplifiedChinese, EditorError::InvalidRuleSet) => {
                "组合规则无法使用，请检查词缀模板和数值条件".into()
            }
            (UiLanguage::English, EditorError::InvalidRuleSet) => {
                "The combination rules are invalid. Check affix templates and numeric rules".into()
            }
        }
    }

    pub fn hot_key_conflict(self) -> &'static str {
        match self.language {
            UiLanguage::SimplifiedChinese => "快捷键已被其他程序占用，请在设置中更换",
            UiLanguage::English => "The shortcut is already in use. Choose another shortcut",
        }
    }

    pub fn sound_fallback(self) -> &'static str {
        match self.language {
            UiLanguage::SimplifiedChinese => "所选声音无法使用，已改用内置提示音",
            UiLanguage::English => {
                "The selected sound could not be used. The built-in sound will play instead"
            }
        }
    }

    pub fn sound_playback_failed(self) -> &'static str {
        match self.language {
            UiLanguage::SimplifiedChinese => "提示音播放失败，请检查声音设置；目标提醒仍在保护中",
            UiLanguage::English => {
                "The alert sound could not play. Check the sound setting; the target alert remains active"
            }
        }
    }

    pub fn runtime_failed(self) -> &'static str {
        match self.language {
            UiLanguage::SimplifiedChinese => "后台运行失败，请检查设置后重试",
            UiLanguage::English => "Background processing failed. Check the settings and try again",
        }
    }

    pub fn closing(self) -> &'static str {
        match self.language {
            UiLanguage::SimplifiedChinese => "正在安全关闭…",
            UiLanguage::English => "Closing safely…",
        }
    }

    pub fn applying_alert_settings(self) -> &'static str {
        match self.language {
            UiLanguage::SimplifiedChinese => "正在应用提醒设置…",
            UiLanguage::English => "Applying alert settings…",
        }
    }

    pub fn import_prompt(self) -> (&'static str, &'static str) {
        match self.language {
            UiLanguage::SimplifiedChinese => (
                "发现正式版一点零的设置。是否复制到新版本？\n\n原设置文件不会被修改。",
                "导入原设置",
            ),
            UiLanguage::English => (
                "Settings from version 1.0 were found. Copy them into the new version?\n\nThe original settings file will not be changed.",
                "Import previous settings",
            ),
        }
    }

    pub fn import_rejected(self) -> &'static str {
        match self.language {
            UiLanguage::SimplifiedChinese => "原设置版本较新，无法安全导入；原文件保持不变",
            UiLanguage::English => {
                "The previous settings are newer and cannot be imported safely. The original file is unchanged"
            }
        }
    }

    pub fn screenshot_report(
        self,
        is_match: bool,
        lines: &[String],
        load_ms: f64,
        preprocessing_ms: f64,
        recognition_ms: f64,
        evaluation_ms: f64,
    ) -> String {
        let outcome = match (self.language, is_match) {
            (UiLanguage::SimplifiedChinese, true) => "结果：命中目标",
            (UiLanguage::SimplifiedChinese, false) => "结果：没有命中",
            (UiLanguage::English, true) => "Result: target matched",
            (UiLanguage::English, false) => "Result: no match",
        };
        let timing = match self.language {
            UiLanguage::SimplifiedChinese => format!(
                "读取 {load_ms:.1} 毫秒  ·  画面处理 {preprocessing_ms:.1} 毫秒  ·  文字识别 {recognition_ms:.1} 毫秒  ·  规则判断 {evaluation_ms:.1} 毫秒"
            ),
            UiLanguage::English => format!(
                "Load {load_ms:.1} ms  ·  preprocessing {preprocessing_ms:.1} ms  ·  recognition {recognition_ms:.1} ms  ·  rule check {evaluation_ms:.1} ms"
            ),
        };
        let line_heading = match self.language {
            UiLanguage::SimplifiedChinese => "识别到的文字：",
            UiLanguage::English => "Recognized text:",
        };
        let recognized = if lines.is_empty() {
            match self.language {
                UiLanguage::SimplifiedChinese => "（没有识别到文字）".to_owned(),
                UiLanguage::English => "(No text was recognized)".to_owned(),
            }
        } else {
            lines.join("\r\n")
        };
        format!("{outcome}\r\n{timing}\r\n{line_heading}\r\n{recognized}")
    }

    pub fn screenshot_full_image_fallback(self) -> &'static str {
        match self.language {
            UiLanguage::SimplifiedChinese => {
                "提示：保存的识别区域不在这张截图内，本次改为识别整张截图。"
            }
            UiLanguage::English => {
                "Note: The saved capture area is outside this image, so the whole screenshot was analyzed."
            }
        }
    }

    pub fn hud_status(self, status: MonitorStatus) -> &'static str {
        match (self.language, status) {
            (UiLanguage::SimplifiedChinese, MonitorStatus::MatchFound) => "已命中目标",
            (UiLanguage::SimplifiedChinese, MonitorStatus::Monitoring) => "正在监控",
            (UiLanguage::SimplifiedChinese, _) => "等待开始",
            (UiLanguage::English, MonitorStatus::MatchFound) => "TARGET FOUND",
            (UiLanguage::English, MonitorStatus::Monitoring) => "MONITORING",
            (UiLanguage::English, _) => "READY",
        }
    }

    pub fn hud_text(
        self,
        status: MonitorStatus,
        target_summary: &str,
        elapsed: Option<&str>,
    ) -> String {
        let target = target_summary.trim();
        match (self.language, status, elapsed) {
            (UiLanguage::SimplifiedChinese, MonitorStatus::Monitoring, Some(elapsed)) => {
                format!("正在监控  {elapsed}\r\n当前目标：{target}")
            }
            (UiLanguage::English, MonitorStatus::Monitoring, Some(elapsed)) => {
                format!("MONITORING  {elapsed}\r\nCurrent target: {target}")
            }
            (UiLanguage::SimplifiedChinese, _, _) => {
                format!("{}\r\n当前目标：{target}", self.hud_status(status))
            }
            (UiLanguage::English, _, _) => {
                format!("{}\r\nCurrent target: {target}", self.hud_status(status))
            }
        }
    }

    pub fn hud_empty_target(self) -> &'static str {
        match self.language {
            UiLanguage::SimplifiedChinese => "尚未设置目标",
            UiLanguage::English => "No target configured",
        }
    }

    pub fn structured_rule_summary(
        self,
        name: &str,
        result_count: usize,
        condition_count: usize,
    ) -> String {
        let name = if name.trim().is_empty() {
            self.mode_multiple
        } else {
            name.trim()
        };
        match self.language {
            UiLanguage::SimplifiedChinese => {
                format!("{name}（{result_count} 套方案，{condition_count} 条词缀）")
            }
            UiLanguage::English => {
                format!("{name} ({result_count} options, {condition_count} affixes)")
            }
        }
    }

    pub fn hud_placement_instruction(self) -> &'static str {
        match self.language {
            UiLanguage::SimplifiedChinese => "拖到想要的位置\r\n再点击“保存提示位置”",
            UiLanguage::English => "Drag to the desired position\r\nThen click \"Save position\"",
        }
    }
}

static ZH: TextCatalog = TextCatalog {
    language: UiLanguage::SimplifiedChinese,
    window_title: "流放之路词缀提醒 — 原生迁移预览版零点一",
    heading: "做装词缀提醒 · 原生迁移预览版",
    subtitle: "设置想要的完整词缀。画面命中后会立即提醒。",
    game_label: "游戏版本",
    language_label: "界面语言",
    rule_mode_label: "命中方式",
    save_button: "保存设置",
    game_poe1: "流放之路一",
    game_poe2: "流放之路二",
    language_zh: "中文",
    language_en: "英文",
    mode_quick: "单条词缀",
    mode_multiple: "多词缀组合",
    quick_help: "直接粘贴游戏里显示的整条词缀；数值会自动识别。",
    quick_template_label: "完整词缀",
    ocr_language_label: "词缀文字",
    ocr_en: "英文词缀",
    ocr_zh: "繁体中文词缀",
    capture_region_label: "识别区域（建议点击右侧按钮框选）",
    region_x: "左边",
    region_y: "上边",
    region_width: "宽度",
    region_height: "高度",
    select_region_button: "框选识别区域",
    structured_single_help: "添加想要的词缀，再选择什么时候提醒。",
    structured_help: "每套命中方案互为“或者”；满足任意一套就提醒。",
    results_label: "命中方案",
    add_result: "添加另一套命中方案",
    delete_result: "删除当前方案",
    result_name: "方案名称",
    group_rule: "什么时候提醒",
    group_any: "任意一条命中就提醒",
    group_all: "全部命中才提醒",
    group_at_least: "达到指定条数就提醒",
    required_count: "最少条数",
    conditions_label: "想要的词缀",
    add_condition: "添加词缀",
    delete_condition: "删除词缀",
    condition_name: "条件名称",
    condition_template: "完整词缀模板",
    numeric_rules_label: "数值条件",
    numeric_comparison: "比较方式",
    numeric_value_help: "催化剂、品质或特殊效果可能改变屏幕显示值；数值条件只比较屏幕上实际显示的值，不推算基础值。",
    add_numeric_rule: "添加数值",
    delete_numeric_rule: "删除数值",
    numeric_ignore: "不限制数值",
    numeric_range: "在范围内",
    numeric_at_least: "至少",
    numeric_at_most: "至多",
    numeric_exact: "等于",
    first_value: "数值 / 下限",
    second_value: "上限",
    general_settings_label: "提醒与显示",
    keep_hud_visible: "持续显示画面提示",
    allow_overlay_capture: "截图中保留画面提示",
    hud_monitor: "显示器名称（可留空）",
    hud_x: "横向位置",
    hud_y: "纵向位置",
    hud_position_help: "点击按钮后，可直接拖动画面提示。",
    place_hud_button: "调整提示位置",
    save_hud_position_button: "保存提示位置",
    alert_sound: "提醒声音（留空使用内置声音）",
    browse_sound_button: "选择声音",
    default_sound_button: "使用内置",
    hotkey: "开始监控快捷键",
    status_label: "当前状态",
    screenshot_results_label: "截图识别结果",
    start_button: "开始监控",
    stop_button: "停止监控",
    screenshot_button: "识别截图",
};

static EN: TextCatalog = TextCatalog {
    language: UiLanguage::English,
    window_title: "Path of Exile Affix Alarm — Rust Preview 0.1.0",
    heading: "Crafting affix alarm · Rust Preview",
    subtitle: "Configure complete affixes. The app alerts immediately when the screen matches.",
    game_label: "Game",
    language_label: "Display language",
    rule_mode_label: "Match mode",
    save_button: "Save settings",
    game_poe1: "Path of Exile 1",
    game_poe2: "Path of Exile 2",
    language_zh: "Chinese",
    language_en: "English",
    mode_quick: "Single affix",
    mode_multiple: "Affix combinations",
    quick_help: "Paste the complete affix shown in game. Numeric values are detected automatically.",
    quick_template_label: "Complete affix",
    ocr_language_label: "Affix language",
    ocr_en: "English affixes",
    ocr_zh: "Traditional Chinese affixes",
    capture_region_label: "Capture area (use the selection button on the right)",
    region_x: "Left",
    region_y: "Top",
    region_width: "Width",
    region_height: "Height",
    select_region_button: "Select capture area",
    structured_single_help: "Add the affixes you want, then choose when to trigger the alert.",
    structured_help: "Match options are alternatives. Meeting any one option triggers the alert.",
    results_label: "Match options",
    add_result: "Add another match option",
    delete_result: "Delete this option",
    result_name: "Option name",
    group_rule: "Alert when",
    group_any: "Any affix matches",
    group_all: "All affixes match",
    group_at_least: "A chosen number match",
    required_count: "Minimum count",
    conditions_label: "Desired affixes",
    add_condition: "Add affix",
    delete_condition: "Delete affix",
    condition_name: "Condition name",
    condition_template: "Complete affix template",
    numeric_rules_label: "Numeric rules",
    numeric_comparison: "Comparison",
    numeric_value_help: "Catalysts, quality, and special effects can change the value shown on screen. Numeric conditions compare only the shown value and do not estimate its base value.",
    add_numeric_rule: "Add value",
    delete_numeric_rule: "Delete value",
    numeric_ignore: "Ignore value",
    numeric_range: "Within range",
    numeric_at_least: "At least",
    numeric_at_most: "At most",
    numeric_exact: "Exactly",
    first_value: "Value / min.",
    second_value: "Max.",
    general_settings_label: "Alerts and display",
    keep_hud_visible: "Keep the on-screen alert visible",
    allow_overlay_capture: "Include the on-screen alert in screenshots",
    hud_monitor: "Display name (optional)",
    hud_x: "Horizontal",
    hud_y: "Vertical",
    hud_position_help: "Click the button, then drag the on-screen alert.",
    place_hud_button: "Move status window",
    save_hud_position_button: "Save position",
    alert_sound: "Alert sound file (optional)",
    browse_sound_button: "Choose sound",
    default_sound_button: "Use built-in",
    hotkey: "Start monitoring shortcut",
    status_label: "Status",
    screenshot_results_label: "Screenshot results",
    start_button: "Start monitoring",
    stop_button: "Stop monitoring",
    screenshot_button: "Analyze screenshot",
};

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(catalog: &TextCatalog) -> Vec<&str> {
        vec![
            catalog.window_title,
            catalog.heading,
            catalog.subtitle,
            catalog.game_label,
            catalog.language_label,
            catalog.rule_mode_label,
            catalog.save_button,
            catalog.game_poe1,
            catalog.game_poe2,
            catalog.language_zh,
            catalog.language_en,
            catalog.mode_quick,
            catalog.mode_multiple,
            catalog.quick_help,
            catalog.quick_template_label,
            catalog.ocr_language_label,
            catalog.ocr_en,
            catalog.ocr_zh,
            catalog.capture_region_label,
            catalog.region_x,
            catalog.region_y,
            catalog.region_width,
            catalog.region_height,
            catalog.select_region_button,
            catalog.structured_single_help,
            catalog.structured_help,
            catalog.results_label,
            catalog.add_result,
            catalog.delete_result,
            catalog.result_name,
            catalog.group_rule,
            catalog.group_any,
            catalog.group_all,
            catalog.group_at_least,
            catalog.required_count,
            catalog.conditions_label,
            catalog.add_condition,
            catalog.delete_condition,
            catalog.condition_name,
            catalog.condition_template,
            catalog.numeric_rules_label,
            catalog.numeric_comparison,
            catalog.numeric_value_help,
            catalog.add_numeric_rule,
            catalog.delete_numeric_rule,
            catalog.numeric_ignore,
            catalog.numeric_range,
            catalog.numeric_at_least,
            catalog.numeric_at_most,
            catalog.numeric_exact,
            catalog.first_value,
            catalog.second_value,
            catalog.general_settings_label,
            catalog.keep_hud_visible,
            catalog.allow_overlay_capture,
            catalog.hud_monitor,
            catalog.hud_x,
            catalog.hud_y,
            catalog.hud_position_help,
            catalog.place_hud_button,
            catalog.save_hud_position_button,
            catalog.alert_sound,
            catalog.browse_sound_button,
            catalog.default_sound_button,
            catalog.hotkey,
            catalog.status_label,
            catalog.screenshot_results_label,
            catalog.start_button,
            catalog.stop_button,
            catalog.screenshot_button,
        ]
    }

    #[test]
    fn catalogs_are_complete_and_do_not_mix_languages() {
        for field in fields(UiLanguage::SimplifiedChinese.text()) {
            assert!(!field.trim().is_empty());
            assert!(
                !field
                    .chars()
                    .any(|character| character.is_ascii_alphabetic()),
                "Chinese text contains English letters: {field}"
            );
        }
        for field in fields(UiLanguage::English.text()) {
            assert!(!field.trim().is_empty());
            assert!(
                !field.chars().any(is_han),
                "English text contains Han characters: {field}"
            );
        }
    }

    #[test]
    fn dynamic_status_and_validation_text_do_not_mix_languages() {
        let errors = [
            EditorError::ReadOnlySettings { detected: 9 },
            EditorError::IncompleteCaptureRegion,
            EditorError::InvalidNumberRange,
            EditorError::TooManyConditions,
            EditorError::InvalidRuleSet,
        ];
        for error in errors {
            let english = UiLanguage::English.text().editor_error(&error);
            assert!(!english.chars().any(is_han), "{english}");
            let chinese = UiLanguage::SimplifiedChinese.text().editor_error(&error);
            assert!(
                !chinese
                    .chars()
                    .any(|character| character.is_ascii_alphabetic()),
                "{chinese}"
            );
        }
        let future = UiNotice::FutureSchema {
            detected: 9,
            supported: 3,
        };
        assert!(
            !UiLanguage::English
                .text()
                .status(MonitorStatus::ReadOnly, &future)
                .chars()
                .any(is_han)
        );

        let zh = UiLanguage::SimplifiedChinese.text();
        let en = UiLanguage::English.text();
        let (zh_import, zh_import_title) = zh.import_prompt();
        let (en_import, en_import_title) = en.import_prompt();
        let zh_dynamic = [
            zh.hot_key_conflict().to_owned(),
            zh.sound_fallback().to_owned(),
            zh.sound_playback_failed().to_owned(),
            zh.runtime_failed().to_owned(),
            zh.closing().to_owned(),
            zh.applying_alert_settings().to_owned(),
            zh_import.to_owned(),
            zh_import_title.to_owned(),
            zh.import_rejected().to_owned(),
            zh.hud_status(MonitorStatus::Monitoring).to_owned(),
            zh.hud_text(MonitorStatus::Monitoring, "暴击率", Some("00:12")),
            zh.hud_empty_target().to_owned(),
            zh.structured_rule_summary("命中方案", 2, 4),
            zh.hud_placement_instruction().to_owned(),
            zh.screenshot_report(false, &[], 1.0, 2.0, 3.0, 4.0),
            zh.screenshot_full_image_fallback().to_owned(),
        ];
        for text in zh_dynamic {
            assert!(
                !text
                    .chars()
                    .any(|character| character.is_ascii_alphabetic()),
                "{text}"
            );
        }
        let en_dynamic = [
            en.hot_key_conflict().to_owned(),
            en.sound_fallback().to_owned(),
            en.sound_playback_failed().to_owned(),
            en.runtime_failed().to_owned(),
            en.closing().to_owned(),
            en.applying_alert_settings().to_owned(),
            en_import.to_owned(),
            en_import_title.to_owned(),
            en.import_rejected().to_owned(),
            en.hud_status(MonitorStatus::Monitoring).to_owned(),
            en.hud_text(MonitorStatus::Monitoring, "Critical chance", Some("00:12")),
            en.hud_empty_target().to_owned(),
            en.structured_rule_summary("Match options", 2, 4),
            en.hud_placement_instruction().to_owned(),
            en.screenshot_report(false, &[], 1.0, 2.0, 3.0, 4.0),
            en.screenshot_full_image_fallback().to_owned(),
        ];
        for text in en_dynamic {
            assert!(!text.chars().any(is_han), "{text}");
        }
    }

    #[test]
    fn removed_safety_mode_is_never_exposed() {
        let combined = fields(UiLanguage::SimplifiedChinese.text())
            .into_iter()
            .chain(fields(UiLanguage::English.text()))
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        assert!(!combined.contains("mirror"));
        assert!(!combined.contains("careful"));
        assert!(!combined.contains("谨慎"));
        assert!(!combined.contains("鏡子"));
        assert!(!combined.contains("镜子"));
    }

    #[test]
    fn structured_rule_copy_uses_plain_match_option_language() {
        let zh = UiLanguage::SimplifiedChinese.text();
        assert_eq!(zh.conditions_label, "想要的词缀");
        assert_eq!(zh.group_rule, "什么时候提醒");
        assert!(zh.add_result.contains("另一套"));
        assert!(!zh.structured_help.contains("可接受结果"));

        let en = UiLanguage::English.text();
        assert_eq!(en.conditions_label, "Desired affixes");
        assert_eq!(en.group_rule, "Alert when");
        assert!(en.add_result.contains("another match option"));
        assert!(!en.structured_help.contains("acceptable result"));
    }

    fn is_han(character: char) -> bool {
        ('\u{3400}'..='\u{9fff}').contains(&character)
    }
}
