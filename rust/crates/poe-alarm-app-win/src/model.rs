use std::collections::HashSet;
use std::path::PathBuf;

use poe_alarm_core::{
    AcceptableResultGroup, AffixCondition, CompiledRuleSet, Decimal, FullLineAffixMatcher,
    NumericConstraint, NumericConstraintMode, ResultGroupMode, RuleSetDefinition,
    rules::{CURRENT_SCHEMA_VERSION as RULE_SCHEMA_VERSION, MAXIMUM_CONDITIONS, MAXIMUM_GROUPS},
};
use poe_alarm_settings::{
    AppSettings, GameProfile, HudPlacement, RuleEditorMode, ScreenRegion,
    TRADITIONAL_CHINESE_OCR_LANGUAGE, normalize_start_monitoring_hot_key,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Game {
    Poe1,
    #[default]
    Poe2,
}

impl Game {
    fn profile(self) -> GameProfile {
        match self {
            Self::Poe1 => GameProfile::Poe1,
            Self::Poe2 => GameProfile::Poe2,
        }
    }

    fn from_profile(profile: GameProfile) -> Self {
        match profile {
            GameProfile::Poe1 => Self::Poe1,
            GameProfile::Poe2 => Self::Poe2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UiLanguage {
    #[default]
    SimplifiedChinese,
    English,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OcrLanguage {
    #[default]
    English,
    TraditionalChinese,
}

impl OcrLanguage {
    fn from_setting(value: &str) -> Self {
        if value.eq_ignore_ascii_case(TRADITIONAL_CHINESE_OCR_LANGUAGE) {
            Self::TraditionalChinese
        } else {
            Self::English
        }
    }

    fn setting_value(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::TraditionalChinese => TRADITIONAL_CHINESE_OCR_LANGUAGE,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RuleMode {
    #[default]
    Quick,
    MultipleAffixes,
}

impl RuleMode {
    fn from_setting(mode: RuleEditorMode) -> Self {
        match mode {
            RuleEditorMode::Quick => Self::Quick,
            RuleEditorMode::Structured => Self::MultipleAffixes,
        }
    }

    fn setting_value(self) -> RuleEditorMode {
        match self {
            Self::Quick => RuleEditorMode::Quick,
            Self::MultipleAffixes => RuleEditorMode::Structured,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MonitorStatus {
    #[default]
    Ready,
    Starting,
    Monitoring,
    MatchFound,
    Stopping,
    TestingScreenshot,
    ScreenshotComplete,
    SavingSettings,
    SettingsSaved,
    ValidationError,
    ReadOnly,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    StartMonitoring,
    StopMonitoring,
    TestScreenshot,
    SaveSettings,
}

impl Operation {
    pub fn message_code(self) -> usize {
        match self {
            Self::StartMonitoring => 1,
            Self::StopMonitoring => 2,
            Self::TestScreenshot => 3,
            Self::SaveSettings => 4,
        }
    }

    pub fn from_message_code(code: usize) -> Option<Self> {
        match code {
            1 => Some(Self::StartMonitoring),
            2 => Some(Self::StopMonitoring),
            3 => Some(Self::TestScreenshot),
            4 => Some(Self::SaveSettings),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonitoringConfig {
    pub game: Game,
    pub rule_mode: RuleMode,
    /// Immutable configuration snapshot consumed by the future monitor adapter.
    pub settings: AppSettings,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BackgroundCommand {
    StartMonitoring(Box<MonitoringConfig>),
    StopMonitoring,
    TestScreenshot {
        config: Box<MonitoringConfig>,
        path: PathBuf,
    },
    SaveSettings(Box<AppSettings>),
}

impl BackgroundCommand {
    pub fn operation(&self) -> Operation {
        match self {
            Self::StartMonitoring(_) => Operation::StartMonitoring,
            Self::StopMonitoring => Operation::StopMonitoring,
            Self::TestScreenshot { .. } => Operation::TestScreenshot,
            Self::SaveSettings(_) => Operation::SaveSettings,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkFailure {
    BackgroundStopped,
    SaveFailed,
    FutureSchema { detected: u32, supported: u32 },
    External(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkOutcome {
    Succeeded,
    Failed(WorkFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackgroundCompletion {
    pub operation: Operation,
    pub outcome: WorkOutcome,
}

impl BackgroundCompletion {
    pub fn succeeded(operation: Operation) -> Self {
        Self {
            operation,
            outcome: WorkOutcome::Succeeded,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiAction {
    SelectGame(Game),
    SelectLanguage(UiLanguage),
    SelectOcrLanguage(OcrLanguage),
    SelectRuleMode(RuleMode),
    StartMonitoring,
    StopMonitoring,
    TestScreenshot(PathBuf),
    SaveSettings,
    BackgroundCompleted(BackgroundCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiAvailability {
    pub configuration_enabled: bool,
    pub selectors_enabled: bool,
    pub save_enabled: bool,
    pub start_enabled: bool,
    pub stop_enabled: bool,
    pub screenshot_test_enabled: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionEdit {
    pub x: String,
    pub y: String,
    pub width: String,
    pub height: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GlobalEdit {
    pub keep_hud_visible: bool,
    pub allow_overlay_capture: bool,
    pub hud_monitor: String,
    pub hud_x: String,
    pub hud_y: String,
    pub custom_alert_sound_path: String,
    pub start_monitoring_hot_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupEdit {
    pub name: String,
    pub mode: ResultGroupMode,
    pub required_count: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConditionEdit {
    pub name: String,
    pub template: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumericConstraintEdit {
    pub mode: NumericConstraintMode,
    pub first_value: String,
    pub second_value: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditorForm {
    pub quick_template: Option<String>,
    pub ocr_language: OcrLanguage,
    pub region: RegionEdit,
    pub global: GlobalEdit,
    pub group: Option<GroupEdit>,
    pub condition: Option<ConditionEdit>,
    pub numeric_constraint: Option<NumericConstraintEdit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorError {
    ReadOnlySettings { detected: u32 },
    IncompleteCaptureRegion,
    MissingCaptureRegion,
    InvalidCaptureRegion,
    InvalidCaptureSize,
    IncompleteHudPosition,
    InvalidHudPosition,
    InvalidAlertSound,
    InvalidRequiredCount,
    InvalidNumber,
    InvalidNumberRange,
    TooManyResults,
    LastResultRequired,
    TooManyConditions,
    LastConditionRequired,
    InvalidTemplate,
    NoNumericSlots,
    TooManyNumericConstraints,
    EmptyQuickTemplate,
    EmptyResult,
    EmptyConditionTemplate,
    DuplicateResultName,
    DuplicateConditionName,
    InvalidRuleThreshold,
    InvalidRuleSet,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiNotice {
    None,
    Validation(EditorError),
    Saved,
    SaveFailed,
    FutureSchema { detected: u32, supported: u32 },
    BackgroundFailed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    settings: AppSettings,
    game: Game,
    language: UiLanguage,
    rule_mode: RuleMode,
    selected_group: usize,
    selected_condition: usize,
    selected_numeric_constraint: usize,
    monitoring: bool,
    pending: Option<Operation>,
    status: MonitorStatus,
    notice: UiNotice,
    future_schema: Option<u32>,
    dirty: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self::from_settings(AppSettings::default(), None)
    }
}

impl AppState {
    pub fn from_settings(settings: AppSettings, future_schema: Option<u32>) -> Self {
        let mut settings = settings.normalize();
        synchronize_settings_numeric_slots(&mut settings);
        let game = Game::from_profile(settings.selected_game_profile);
        let language = if settings.ui_language.eq_ignore_ascii_case("en") {
            UiLanguage::English
        } else {
            UiLanguage::SimplifiedChinese
        };
        let rule_mode = RuleMode::from_setting(
            settings
                .profiles
                .get(game.profile())
                .selected_rules()
                .rule_editor_mode,
        );
        let mut state = Self {
            settings,
            game,
            language,
            rule_mode,
            selected_group: 0,
            selected_condition: 0,
            selected_numeric_constraint: 0,
            monitoring: false,
            pending: None,
            status: if future_schema.is_some() {
                MonitorStatus::ReadOnly
            } else {
                MonitorStatus::Ready
            },
            notice: future_schema.map_or(UiNotice::None, |detected| UiNotice::FutureSchema {
                detected,
                supported: poe_alarm_settings::CURRENT_SCHEMA_VERSION,
            }),
            future_schema,
            dirty: false,
        };
        state.clamp_editor_selection();
        state
    }

    pub fn settings(&self) -> &AppSettings {
        &self.settings
    }

    pub fn game(&self) -> Game {
        self.game
    }

    pub fn language(&self) -> UiLanguage {
        self.language
    }

    pub fn rule_mode(&self) -> RuleMode {
        self.rule_mode
    }

    pub fn ocr_language(&self) -> OcrLanguage {
        OcrLanguage::from_setting(&self.current_profile().ocr_language)
    }

    pub fn is_monitoring(&self) -> bool {
        self.monitoring
    }

    pub fn pending_operation(&self) -> Option<Operation> {
        self.pending
    }

    pub fn status(&self) -> MonitorStatus {
        self.status
    }

    pub fn notice(&self) -> &UiNotice {
        &self.notice
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn future_schema(&self) -> Option<u32> {
        self.future_schema
    }

    pub fn selected_group_index(&self) -> usize {
        self.selected_group
    }

    pub fn selected_condition_index(&self) -> usize {
        self.selected_condition
    }

    pub fn selected_numeric_constraint_index(&self) -> usize {
        self.selected_numeric_constraint
    }

    pub fn current_profile(&self) -> &poe_alarm_settings::GameProfileSettings {
        self.settings.profiles.get(self.game.profile())
    }

    pub fn current_rule_set(&self) -> Option<&RuleSetDefinition> {
        self.current_profile()
            .selected_rules()
            .structured_rule_set
            .as_ref()
    }

    pub fn current_group(&self) -> Option<&AcceptableResultGroup> {
        self.current_rule_set()?.groups.get(self.selected_group)
    }

    pub fn current_condition(&self) -> Option<&AffixCondition> {
        self.current_group()?
            .conditions
            .get(self.selected_condition)
    }

    pub fn current_numeric_constraint(&self) -> Option<&NumericConstraint> {
        self.current_condition()?
            .numeric_constraints
            .get(self.selected_numeric_constraint)
    }

    pub fn availability(&self) -> UiAvailability {
        let idle = self.pending.is_none();
        let writable = self.future_schema.is_none();
        let configuration_enabled = idle && !self.monitoring && writable;
        UiAvailability {
            configuration_enabled,
            selectors_enabled: configuration_enabled,
            // Native EDIT controls do not write through on every keystroke. Keeping Save
            // available lets the click transactionally read and validate the current form.
            save_enabled: configuration_enabled,
            start_enabled: idle && !self.monitoring && writable,
            stop_enabled: idle && self.monitoring,
            // Screenshot analysis uses the same recognition chain and asks the runtime to stop
            // live monitoring first. Choosing a file remains available while live monitoring.
            screenshot_test_enabled: idle && writable,
        }
    }

    /// Commits every visible edit as one transaction. Invalid text never partly updates settings.
    pub fn commit_form(&mut self, form: EditorForm) -> Result<(), EditorError> {
        self.require_writable()?;
        let mut next = self.settings.clone();
        let profile = next.profiles.get_mut(self.game.profile());
        if let Some(template) = form.quick_template {
            profile.selected_rules_mut().target_affix = template.trim().to_owned();
        }
        profile.capture_region = parse_region(&form.region)?;

        next.keep_hud_visible = form.global.keep_hud_visible;
        next.allow_overlay_capture = form.global.allow_overlay_capture;
        next.hud_placement = parse_hud_placement(&form.global)?;
        next.custom_alert_sound_path = trimmed_option(&form.global.custom_alert_sound_path);
        next.start_monitoring_hot_key =
            normalize_start_monitoring_hot_key(&form.global.start_monitoring_hot_key).to_owned();

        if self.rule_mode == RuleMode::MultipleAffixes {
            let parsed_numeric_constraint = form
                .numeric_constraint
                .as_ref()
                .map(parse_numeric_constraint)
                .transpose()?;
            let rules = profile
                .selected_rules_mut()
                .structured_rule_set
                .as_mut()
                .ok_or(EditorError::EmptyResult)?;
            let group = rules
                .groups
                .get_mut(self.selected_group)
                .ok_or(EditorError::EmptyResult)?;
            if let Some(edit) = form.group {
                group.name = edit.name.trim().to_owned();
                group.mode = edit.mode;
                group.required_count = edit
                    .required_count
                    .trim()
                    .parse()
                    .map_err(|_| EditorError::InvalidRequiredCount)?;
            }
            let condition = group
                .conditions
                .get_mut(self.selected_condition)
                .ok_or(EditorError::LastConditionRequired)?;
            let apply_numeric_after_sync = if let Some(constraint) = &parsed_numeric_constraint {
                if let Some(current) = condition
                    .numeric_constraints
                    .get_mut(self.selected_numeric_constraint)
                {
                    *current = constraint.clone();
                    false
                } else {
                    true
                }
            } else {
                false
            };
            if let Some(edit) = form.condition {
                condition.name = edit.name.trim().to_owned();
                condition.template = edit.template.trim().to_owned();
            }
            synchronize_condition_numeric_slots(condition);
            if apply_numeric_after_sync {
                let constraint = condition
                    .numeric_constraints
                    .get_mut(self.selected_numeric_constraint)
                    .ok_or(EditorError::NoNumericSlots)?;
                *constraint =
                    parsed_numeric_constraint.expect("a deferred numeric edit was already parsed");
            }
        }
        next.selected_game_profile = self.game.profile();
        next.ui_language = match self.language {
            UiLanguage::SimplifiedChinese => "zh-CN",
            UiLanguage::English => "en",
        }
        .to_owned();
        self.settings = next.normalize();
        self.dirty = true;
        self.notice = UiNotice::None;
        if !matches!(self.status, MonitorStatus::Monitoring) {
            self.status = MonitorStatus::Ready;
        }
        Ok(())
    }

    pub fn select_group(&mut self, index: usize) -> bool {
        let Some(rules) = self.current_rule_set() else {
            return false;
        };
        if index >= rules.groups.len() {
            return false;
        }
        self.selected_group = index;
        self.selected_condition = 0;
        self.selected_numeric_constraint = 0;
        true
    }

    pub fn select_condition(&mut self, index: usize) -> bool {
        let Some(group) = self.current_group() else {
            return false;
        };
        if index >= group.conditions.len() {
            return false;
        }
        self.selected_condition = index;
        self.selected_numeric_constraint = 0;
        true
    }

    pub fn select_numeric_constraint(&mut self, index: usize) -> bool {
        let Some(condition) = self.current_condition() else {
            return false;
        };
        if index >= condition.numeric_constraints.len() {
            return false;
        }
        self.selected_numeric_constraint = index;
        true
    }

    pub fn add_result(&mut self) -> Result<(), EditorError> {
        self.require_writable()?;
        self.ensure_structured_rules();
        let rules = self
            .current_rule_set_mut()
            .expect("rules were just created");
        if rules.groups.len() >= MAXIMUM_GROUPS {
            return Err(EditorError::TooManyResults);
        }
        let total = rules
            .groups
            .iter()
            .map(|group| group.conditions.len())
            .sum::<usize>();
        if total >= MAXIMUM_CONDITIONS {
            return Err(EditorError::TooManyConditions);
        }
        rules.groups.push(default_group());
        self.selected_group = rules.groups.len() - 1;
        self.selected_condition = 0;
        self.selected_numeric_constraint = 0;
        self.mark_edited();
        Ok(())
    }

    pub fn remove_result(&mut self) -> Result<(), EditorError> {
        self.require_writable()?;
        let selected_group = self.selected_group;
        let rules = self
            .current_rule_set_mut()
            .ok_or(EditorError::LastResultRequired)?;
        if rules.groups.len() <= 1 {
            return Err(EditorError::LastResultRequired);
        }
        rules.groups.remove(selected_group);
        let remaining = rules.groups.len();
        self.selected_group = selected_group.min(remaining - 1);
        self.selected_condition = 0;
        self.selected_numeric_constraint = 0;
        self.mark_edited();
        Ok(())
    }

    pub fn add_condition(&mut self) -> Result<(), EditorError> {
        self.require_writable()?;
        let total = self
            .current_rule_set()
            .map(|rules| {
                rules
                    .groups
                    .iter()
                    .map(|group| group.conditions.len())
                    .sum()
            })
            .unwrap_or(0usize);
        if total >= MAXIMUM_CONDITIONS {
            return Err(EditorError::TooManyConditions);
        }
        let group = self.current_group_mut().ok_or(EditorError::EmptyResult)?;
        group.conditions.push(AffixCondition::default());
        self.selected_condition = group.conditions.len() - 1;
        self.selected_numeric_constraint = 0;
        self.mark_edited();
        Ok(())
    }

    pub fn remove_condition(&mut self) -> Result<(), EditorError> {
        self.require_writable()?;
        let selected_condition = self.selected_condition;
        let group = self
            .current_group_mut()
            .ok_or(EditorError::LastConditionRequired)?;
        if group.conditions.len() <= 1 {
            return Err(EditorError::LastConditionRequired);
        }
        group.conditions.remove(selected_condition);
        let remaining = group.conditions.len();
        self.selected_condition = selected_condition.min(remaining - 1);
        self.selected_numeric_constraint = 0;
        self.mark_edited();
        Ok(())
    }

    pub fn add_numeric_constraint(&mut self) -> Result<(), EditorError> {
        self.require_writable()?;
        let condition = self
            .current_condition_mut()
            .ok_or(EditorError::LastConditionRequired)?;
        let matcher = FullLineAffixMatcher::new(&condition.template)
            .map_err(|_| EditorError::InvalidTemplate)?;
        let slot_count = matcher
            .template()
            .tokens
            .iter()
            .filter(|token| !token.kind.is_word())
            .count();
        if slot_count == 0 {
            return Err(EditorError::NoNumericSlots);
        }
        if condition.numeric_constraints.len() >= slot_count {
            return Err(EditorError::TooManyNumericConstraints);
        }
        condition
            .numeric_constraints
            .push(NumericConstraint::ignored());
        self.selected_numeric_constraint = condition.numeric_constraints.len() - 1;
        self.mark_edited();
        Ok(())
    }

    pub fn remove_numeric_constraint(&mut self) -> Result<(), EditorError> {
        self.require_writable()?;
        let selected_numeric_constraint = self.selected_numeric_constraint;
        let condition = self
            .current_condition_mut()
            .ok_or(EditorError::NoNumericSlots)?;
        if condition.numeric_constraints.is_empty() {
            return Err(EditorError::NoNumericSlots);
        }
        condition
            .numeric_constraints
            .remove(selected_numeric_constraint);
        let remaining = condition.numeric_constraints.len();
        self.selected_numeric_constraint =
            selected_numeric_constraint.min(remaining.saturating_sub(1));
        self.mark_edited();
        Ok(())
    }

    pub fn validate_for_save(&self) -> Result<(), EditorError> {
        self.require_writable()?;
        match self.rule_mode {
            RuleMode::Quick => {
                let target_affix = &self.current_profile().selected_rules().target_affix;
                if target_affix.trim().is_empty() {
                    Err(EditorError::EmptyQuickTemplate)
                } else {
                    FullLineAffixMatcher::new(target_affix)
                        .map(|_| ())
                        .map_err(|_| EditorError::InvalidTemplate)
                }
            }
            RuleMode::MultipleAffixes => validate_rule_set(
                self.current_profile()
                    .selected_rules()
                    .structured_rule_set
                    .as_ref()
                    .ok_or(EditorError::EmptyResult)?,
            ),
        }
    }

    /// Queues persistence for a native interaction that already produced a valid settings value
    /// (for example, the drag-selection overlay). Unlike the explicit Save button, this does not
    /// require the rule editor to be complete before preserving an independently valid region.
    pub fn begin_automatic_save(&mut self) -> Option<BackgroundCommand> {
        if self.future_schema.is_some() || self.pending.is_some() {
            return None;
        }
        self.pending = Some(Operation::SaveSettings);
        self.status = MonitorStatus::SavingSettings;
        Some(BackgroundCommand::SaveSettings(Box::new(
            self.settings.clone().normalize(),
        )))
    }

    fn validate_for_recognition(&self) -> Result<(), EditorError> {
        self.validate_for_save()?;
        if self.current_profile().capture_region.is_none() {
            return Err(EditorError::MissingCaptureRegion);
        }
        Ok(())
    }

    /// Applies one typed event and returns background work to queue, if any.
    pub fn apply(&mut self, action: UiAction) -> Option<BackgroundCommand> {
        match action {
            UiAction::SelectLanguage(language) => {
                self.language = language;
                self.settings.ui_language = match language {
                    UiLanguage::SimplifiedChinese => "zh-CN",
                    UiLanguage::English => "en",
                }
                .to_owned();
                if self.future_schema.is_none() {
                    self.dirty = true;
                }
                None
            }
            UiAction::SelectGame(game) if self.availability().selectors_enabled => {
                self.game = game;
                self.settings.selected_game_profile = game.profile();
                self.rule_mode = RuleMode::from_setting(
                    self.current_profile().selected_rules().rule_editor_mode,
                );
                self.selected_group = 0;
                self.selected_condition = 0;
                self.selected_numeric_constraint = 0;
                self.clamp_editor_selection();
                self.mark_edited();
                None
            }
            UiAction::SelectOcrLanguage(language) if self.availability().selectors_enabled => {
                self.current_profile_mut().ocr_language = language.setting_value().to_owned();
                self.rule_mode = RuleMode::from_setting(
                    self.current_profile().selected_rules().rule_editor_mode,
                );
                if self.rule_mode == RuleMode::MultipleAffixes {
                    self.ensure_structured_rules();
                }
                self.selected_group = 0;
                self.selected_condition = 0;
                self.selected_numeric_constraint = 0;
                self.clamp_editor_selection();
                self.mark_edited();
                None
            }
            UiAction::SelectRuleMode(mode) if self.availability().selectors_enabled => {
                self.rule_mode = mode;
                self.current_profile_mut()
                    .selected_rules_mut()
                    .rule_editor_mode = mode.setting_value();
                if mode == RuleMode::MultipleAffixes {
                    self.ensure_structured_rules();
                }
                self.selected_group = 0;
                self.selected_condition = 0;
                self.selected_numeric_constraint = 0;
                self.mark_edited();
                None
            }
            UiAction::StartMonitoring if self.availability().start_enabled => {
                if let Err(error) = self.validate_for_recognition() {
                    self.validation_failed(error);
                    return None;
                }
                self.pending = Some(Operation::StartMonitoring);
                self.status = MonitorStatus::Starting;
                Some(BackgroundCommand::StartMonitoring(Box::new(self.config())))
            }
            UiAction::StopMonitoring if self.availability().stop_enabled => {
                self.pending = Some(Operation::StopMonitoring);
                self.status = MonitorStatus::Stopping;
                Some(BackgroundCommand::StopMonitoring)
            }
            UiAction::TestScreenshot(path) if self.availability().screenshot_test_enabled => {
                if let Err(error) = self.validate_for_recognition() {
                    self.validation_failed(error);
                    return None;
                }
                self.pending = Some(Operation::TestScreenshot);
                self.status = MonitorStatus::TestingScreenshot;
                Some(BackgroundCommand::TestScreenshot {
                    config: Box::new(self.config()),
                    path,
                })
            }
            UiAction::SaveSettings if self.availability().configuration_enabled => {
                if let Err(error) = self.validate_for_save() {
                    self.validation_failed(error);
                    return None;
                }
                self.pending = Some(Operation::SaveSettings);
                self.status = MonitorStatus::SavingSettings;
                Some(BackgroundCommand::SaveSettings(Box::new(
                    self.settings.clone().normalize(),
                )))
            }
            UiAction::BackgroundCompleted(completion)
                if self.pending == Some(completion.operation) =>
            {
                self.pending = None;
                match completion.outcome {
                    WorkOutcome::Succeeded => match completion.operation {
                        Operation::StartMonitoring => {
                            self.monitoring = true;
                            self.status = MonitorStatus::Monitoring;
                        }
                        Operation::StopMonitoring => {
                            self.monitoring = false;
                            self.status = MonitorStatus::Ready;
                        }
                        Operation::TestScreenshot => {
                            self.monitoring = false;
                            self.status = MonitorStatus::ScreenshotComplete;
                        }
                        Operation::SaveSettings => {
                            self.status = MonitorStatus::SettingsSaved;
                            self.notice = UiNotice::Saved;
                            self.dirty = false;
                        }
                    },
                    WorkOutcome::Failed(WorkFailure::FutureSchema {
                        detected,
                        supported,
                    }) => {
                        self.future_schema = Some(detected);
                        self.status = MonitorStatus::ReadOnly;
                        self.notice = UiNotice::FutureSchema {
                            detected,
                            supported,
                        };
                    }
                    WorkOutcome::Failed(WorkFailure::SaveFailed) => {
                        self.status = MonitorStatus::Error;
                        self.notice = UiNotice::SaveFailed;
                    }
                    WorkOutcome::Failed(_) => {
                        if completion.operation == Operation::TestScreenshot {
                            self.monitoring = false;
                        }
                        self.status = MonitorStatus::Error;
                        self.notice = UiNotice::BackgroundFailed;
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub fn report_validation_error(&mut self, error: EditorError) {
        self.validation_failed(error);
    }

    /// Applies a region returned by the native drag-selection overlay.
    pub fn set_capture_region(&mut self, region: ScreenRegion) -> Result<(), EditorError> {
        self.require_writable()?;
        if !region.is_valid() {
            return Err(EditorError::InvalidCaptureSize);
        }
        self.current_profile_mut().capture_region = Some(region);
        self.mark_edited();
        Ok(())
    }

    pub fn set_hud_position(
        &mut self,
        relative_x: f64,
        relative_y: f64,
    ) -> Result<(), EditorError> {
        self.require_writable()?;
        if !relative_x.is_finite()
            || !relative_y.is_finite()
            || !(0.0..=1.0).contains(&relative_x)
            || !(0.0..=1.0).contains(&relative_y)
        {
            return Err(EditorError::InvalidHudPosition);
        }
        self.settings.hud_placement.relative_x = Some(relative_x);
        self.settings.hud_placement.relative_y = Some(relative_y);
        self.mark_edited();
        Ok(())
    }

    /// Keeps the live session available for the explicit stop/acknowledge action while the
    /// blocking red alert owns input.
    pub fn report_match_found(&mut self) {
        if self.pending == Some(Operation::StartMonitoring) {
            self.pending = None;
        }
        self.monitoring = true;
        self.status = MonitorStatus::MatchFound;
        self.notice = UiNotice::None;
    }

    /// Converts an asynchronous runtime fault into an idle, recoverable UI state.
    pub fn report_runtime_failure(&mut self) {
        self.pending = None;
        self.monitoring = false;
        self.status = MonitorStatus::Error;
        self.notice = UiNotice::BackgroundFailed;
    }

    fn require_writable(&self) -> Result<(), EditorError> {
        match self.future_schema {
            Some(detected) => Err(EditorError::ReadOnlySettings { detected }),
            None => Ok(()),
        }
    }

    fn validation_failed(&mut self, error: EditorError) {
        self.status = MonitorStatus::ValidationError;
        self.notice = UiNotice::Validation(error);
    }

    fn mark_edited(&mut self) {
        self.dirty = true;
        self.notice = UiNotice::None;
        if !self.monitoring && self.pending.is_none() {
            self.status = MonitorStatus::Ready;
        }
    }

    fn current_profile_mut(&mut self) -> &mut poe_alarm_settings::GameProfileSettings {
        self.settings.profiles.get_mut(self.game.profile())
    }

    fn current_rule_set_mut(&mut self) -> Option<&mut RuleSetDefinition> {
        self.current_profile_mut()
            .selected_rules_mut()
            .structured_rule_set
            .as_mut()
    }

    fn current_group_mut(&mut self) -> Option<&mut AcceptableResultGroup> {
        let index = self.selected_group;
        self.current_rule_set_mut()?.groups.get_mut(index)
    }

    fn current_condition_mut(&mut self) -> Option<&mut AffixCondition> {
        let index = self.selected_condition;
        self.current_group_mut()?.conditions.get_mut(index)
    }

    fn ensure_structured_rules(&mut self) {
        let profile = self.current_profile_mut();
        let rules = profile
            .selected_rules_mut()
            .structured_rule_set
            .get_or_insert_with(default_rule_set);
        if rules.groups.is_empty() {
            rules.groups.push(default_group());
        }
    }

    fn clamp_editor_selection(&mut self) {
        let Some(rules) = self.current_rule_set() else {
            self.selected_group = 0;
            self.selected_condition = 0;
            self.selected_numeric_constraint = 0;
            return;
        };
        let selected_group = self
            .selected_group
            .min(rules.groups.len().saturating_sub(1));
        let condition_count = rules
            .groups
            .get(selected_group)
            .map_or(0, |group| group.conditions.len());
        let selected_condition = self
            .selected_condition
            .min(condition_count.saturating_sub(1));
        let constraint_count = rules
            .groups
            .get(selected_group)
            .and_then(|group| group.conditions.get(selected_condition))
            .map_or(0, |condition| condition.numeric_constraints.len());
        let selected_numeric_constraint = self
            .selected_numeric_constraint
            .min(constraint_count.saturating_sub(1));
        self.selected_group = selected_group;
        self.selected_condition = selected_condition;
        self.selected_numeric_constraint = selected_numeric_constraint;
    }

    fn config(&self) -> MonitoringConfig {
        MonitoringConfig {
            game: self.game,
            rule_mode: self.rule_mode,
            settings: self.settings.clone().normalize(),
        }
    }
}

fn default_rule_set() -> RuleSetDefinition {
    RuleSetDefinition {
        schema_version: RULE_SCHEMA_VERSION,
        name: String::new(),
        groups: vec![default_group()],
    }
}

fn default_group() -> AcceptableResultGroup {
    AcceptableResultGroup {
        name: String::new(),
        mode: ResultGroupMode::Any,
        required_count: 1,
        conditions: vec![AffixCondition::default()],
    }
}

/// Keeps the editable numeric rules aligned with the numeric slots parsed by the core matcher.
/// Invalid or temporarily empty templates deliberately leave the previous vector untouched so a
/// user can restore the template without losing numeric edits.
fn synchronize_condition_numeric_slots(condition: &mut AffixCondition) -> bool {
    let Ok(matcher) = FullLineAffixMatcher::new(&condition.template) else {
        return false;
    };
    let slot_count = matcher
        .template()
        .tokens
        .iter()
        .filter(|token| !token.kind.is_word())
        .count();
    let changed = condition.numeric_constraints.len() != slot_count;
    condition
        .numeric_constraints
        .resize_with(slot_count, NumericConstraint::ignored);
    changed
}

fn synchronize_rule_set_numeric_slots(rule_set: Option<&mut RuleSetDefinition>) {
    let Some(rule_set) = rule_set else {
        return;
    };
    for group in &mut rule_set.groups {
        for condition in &mut group.conditions {
            synchronize_condition_numeric_slots(condition);
        }
    }
}

fn synchronize_game_profile_numeric_slots(profile: &mut poe_alarm_settings::GameProfileSettings) {
    synchronize_rule_set_numeric_slots(profile.rules_for_mut("en").structured_rule_set.as_mut());
    synchronize_rule_set_numeric_slots(
        profile
            .rules_for_mut(TRADITIONAL_CHINESE_OCR_LANGUAGE)
            .structured_rule_set
            .as_mut(),
    );
}

fn synchronize_settings_numeric_slots(settings: &mut AppSettings) {
    synchronize_game_profile_numeric_slots(&mut settings.profiles.poe1);
    synchronize_game_profile_numeric_slots(&mut settings.profiles.poe2);
}

fn parse_region(edit: &RegionEdit) -> Result<Option<ScreenRegion>, EditorError> {
    let values = [
        edit.x.trim(),
        edit.y.trim(),
        edit.width.trim(),
        edit.height.trim(),
    ];
    if values.iter().all(|value| value.is_empty()) {
        return Ok(None);
    }
    if values.iter().any(|value| value.is_empty()) {
        return Err(EditorError::IncompleteCaptureRegion);
    }
    let x = values[0]
        .parse::<i32>()
        .map_err(|_| EditorError::InvalidCaptureRegion)?;
    let y = values[1]
        .parse::<i32>()
        .map_err(|_| EditorError::InvalidCaptureRegion)?;
    let width = values[2]
        .parse::<i32>()
        .map_err(|_| EditorError::InvalidCaptureSize)?;
    let height = values[3]
        .parse::<i32>()
        .map_err(|_| EditorError::InvalidCaptureSize)?;
    if width <= 0 || height <= 0 {
        return Err(EditorError::InvalidCaptureSize);
    }
    Ok(Some(ScreenRegion::new(x, y, width, height)))
}

fn parse_hud_placement(edit: &GlobalEdit) -> Result<HudPlacement, EditorError> {
    let x = edit.hud_x.trim();
    let y = edit.hud_y.trim();
    if x.is_empty() && y.is_empty() {
        return Ok(HudPlacement {
            monitor_device_name: trimmed_option(&edit.hud_monitor),
            relative_x: None,
            relative_y: None,
        });
    }
    if x.is_empty() || y.is_empty() {
        return Err(EditorError::IncompleteHudPosition);
    }
    let x = x
        .parse::<f64>()
        .map_err(|_| EditorError::InvalidHudPosition)?;
    let y = y
        .parse::<f64>()
        .map_err(|_| EditorError::InvalidHudPosition)?;
    if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err(EditorError::InvalidHudPosition);
    }
    Ok(HudPlacement {
        monitor_device_name: trimmed_option(&edit.hud_monitor),
        relative_x: Some(x),
        relative_y: Some(y),
    })
}

fn parse_numeric_constraint(
    edit: &NumericConstraintEdit,
) -> Result<NumericConstraint, EditorError> {
    let parse = |value: &str| {
        value
            .trim()
            .parse::<Decimal>()
            .map_err(|_| EditorError::InvalidNumber)
    };
    match edit.mode {
        NumericConstraintMode::Ignore => Ok(NumericConstraint::ignored()),
        NumericConstraintMode::RangeInclusive => {
            let minimum = parse(&edit.first_value)?;
            let maximum = parse(&edit.second_value)?;
            if minimum > maximum {
                return Err(EditorError::InvalidNumberRange);
            }
            Ok(NumericConstraint::range(minimum, maximum))
        }
        NumericConstraintMode::AtLeast => {
            Ok(NumericConstraint::at_least(parse(&edit.first_value)?))
        }
        NumericConstraintMode::AtMost => Ok(NumericConstraint::at_most(parse(&edit.first_value)?)),
        NumericConstraintMode::Exactly => Ok(NumericConstraint::exactly(parse(&edit.first_value)?)),
    }
}

fn validate_rule_set(rules: &RuleSetDefinition) -> Result<(), EditorError> {
    if rules.groups.is_empty() {
        return Err(EditorError::EmptyResult);
    }
    if rules.groups.len() > MAXIMUM_GROUPS {
        return Err(EditorError::TooManyResults);
    }
    let condition_count = rules
        .groups
        .iter()
        .map(|group| group.conditions.len())
        .sum::<usize>();
    if condition_count > MAXIMUM_CONDITIONS {
        return Err(EditorError::TooManyConditions);
    }
    let mut group_names = HashSet::new();
    for group in &rules.groups {
        if !group.name.is_empty() && !group_names.insert(group.name.as_str()) {
            return Err(EditorError::DuplicateResultName);
        }
        if group.conditions.is_empty() {
            return Err(EditorError::LastConditionRequired);
        }
        if group.mode == ResultGroupMode::AtLeast
            && !(1..=group.conditions.len()).contains(&group.required_count)
        {
            return Err(EditorError::InvalidRuleThreshold);
        }
        let mut condition_names = HashSet::new();
        for condition in &group.conditions {
            if condition.template.trim().is_empty() {
                return Err(EditorError::EmptyConditionTemplate);
            }
            if !condition.name.is_empty() && !condition_names.insert(condition.name.as_str()) {
                return Err(EditorError::DuplicateConditionName);
            }
            let matcher = FullLineAffixMatcher::new(&condition.template)
                .map_err(|_| EditorError::InvalidTemplate)?;
            let slots = matcher
                .template()
                .tokens
                .iter()
                .filter(|token| !token.kind.is_word())
                .count();
            if condition.numeric_constraints.len() > slots {
                return Err(EditorError::TooManyNumericConstraints);
            }
            for constraint in &condition.numeric_constraints {
                if !valid_numeric_constraint(constraint) {
                    return Err(EditorError::InvalidNumberRange);
                }
            }
        }
    }
    CompiledRuleSet::compile(rules.clone())
        .map(|_| ())
        .map_err(|_| EditorError::InvalidRuleSet)
}

fn valid_numeric_constraint(constraint: &NumericConstraint) -> bool {
    match constraint.mode {
        NumericConstraintMode::Ignore => true,
        NumericConstraintMode::RangeInclusive => matches!(
            (constraint.minimum, constraint.maximum),
            (Some(minimum), Some(maximum)) if minimum <= maximum
        ),
        NumericConstraintMode::AtLeast => constraint.minimum.is_some(),
        NumericConstraintMode::AtMost => constraint.maximum.is_some(),
        NumericConstraintMode::Exactly => constraint.expected.is_some(),
    }
}

fn trimmed_option(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_quick_form(template: &str) -> EditorForm {
        EditorForm {
            quick_template: Some(template.to_owned()),
            ocr_language: OcrLanguage::English,
            region: RegionEdit {
                x: "10".into(),
                y: "20".into(),
                width: "300".into(),
                height: "400".into(),
            },
            global: GlobalEdit {
                keep_hud_visible: true,
                allow_overlay_capture: false,
                hud_x: "0.25".into(),
                hud_y: "0.75".into(),
                start_monitoring_hot_key: "Alt+F10".into(),
                ..GlobalEdit::default()
            },
            ..EditorForm::default()
        }
    }

    fn structured_form(
        template: &str,
        mode: NumericConstraintMode,
        first: &str,
        second: &str,
    ) -> EditorForm {
        EditorForm {
            ocr_language: OcrLanguage::English,
            global: GlobalEdit {
                keep_hud_visible: true,
                allow_overlay_capture: true,
                start_monitoring_hot_key: "Ctrl+Shift+F10".into(),
                ..GlobalEdit::default()
            },
            group: Some(GroupEdit {
                name: "result".into(),
                mode: ResultGroupMode::AtLeast,
                required_count: "1".into(),
            }),
            condition: Some(ConditionEdit {
                name: "critical".into(),
                template: template.into(),
            }),
            numeric_constraint: Some(NumericConstraintEdit {
                mode,
                first_value: first.into(),
                second_value: second.into(),
            }),
            ..EditorForm::default()
        }
    }

    fn structured_form_without_numeric(template: &str) -> EditorForm {
        let mut form = structured_form(template, NumericConstraintMode::Ignore, "", "");
        form.numeric_constraint = None;
        form
    }

    fn structured_settings(
        template: &str,
        numeric_constraints: Vec<NumericConstraint>,
    ) -> AppSettings {
        let mut settings = AppSettings::default();
        let profile = &mut settings.profiles.poe1;
        let rules = profile.selected_rules_mut();
        rules.rule_editor_mode = RuleEditorMode::Structured;
        rules.structured_rule_set = Some(RuleSetDefinition {
            schema_version: RULE_SCHEMA_VERSION,
            name: "damage".into(),
            groups: vec![AcceptableResultGroup {
                name: "result".into(),
                mode: ResultGroupMode::All,
                required_count: 1,
                conditions: vec![AffixCondition::new("damage", template, numeric_constraints)],
            }],
        });
        settings
    }

    fn evaluate_current_rules(state: &AppState, candidate: &str) -> bool {
        CompiledRuleSet::compile(state.current_rule_set().unwrap().clone())
            .unwrap()
            .evaluate(&[candidate.into()])
            .is_match
    }

    #[test]
    fn game_profiles_remain_isolated() {
        let mut state = AppState::default();
        state.apply(UiAction::SelectGame(Game::Poe1));
        state
            .commit_form(valid_quick_form("#% increased Attack Speed"))
            .unwrap();
        state.apply(UiAction::SelectGame(Game::Poe2));
        state.commit_form(valid_quick_form("+#%暴擊率")).unwrap();

        assert_eq!(
            state.settings.profiles.poe1.rules_for("en").target_affix,
            "#% increased Attack Speed"
        );
        assert_eq!(
            state.settings.profiles.poe2.rules_for("en").target_affix,
            "+#%暴擊率"
        );
        assert_eq!(state.settings.selected_game_profile, GameProfile::Poe2);
    }

    #[test]
    fn ocr_languages_remember_quick_and_structured_rules_independently() {
        let mut state = AppState::default();
        state
            .commit_form(valid_quick_form("#% increased Attack Speed"))
            .unwrap();

        state.apply(UiAction::SelectOcrLanguage(OcrLanguage::TraditionalChinese));
        assert_eq!(state.rule_mode(), RuleMode::Quick);
        assert!(
            state
                .current_profile()
                .selected_rules()
                .target_affix
                .is_empty()
        );
        state.commit_form(valid_quick_form("+#%暴擊率")).unwrap();
        state.apply(UiAction::SelectRuleMode(RuleMode::MultipleAffixes));
        state
            .commit_form(structured_form_without_numeric("+#%暴擊率"))
            .unwrap();

        state.apply(UiAction::SelectOcrLanguage(OcrLanguage::English));
        assert_eq!(state.rule_mode(), RuleMode::Quick);
        assert_eq!(
            state.current_profile().selected_rules().target_affix,
            "#% increased Attack Speed"
        );
        assert!(state.current_rule_set().is_none());

        state.apply(UiAction::SelectOcrLanguage(OcrLanguage::TraditionalChinese));
        assert_eq!(state.rule_mode(), RuleMode::MultipleAffixes);
        assert_eq!(
            state.current_profile().selected_rules().target_affix,
            "+#%暴擊率"
        );
        assert_eq!(
            state
                .current_condition()
                .map(|condition| condition.template.as_str()),
            Some("+#%暴擊率")
        );
    }

    #[test]
    fn form_commit_is_transactional_on_invalid_region() {
        let mut state = AppState::default();
        let before = state.settings.clone();
        let mut form = valid_quick_form("+#%暴擊率");
        form.region.height.clear();
        assert_eq!(
            state.commit_form(form),
            Err(EditorError::IncompleteCaptureRegion)
        );
        assert_eq!(state.settings, before);
    }

    #[test]
    fn exact_decimal_constraints_keep_boundary_text() {
        let mut state = AppState::default();
        state.apply(UiAction::SelectRuleMode(RuleMode::MultipleAffixes));
        {
            let condition = state.current_condition_mut().unwrap();
            condition.template = "+(3.11—3.8)%暴擊率".into();
        }
        state.add_numeric_constraint().unwrap();
        state
            .commit_form(structured_form(
                "+(3.11—3.8)%暴擊率",
                NumericConstraintMode::AtLeast,
                "3.1000000000000000000001",
                "",
            ))
            .unwrap();
        let constraint = state.current_numeric_constraint().unwrap();
        assert_eq!(
            constraint.minimum.unwrap().to_string(),
            "3.1000000000000000000001"
        );
        assert!(state.validate_for_save().is_ok());
    }

    #[test]
    fn valid_template_changes_resize_numeric_slots_by_position() {
        let mut state = AppState::from_settings(
            structured_settings("+#%暴擊率", vec![NumericConstraint::at_least(3)]),
            None,
        );
        assert_eq!(
            state.current_condition().unwrap().numeric_constraints,
            vec![NumericConstraint::at_least(3)]
        );

        state
            .commit_form(structured_form_without_numeric(
                "匕首攻擊附加 (7—8) 至 (9—10) 物理傷害",
            ))
            .unwrap();
        assert_eq!(
            state.current_condition().unwrap().numeric_constraints,
            vec![NumericConstraint::at_least(3), NumericConstraint::ignored(),]
        );

        state
            .commit_form(structured_form_without_numeric("+#%暴擊率"))
            .unwrap();
        assert_eq!(
            state.current_condition().unwrap().numeric_constraints,
            vec![NumericConstraint::at_least(3)]
        );
    }

    #[test]
    fn two_slot_damage_preserves_zero_one_and_two_constraint_semantics() {
        const TEMPLATE: &str = "匕首攻擊附加 (7—8) 至 (9—10) 物理傷害";

        let zero = AppState::from_settings(structured_settings(TEMPLATE, vec![]), None);
        assert_eq!(
            zero.current_condition().unwrap().numeric_constraints,
            vec![NumericConstraint::ignored(), NumericConstraint::ignored()]
        );
        assert!(evaluate_current_rules(
            &zero,
            "匕首攻擊附加 1 至 999 物理傷害"
        ));

        let one = AppState::from_settings(
            structured_settings(TEMPLATE, vec![NumericConstraint::at_least(8)]),
            None,
        );
        assert_eq!(
            one.current_condition().unwrap().numeric_constraints,
            vec![NumericConstraint::at_least(8), NumericConstraint::ignored(),]
        );
        assert!(!evaluate_current_rules(
            &one,
            "匕首攻擊附加 7 至 999 物理傷害"
        ));
        assert!(evaluate_current_rules(&one, "匕首攻擊附加 8 至 1 物理傷害"));

        let two = AppState::from_settings(
            structured_settings(
                TEMPLATE,
                vec![
                    NumericConstraint::at_least(8),
                    NumericConstraint::at_least(10),
                ],
            ),
            None,
        );
        assert!(!evaluate_current_rules(
            &two,
            "匕首攻擊附加 8 至 9 物理傷害"
        ));
        assert!(evaluate_current_rules(
            &two,
            "匕首攻擊附加 8(7-8) 至 10(9-10) 物理傷害"
        ));
    }

    #[test]
    fn temporary_empty_template_keeps_numeric_constraints_for_restore() {
        const TEMPLATE: &str = "匕首攻擊附加 (7—8) 至 (9—10) 物理傷害";
        let expected = vec![
            NumericConstraint::at_least(7),
            NumericConstraint::exactly(10),
        ];
        let mut state =
            AppState::from_settings(structured_settings(TEMPLATE, expected.clone()), None);

        state
            .commit_form(structured_form_without_numeric(""))
            .unwrap();
        assert!(state.current_condition().unwrap().template.is_empty());
        assert_eq!(
            state.current_condition().unwrap().numeric_constraints,
            expected
        );

        state
            .commit_form(structured_form_without_numeric(TEMPLATE))
            .unwrap();
        assert_eq!(
            state.current_condition().unwrap().numeric_constraints,
            expected
        );
    }

    #[test]
    fn range_rejects_reversed_boundaries_without_mutating() {
        let mut state = AppState::default();
        state.apply(UiAction::SelectRuleMode(RuleMode::MultipleAffixes));
        state.current_condition_mut().unwrap().template = "+#%暴擊率".into();
        state.add_numeric_constraint().unwrap();
        let before = state.current_numeric_constraint().cloned();
        assert_eq!(
            state.commit_form(structured_form(
                "+#%暴擊率",
                NumericConstraintMode::RangeInclusive,
                "4",
                "3",
            )),
            Err(EditorError::InvalidNumberRange)
        );
        assert_eq!(state.current_numeric_constraint().cloned(), before);
    }

    #[test]
    fn structured_editor_enforces_result_condition_and_slot_limits() {
        let mut state = AppState::default();
        state.apply(UiAction::SelectRuleMode(RuleMode::MultipleAffixes));
        assert_eq!(state.remove_result(), Err(EditorError::LastResultRequired));
        assert_eq!(
            state.remove_condition(),
            Err(EditorError::LastConditionRequired)
        );
        state.current_condition_mut().unwrap().template = "+#%暴擊率".into();
        state.add_numeric_constraint().unwrap();
        assert_eq!(
            state.add_numeric_constraint(),
            Err(EditorError::TooManyNumericConstraints)
        );
        for _ in 1..MAXIMUM_GROUPS {
            state.add_result().unwrap();
        }
        assert_eq!(state.add_result(), Err(EditorError::TooManyResults));
    }

    #[test]
    fn all_any_and_at_least_groups_survive_validation() {
        for mode in [
            ResultGroupMode::All,
            ResultGroupMode::Any,
            ResultGroupMode::AtLeast,
        ] {
            let mut state = AppState::default();
            state.apply(UiAction::SelectRuleMode(RuleMode::MultipleAffixes));
            let group = state.current_group_mut().unwrap();
            group.mode = mode;
            group.required_count = 1;
            group.conditions[0].template = "#% increased Attack Speed".into();
            assert!(state.validate_for_save().is_ok(), "mode {mode:?}");
        }
    }

    #[test]
    fn future_schema_is_clearly_read_only_and_never_queues_save() {
        let mut state = AppState::from_settings(AppSettings::default(), Some(77));
        assert!(!state.availability().configuration_enabled);
        assert_eq!(
            state.commit_form(valid_quick_form("+#%暴擊率")),
            Err(EditorError::ReadOnlySettings { detected: 77 })
        );
        assert_eq!(state.apply(UiAction::SaveSettings), None);
        assert_eq!(state.status(), MonitorStatus::ReadOnly);
    }

    #[test]
    fn save_and_monitor_commands_remain_typed_and_asynchronous() {
        let mut state = AppState::default();
        state.commit_form(valid_quick_form("+#%暴擊率")).unwrap();
        let save = state.apply(UiAction::SaveSettings).unwrap();
        assert!(matches!(save, BackgroundCommand::SaveSettings(_)));
        assert_eq!(state.pending_operation(), Some(Operation::SaveSettings));
        state.apply(UiAction::BackgroundCompleted(
            BackgroundCompletion::succeeded(Operation::SaveSettings),
        ));
        assert_eq!(state.status(), MonitorStatus::SettingsSaved);
        assert!(!state.is_dirty());

        let start = state.apply(UiAction::StartMonitoring).unwrap();
        let BackgroundCommand::StartMonitoring(snapshot) = start else {
            panic!("expected a start-monitoring command");
        };
        assert_eq!(snapshot.game, Game::Poe1);
        assert_eq!(
            snapshot.settings.profiles.poe1.rules_for("en").target_affix,
            "+#%暴擊率"
        );
        state.apply(UiAction::BackgroundCompleted(
            BackgroundCompletion::succeeded(Operation::StartMonitoring),
        ));
        assert!(state.is_monitoring());
        assert!(state.availability().stop_enabled);
    }

    #[test]
    fn automatic_save_preserves_a_selected_region_before_a_rule_is_complete() {
        let mut state = AppState::default();
        state
            .set_capture_region(ScreenRegion::new(12, 34, 500, 700))
            .unwrap();
        let command = state
            .begin_automatic_save()
            .expect("region selection should queue an asynchronous save");
        let BackgroundCommand::SaveSettings(settings) = command else {
            panic!("expected settings save");
        };
        assert_eq!(
            settings.profiles.poe1.capture_region,
            Some(ScreenRegion::new(12, 34, 500, 700))
        );
        assert_eq!(state.pending_operation(), Some(Operation::SaveSettings));
    }

    #[test]
    fn stale_background_completion_cannot_change_state() {
        let mut state = AppState::default();
        state.commit_form(valid_quick_form("+#%暴擊率")).unwrap();
        state.apply(UiAction::StartMonitoring);
        state.apply(UiAction::BackgroundCompleted(
            BackgroundCompletion::succeeded(Operation::TestScreenshot),
        ));
        assert_eq!(state.status(), MonitorStatus::Starting);
        assert!(!state.is_monitoring());
    }
}
