//! Settings compatibility boundary for the Rust POE Alarm preview.
//!
//! The public model represents normalized schema 4 settings. Loading accepts the released
//! profile-aware schemas, the old flat POE1 layout, and the retired `MonitoringPolicy` field.
//! A settings file from a future schema is never normalized or overwritten.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use poe_alarm_core::{
    AcceptableResultGroup, AffixCondition, Decimal, NumericConstraint, NumericConstraintMode,
    ResultGroupMode, RuleSetDefinition,
};
use serde::de::{Error as DeError, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

pub const CURRENT_SCHEMA_VERSION: u32 = 4;
const RULE_SET_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_UI_LANGUAGE: &str = "en";
pub const DEFAULT_OCR_LANGUAGE: &str = "en";
pub const TRADITIONAL_CHINESE_OCR_LANGUAGE: &str = "zh-TW";
pub const DEFAULT_START_MONITORING_HOTKEY: &str = "Ctrl+Shift+F10";

const PREVIEW_SETTINGS_DIRECTORY: &str = "PoeAlarm-RustPreview";
const RELEASE_SETTINGS_DIRECTORY: &str = "PoeAlarm";
const SETTINGS_FILE_NAME: &str = "settings.json";

/// Returns the Rust preview path without consulting process-global environment variables.
pub fn preview_settings_path_from(local_app_data: impl AsRef<Path>) -> PathBuf {
    local_app_data
        .as_ref()
        .join(PREVIEW_SETTINGS_DIRECTORY)
        .join(SETTINGS_FILE_NAME)
}

/// Returns the released .NET 1.0 path for read-only import tooling.
pub fn release_settings_path_from(local_app_data: impl AsRef<Path>) -> PathBuf {
    local_app_data
        .as_ref()
        .join(RELEASE_SETTINGS_DIRECTORY)
        .join(SETTINGS_FILE_NAME)
}

/// Resolves the isolated Rust preview settings path from `%LOCALAPPDATA%`.
pub fn preview_settings_path() -> io::Result<PathBuf> {
    local_app_data_path().map(preview_settings_path_from)
}

/// Resolves the released .NET settings path. Callers must treat this as an import source until
/// the migration gate explicitly switches ownership to Rust.
pub fn release_settings_path() -> io::Result<PathBuf> {
    local_app_data_path().map(release_settings_path_from)
}

fn local_app_data_path() -> io::Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "LOCALAPPDATA is unavailable; the settings path cannot be resolved",
            )
        })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub enum GameProfile {
    #[default]
    Poe1,
    Poe2,
}

impl GameProfile {
    fn parse(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str).map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("Poe2") => Self::Poe2,
            _ => Self::Poe1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub enum RuleEditorMode {
    #[default]
    Quick,
    Structured,
}

impl RuleEditorMode {
    fn parse(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str).map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("Structured") => Self::Structured,
            _ => Self::Quick,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct HudPlacement {
    pub monitor_device_name: Option<String>,
    pub relative_x: Option<f64>,
    pub relative_y: Option<f64>,
}

impl HudPlacement {
    pub fn is_automatic(&self) -> bool {
        self.relative_x.is_none() && self.relative_y.is_none()
    }

    pub fn is_manual(&self) -> bool {
        self.relative_x.is_some() && self.relative_y.is_some() && self.is_valid()
    }

    pub fn is_valid(&self) -> bool {
        if self.is_automatic() {
            return true;
        }

        matches!(
            (self.relative_x, self.relative_y),
            (Some(x), Some(y))
                if x.is_finite()
                    && y.is_finite()
                    && (0.0..=1.0).contains(&x)
                    && (0.0..=1.0).contains(&y)
        )
    }

    pub fn sanitize(mut self) -> Self {
        if !self.is_valid() {
            return Self::default();
        }

        self.monitor_device_name = self
            .monitor_device_name
            .take()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty());
        self
    }

    fn parse(value: Option<&Value>) -> Self {
        let Some(object) = value.and_then(Value::as_object) else {
            return Self::default();
        };

        Self {
            monitor_device_name: json_string(member_ci(object, "MonitorDeviceName")),
            relative_x: json_f64(member_ci(object, "RelativeX")),
            relative_y: json_f64(member_ci(object, "RelativeY")),
        }
        .sanitize()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RuleProfileSettings {
    pub target_affix: String,
    pub rule_editor_mode: RuleEditorMode,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_rule_set"
    )]
    pub structured_rule_set: Option<RuleSetDefinition>,
}

impl Default for RuleProfileSettings {
    fn default() -> Self {
        Self {
            target_affix: String::new(),
            rule_editor_mode: RuleEditorMode::Quick,
            structured_rule_set: None,
        }
    }
}

impl RuleProfileSettings {
    pub fn normalize(mut self) -> Self {
        self.target_affix = self.target_affix.trim().to_owned();
        self
    }

    fn parse(value: Option<&Value>) -> Self {
        let Some(object) = value.and_then(Value::as_object) else {
            return Self::default();
        };

        Self {
            target_affix: json_string(member_ci(object, "TargetAffix")).unwrap_or_default(),
            rule_editor_mode: RuleEditorMode::parse(member_ci(object, "RuleEditorMode")),
            structured_rule_set: member_ci(object, "StructuredRuleSet").and_then(parse_rule_set),
        }
        .normalize()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RuleProfileSettingsSet {
    pub english: RuleProfileSettings,
    pub traditional_chinese: RuleProfileSettings,
}

impl RuleProfileSettingsSet {
    /// Returns the rules for a persisted OCR language code. Unsupported values follow the
    /// settings normalization contract and select English.
    pub fn get(&self, ocr_language: &str) -> &RuleProfileSettings {
        if is_traditional_chinese_ocr_language(ocr_language) {
            &self.traditional_chinese
        } else {
            &self.english
        }
    }

    /// Returns mutable rules for a persisted OCR language code. Unsupported values follow the
    /// settings normalization contract and select English.
    pub fn get_mut(&mut self, ocr_language: &str) -> &mut RuleProfileSettings {
        if is_traditional_chinese_ocr_language(ocr_language) {
            &mut self.traditional_chinese
        } else {
            &mut self.english
        }
    }

    pub fn normalize(mut self) -> Self {
        self.english = self.english.normalize();
        self.traditional_chinese = self.traditional_chinese.normalize();
        self
    }

    fn parse(value: &Value) -> Self {
        let Some(object) = value.as_object() else {
            return Self::default();
        };

        Self {
            english: RuleProfileSettings::parse(member_ci(object, "English")),
            traditional_chinese: RuleProfileSettings::parse(member_ci(
                object,
                "TraditionalChinese",
            )),
        }
        .normalize()
    }

    fn from_legacy(ocr_language: &str, rules: RuleProfileSettings) -> Self {
        let mut profiles = Self::default();
        *profiles.get_mut(ocr_language) = rules.normalize();
        profiles
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GameProfileSettings {
    pub ocr_language: String,
    pub rule_profiles: RuleProfileSettingsSet,
}

impl Default for GameProfileSettings {
    fn default() -> Self {
        Self {
            ocr_language: DEFAULT_OCR_LANGUAGE.to_owned(),
            rule_profiles: RuleProfileSettingsSet::default(),
        }
    }
}

impl GameProfileSettings {
    /// Returns the rule profile selected by this game's current OCR language.
    pub fn selected_rules(&self) -> &RuleProfileSettings {
        self.rule_profiles.get(&self.ocr_language)
    }

    /// Returns mutable rules selected by this game's current OCR language.
    pub fn selected_rules_mut(&mut self) -> &mut RuleProfileSettings {
        self.rule_profiles.get_mut(&self.ocr_language)
    }

    pub fn rules_for(&self, ocr_language: &str) -> &RuleProfileSettings {
        self.rule_profiles.get(ocr_language)
    }

    pub fn rules_for_mut(&mut self, ocr_language: &str) -> &mut RuleProfileSettings {
        self.rule_profiles.get_mut(ocr_language)
    }

    pub fn normalize(mut self) -> Self {
        self.ocr_language = normalize_ocr_language(&self.ocr_language).to_owned();
        self.rule_profiles = self.rule_profiles.normalize();
        self
    }

    fn parse(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        let Some(object) = value.as_object() else {
            return Self::default();
        };

        let ocr_language = normalize_ocr_language(
            json_string(member_ci(object, "OcrLanguage"))
                .as_deref()
                .unwrap_or(DEFAULT_OCR_LANGUAGE),
        )
        .to_owned();
        let rule_profiles = match member_ci(object, "RuleProfiles") {
            Some(rule_profiles) if rule_profiles.is_object() => {
                RuleProfileSettingsSet::parse(rule_profiles)
            }
            _ => RuleProfileSettingsSet::from_legacy(
                &ocr_language,
                RuleProfileSettings::parse(Some(value)),
            ),
        };

        Self {
            ocr_language,
            rule_profiles,
        }
        .normalize()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GameProfileSettingsSet {
    pub poe1: GameProfileSettings,
    pub poe2: GameProfileSettings,
}

impl GameProfileSettingsSet {
    pub fn get(&self, profile: GameProfile) -> &GameProfileSettings {
        match profile {
            GameProfile::Poe1 => &self.poe1,
            GameProfile::Poe2 => &self.poe2,
        }
    }

    pub fn get_mut(&mut self, profile: GameProfile) -> &mut GameProfileSettings {
        match profile {
            GameProfile::Poe1 => &mut self.poe1,
            GameProfile::Poe2 => &mut self.poe2,
        }
    }

    pub fn normalize(mut self) -> Self {
        self.poe1 = self.poe1.normalize();
        self.poe2 = self.poe2.normalize();
        self
    }

    fn parse(value: &Value) -> Self {
        let Some(object) = value.as_object() else {
            return Self::default();
        };

        Self {
            poe1: GameProfileSettings::parse(member_ci(object, "Poe1")),
            poe2: GameProfileSettings::parse(member_ci(object, "Poe2")),
        }
        .normalize()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AppSettings {
    pub schema_version: u32,
    pub selected_game_profile: GameProfile,
    pub profiles: GameProfileSettingsSet,
    pub ui_language: String,
    pub keep_hud_visible: bool,
    pub hud_placement: HudPlacement,
    pub allow_overlay_capture: bool,
    pub custom_alert_sound_path: Option<String>,
    pub start_monitoring_hot_key: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            selected_game_profile: GameProfile::Poe1,
            profiles: GameProfileSettingsSet::default(),
            ui_language: DEFAULT_UI_LANGUAGE.to_owned(),
            keep_hud_visible: true,
            hud_placement: HudPlacement::default(),
            allow_overlay_capture: true,
            custom_alert_sound_path: None,
            start_monitoring_hot_key: DEFAULT_START_MONITORING_HOTKEY.to_owned(),
        }
    }
}

impl AppSettings {
    pub fn selected_profile(&self) -> &GameProfileSettings {
        self.profiles.get(self.selected_game_profile)
    }

    pub fn selected_profile_mut(&mut self) -> &mut GameProfileSettings {
        self.profiles.get_mut(self.selected_game_profile)
    }

    pub fn selected_rules(&self) -> &RuleProfileSettings {
        self.selected_profile().selected_rules()
    }

    pub fn selected_rules_mut(&mut self) -> &mut RuleProfileSettings {
        self.selected_profile_mut().selected_rules_mut()
    }

    pub fn normalize(mut self) -> Self {
        self.schema_version = CURRENT_SCHEMA_VERSION;
        self.profiles = self.profiles.normalize();
        self.ui_language = normalize_ui_language(&self.ui_language).to_owned();
        self.hud_placement = self.hud_placement.sanitize();
        self.start_monitoring_hot_key =
            normalize_start_monitoring_hot_key(&self.start_monitoring_hot_key).to_owned();
        self
    }

    fn parse(value: &Value) -> Result<Self, &'static str> {
        let object = value
            .as_object()
            .ok_or("the settings document must contain a JSON object")?;

        let profiles = match member_ci(object, "Profiles") {
            Some(Value::Object(_)) => GameProfileSettingsSet::parse(
                member_ci(object, "Profiles").expect("the profile value was just matched"),
            ),
            Some(Value::Null) | None => Self::migrate_flat_profile(object),
            Some(_) => GameProfileSettingsSet::default(),
        };

        Ok(Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            selected_game_profile: GameProfile::parse(member_ci(object, "SelectedGameProfile")),
            profiles,
            ui_language: json_string(member_ci(object, "UiLanguage"))
                .unwrap_or_else(|| DEFAULT_UI_LANGUAGE.to_owned()),
            keep_hud_visible: json_bool(member_ci(object, "KeepHudVisible")).unwrap_or(true),
            hud_placement: HudPlacement::parse(member_ci(object, "HudPlacement")),
            allow_overlay_capture: json_bool(member_ci(object, "AllowOverlayCapture"))
                .unwrap_or(true),
            custom_alert_sound_path: json_string(member_ci(object, "CustomAlertSoundPath")),
            start_monitoring_hot_key: json_string(member_ci(object, "StartMonitoringHotKey"))
                .unwrap_or_else(|| DEFAULT_START_MONITORING_HOTKEY.to_owned()),
        }
        .normalize())
    }

    fn migrate_flat_profile(object: &Map<String, Value>) -> GameProfileSettingsSet {
        let ocr_language = normalize_ocr_language(
            json_string(member_ci(object, "OcrLanguage"))
                .as_deref()
                .unwrap_or(DEFAULT_OCR_LANGUAGE),
        )
        .to_owned();
        let legacy_rules = RuleProfileSettings {
            target_affix: json_string(member_ci(object, "TargetAffix")).unwrap_or_default(),
            ..RuleProfileSettings::default()
        };
        GameProfileSettingsSet {
            poe1: GameProfileSettings {
                rule_profiles: RuleProfileSettingsSet::from_legacy(&ocr_language, legacy_rules),
                ocr_language,
            }
            .normalize(),
            poe2: GameProfileSettings::default(),
        }
    }
}

impl<'de> Deserialize<'de> for AppSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

pub fn normalize_ui_language(language: &str) -> &'static str {
    let lower = language.trim().to_ascii_lowercase();
    if lower.starts_with("zh") {
        "zh-CN"
    } else if lower.starts_with("en") {
        "en"
    } else {
        DEFAULT_UI_LANGUAGE
    }
}

pub fn normalize_ocr_language(language: &str) -> &'static str {
    if is_traditional_chinese_ocr_language(language) {
        TRADITIONAL_CHINESE_OCR_LANGUAGE
    } else {
        DEFAULT_OCR_LANGUAGE
    }
}

fn is_traditional_chinese_ocr_language(language: &str) -> bool {
    language
        .trim()
        .eq_ignore_ascii_case(TRADITIONAL_CHINESE_OCR_LANGUAGE)
}

pub fn normalize_start_monitoring_hot_key(hotkey: &str) -> &'static str {
    let hotkey = hotkey.trim();
    if hotkey.eq_ignore_ascii_case("Ctrl+Alt+F10") {
        "Ctrl+Alt+F10"
    } else if hotkey.eq_ignore_ascii_case("Alt+F10") {
        "Alt+F10"
    } else {
        DEFAULT_START_MONITORING_HOTKEY
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SettingsStoreCompatibility {
    #[default]
    Compatible,
    FutureSchemaReadOnly,
}

/// File-backed schema 4 store. `load` follows the released recovery contract and returns safe
/// defaults for missing, malformed, or unreadable files. `save` remains fallible so callers can
/// surface persistence failures to the user.
#[derive(Debug)]
pub struct SettingsStore {
    settings_path: PathBuf,
    compatibility: SettingsStoreCompatibility,
    detected_schema_version: Option<u32>,
}

impl SettingsStore {
    pub fn new(settings_path: impl Into<PathBuf>) -> Self {
        Self {
            settings_path: settings_path.into(),
            compatibility: SettingsStoreCompatibility::Compatible,
            detected_schema_version: None,
        }
    }

    pub fn preview_default() -> io::Result<Self> {
        preview_settings_path().map(Self::new)
    }

    /// 正式版入口:`%LOCALAPPDATA%/PoeAlarm/settings.json`。
    /// Rust 预览目录仍有设置时做一次性迁移(预览线是更活跃的配置,覆盖旧 .NET
    /// 文件并把 .NET 设置留档为 `settings.json.dotnet-1.0.bak`);迁移完成后
    /// 预览文件改名为 `settings.json.migrated`,保证不会重复迁移。
    pub fn release_default() -> io::Result<Self> {
        let local_app_data = local_app_data_path()?;
        Self::release_default_from(local_app_data)
    }

    /// 与 [`Self::release_default`] 相同,但注入 `%LOCALAPPDATA%` 以便测试。
    pub fn release_default_from(local_app_data: impl AsRef<Path>) -> io::Result<Self> {
        let release = release_settings_path_from(&local_app_data);
        let preview = preview_settings_path_from(&local_app_data);
        if preview.is_file() {
            if let Some(parent) = release.parent() {
                fs::create_dir_all(parent)?;
            }
            let dotnet_backup = release.with_extension("json.dotnet-1.0.bak");
            if release.is_file() && !dotnet_backup.exists() {
                let _ = fs::copy(&release, &dotnet_backup);
            }
            fs::copy(&preview, &release)?;
            let migrated = preview.with_extension("json.migrated");
            let _ = fs::remove_file(&migrated);
            fs::rename(&preview, &migrated).or_else(|_| fs::remove_file(&preview))?;
        }
        Ok(Self::new(release))
    }

    pub fn path(&self) -> &Path {
        &self.settings_path
    }

    pub fn compatibility(&self) -> SettingsStoreCompatibility {
        self.compatibility
    }

    pub fn detected_schema_version(&self) -> Option<u32> {
        self.detected_schema_version
    }

    pub fn is_read_only(&self) -> bool {
        self.compatibility == SettingsStoreCompatibility::FutureSchemaReadOnly
    }

    pub fn load(&mut self) -> AppSettings {
        let bytes = match fs::read(&self.settings_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.mark_compatible();
                return AppSettings::default();
            }
            Err(_) => {
                if !self.is_read_only() {
                    self.mark_compatible();
                }
                return AppSettings::default();
            }
        };

        match scan_schema_version(&bytes) {
            Ok(Some(version)) if version > CURRENT_SCHEMA_VERSION => {
                self.mark_future_schema(version);
                return AppSettings::default();
            }
            Ok(_) => self.mark_compatible(),
            Err(_) => {
                if !self.is_read_only() {
                    self.mark_compatible();
                }
                return AppSettings::default();
            }
        }

        serde_json::from_slice::<AppSettings>(&bytes).unwrap_or_default()
    }

    pub fn save(&mut self, settings: &AppSettings) -> Result<(), SettingsError> {
        if self.is_read_only() {
            return Err(self.schema_too_new_error());
        }

        self.guard_future_schema_on_disk()?;

        let normalized = settings.clone().normalize();
        let serialized = serde_json::to_vec_pretty(&normalized).map_err(SettingsError::Json)?;
        let directory = self.settings_path.parent().ok_or_else(|| {
            SettingsError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the settings path has no parent directory",
            ))
        })?;
        fs::create_dir_all(directory).map_err(SettingsError::Io)?;

        let (temporary_path, mut temporary_file) =
            create_sibling_temporary_file(&self.settings_path).map_err(SettingsError::Io)?;

        let result = (|| {
            temporary_file
                .write_all(&serialized)
                .map_err(SettingsError::Io)?;
            temporary_file.sync_all().map_err(SettingsError::Io)?;
            drop(temporary_file);

            // A newer process can claim the file while serialization is in progress. Recheck
            // immediately before the atomic replacement and keep its file authoritative.
            self.guard_future_schema_on_disk()?;
            atomic_replace(&temporary_path, &self.settings_path).map_err(SettingsError::Io)
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }

        result
    }

    fn guard_future_schema_on_disk(&mut self) -> Result<(), SettingsError> {
        let bytes = match fs::read(&self.settings_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(SettingsError::Io(error)),
        };

        // Malformed files retain the 1.0 recovery behavior. This guard applies only to valid JSON
        // that explicitly declares a future schema.
        let Ok(Some(version)) = scan_schema_version(&bytes) else {
            return Ok(());
        };
        if version <= CURRENT_SCHEMA_VERSION {
            return Ok(());
        }

        self.mark_future_schema(version);
        Err(self.schema_too_new_error())
    }

    fn mark_compatible(&mut self) {
        self.compatibility = SettingsStoreCompatibility::Compatible;
        self.detected_schema_version = None;
    }

    fn mark_future_schema(&mut self, version: u32) {
        self.compatibility = SettingsStoreCompatibility::FutureSchemaReadOnly;
        self.detected_schema_version = Some(version);
    }

    fn schema_too_new_error(&self) -> SettingsError {
        SettingsError::SchemaTooNew {
            detected: self
                .detected_schema_version
                .unwrap_or(CURRENT_SCHEMA_VERSION + 1),
            supported: CURRENT_SCHEMA_VERSION,
        }
    }
}

#[derive(Debug)]
pub enum SettingsError {
    Io(io::Error),
    Json(serde_json::Error),
    SchemaTooNew { detected: u32, supported: u32 },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "settings I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "settings serialization failed: {error}"),
            Self::SchemaTooNew {
                detected,
                supported,
            } => write!(
                formatter,
                "settings schema {detected} is newer than supported schema {supported}; the file was left unchanged"
            ),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::SchemaTooNew { .. } => None,
        }
    }
}

fn member_ci<'a>(object: &'a Map<String, Value>, expected: &str) -> Option<&'a Value> {
    object.get(expected).or_else(|| {
        object
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(expected))
            .map(|(_, value)| value)
    })
}

fn json_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(ToOwned::to_owned)
}

fn json_bool(value: Option<&Value>) -> Option<bool> {
    value.and_then(Value::as_bool)
}

fn json_usize(value: Option<&Value>) -> Option<usize> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn json_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn json_f64(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn json_decimal(value: Option<&Value>) -> Option<Decimal> {
    match value? {
        Value::Number(number) => number.as_str().parse().ok(),
        _ => None,
    }
}

fn parse_rule_set(value: &Value) -> Option<RuleSetDefinition> {
    let object = value.as_object()?;
    let groups = member_ci(object, "Groups")
        .and_then(Value::as_array)
        .map(|groups| groups.iter().filter_map(parse_result_group).collect())
        .unwrap_or_default();

    Some(RuleSetDefinition {
        schema_version: json_u32(member_ci(object, "SchemaVersion"))
            .unwrap_or(RULE_SET_SCHEMA_VERSION),
        name: json_string(member_ci(object, "Name")).unwrap_or_default(),
        groups,
    })
}

fn parse_result_group(value: &Value) -> Option<AcceptableResultGroup> {
    let object = value.as_object()?;
    let conditions = member_ci(object, "Conditions")
        .and_then(Value::as_array)
        .map(|conditions| {
            conditions
                .iter()
                .filter_map(parse_affix_condition)
                .collect()
        })
        .unwrap_or_default();

    Some(AcceptableResultGroup {
        name: json_string(member_ci(object, "Name")).unwrap_or_default(),
        mode: parse_result_group_mode(member_ci(object, "Mode")),
        required_count: json_usize(member_ci(object, "RequiredCount")).unwrap_or(1),
        conditions,
    })
}

fn parse_result_group_mode(value: Option<&Value>) -> ResultGroupMode {
    match value.and_then(Value::as_str).map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("All") => ResultGroupMode::All,
        Some(value) if value.eq_ignore_ascii_case("AtLeast") => ResultGroupMode::AtLeast,
        _ => ResultGroupMode::Any,
    }
}

fn parse_affix_condition(value: &Value) -> Option<AffixCondition> {
    let object = value.as_object()?;
    let numeric_constraints = member_ci(object, "NumericConstraints")
        .and_then(Value::as_array)
        .map(|constraints| {
            constraints
                .iter()
                .filter_map(parse_numeric_constraint)
                .collect()
        })
        .unwrap_or_default();

    Some(AffixCondition {
        name: json_string(member_ci(object, "Name")).unwrap_or_default(),
        template: json_string(member_ci(object, "Template")).unwrap_or_default(),
        numeric_constraints,
    })
}

fn parse_numeric_constraint(value: &Value) -> Option<NumericConstraint> {
    let object = value.as_object()?;
    Some(NumericConstraint {
        mode: parse_numeric_constraint_mode(member_ci(object, "Mode")),
        minimum: json_decimal(member_ci(object, "Minimum")),
        maximum: json_decimal(member_ci(object, "Maximum")),
        expected: json_decimal(member_ci(object, "Expected")),
    })
}

fn parse_numeric_constraint_mode(value: Option<&Value>) -> NumericConstraintMode {
    match value.and_then(Value::as_str).map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("RangeInclusive") => {
            NumericConstraintMode::RangeInclusive
        }
        Some(value) if value.eq_ignore_ascii_case("AtLeast") => NumericConstraintMode::AtLeast,
        Some(value) if value.eq_ignore_ascii_case("AtMost") => NumericConstraintMode::AtMost,
        Some(value) if value.eq_ignore_ascii_case("Exactly") => NumericConstraintMode::Exactly,
        _ => NumericConstraintMode::Ignore,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct RuleSetWire<'a> {
    schema_version: u32,
    name: &'a str,
    groups: Vec<ResultGroupWire<'a>>,
}

impl<'a> From<&'a RuleSetDefinition> for RuleSetWire<'a> {
    fn from(rule_set: &'a RuleSetDefinition) -> Self {
        Self {
            schema_version: rule_set.schema_version,
            name: &rule_set.name,
            groups: rule_set.groups.iter().map(ResultGroupWire::from).collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ResultGroupWire<'a> {
    name: &'a str,
    mode: ResultGroupMode,
    required_count: usize,
    conditions: Vec<AffixConditionWire<'a>>,
}

impl<'a> From<&'a AcceptableResultGroup> for ResultGroupWire<'a> {
    fn from(group: &'a AcceptableResultGroup) -> Self {
        Self {
            name: &group.name,
            mode: group.mode,
            required_count: group.required_count,
            conditions: group
                .conditions
                .iter()
                .map(AffixConditionWire::from)
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct AffixConditionWire<'a> {
    name: &'a str,
    template: &'a str,
    numeric_constraints: Vec<NumericConstraintWire>,
}

impl<'a> From<&'a AffixCondition> for AffixConditionWire<'a> {
    fn from(condition: &'a AffixCondition) -> Self {
        Self {
            name: &condition.name,
            template: &condition.template,
            numeric_constraints: condition
                .numeric_constraints
                .iter()
                .map(NumericConstraintWire::from)
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct NumericConstraintWire {
    mode: NumericConstraintMode,
    minimum: Option<Decimal>,
    maximum: Option<Decimal>,
    expected: Option<Decimal>,
}

impl From<&NumericConstraint> for NumericConstraintWire {
    fn from(constraint: &NumericConstraint) -> Self {
        Self {
            mode: constraint.mode,
            minimum: constraint.minimum,
            maximum: constraint.maximum,
            expected: constraint.expected,
        }
    }
}

fn serialize_optional_rule_set<S>(
    rule_set: &Option<RuleSetDefinition>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match rule_set {
        Some(rule_set) => RuleSetWire::from(rule_set).serialize(serializer),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug)]
struct SchemaScan(Option<u32>);

impl<'de> Deserialize<'de> for SchemaScan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SchemaVisitor;

        impl<'de> Visitor<'de> for SchemaVisitor {
            type Value = SchemaScan;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a settings JSON object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut highest = None;
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("SchemaVersion") {
                        let value = map.next_value::<Value>()?;
                        if let Some(candidate) = value
                            .as_u64()
                            .and_then(|candidate| u32::try_from(candidate).ok())
                        {
                            highest = Some(
                                highest.map_or(candidate, |current: u32| current.max(candidate)),
                            );
                        }
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }

                Ok(SchemaScan(highest))
            }
        }

        deserializer.deserialize_map(SchemaVisitor)
    }
}

fn scan_schema_version(bytes: &[u8]) -> Result<Option<u32>, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let scan = SchemaScan::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(scan.0)
}

static TEMPORARY_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn create_sibling_temporary_file(settings_path: &Path) -> io::Result<(PathBuf, File)> {
    let directory = settings_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the settings path has no parent directory",
        )
    })?;
    let file_name = settings_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(SETTINGS_FILE_NAME);

    for _ in 0..32 {
        let counter = TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary_path = directory.join(format!(
            ".{file_name}.{}.{}.{}.tmp",
            std::process::id(),
            nanos,
            counter
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique settings temporary file",
    ))
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    // SAFETY: Both pointers reference NUL-terminated UTF-16 buffers that remain alive for the
    // duration of the synchronous Win32 call. The files are on the same directory/volume.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let counter = TEST_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "poe-alarm-settings-{}-{}-{counter}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("create isolated settings test directory");
            Self(path)
        }

        fn settings_path(&self) -> PathBuf {
            self.0.join(SETTINGS_FILE_NAME)
        }

        fn temporary_files(&self) -> Vec<PathBuf> {
            fs::read_dir(&self.0)
                .expect("enumerate test directory")
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "tmp"))
                .collect()
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn release_default_migrates_preview_settings_once_and_backs_up_dotnet_file() {
        let directory = TestDirectory::new();
        let local_app_data = &directory.0;
        let preview = preview_settings_path_from(local_app_data);
        let release = release_settings_path_from(local_app_data);
        fs::create_dir_all(preview.parent().unwrap()).unwrap();
        fs::create_dir_all(release.parent().unwrap()).unwrap();
        fs::write(
            &preview,
            br#"{ "SchemaVersion": 4, "UiLanguage": "zh-CN" }"#,
        )
        .unwrap();
        fs::write(&release, br#"{ "SchemaVersion": 3 }"#).unwrap();

        let store = SettingsStore::release_default_from(local_app_data).unwrap();
        assert_eq!(store.path(), release.as_path());
        // 预览设置覆盖正式路径,旧 .NET 文件留档,预览文件改名防止重复迁移。
        assert!(
            fs::read_to_string(&release)
                .unwrap()
                .contains("\"SchemaVersion\": 4")
        );
        assert!(
            fs::read_to_string(release.with_extension("json.dotnet-1.0.bak"))
                .unwrap()
                .contains("\"SchemaVersion\": 3")
        );
        assert!(!preview.exists());
        assert!(preview.with_extension("json.migrated").is_file());

        // 第二次启动:没有预览文件,不再迁移,正式文件保持不变。
        fs::write(&release, br#"{ "SchemaVersion": 4, "UiLanguage": "en" }"#).unwrap();
        let _ = SettingsStore::release_default_from(local_app_data).unwrap();
        assert!(
            fs::read_to_string(&release)
                .unwrap()
                .contains("\"UiLanguage\": \"en\"")
        );
    }

    #[test]
    fn preview_and_release_paths_are_isolated() {
        let root = Path::new(r"C:\Users\tester\AppData\Local");
        assert_eq!(
            preview_settings_path_from(root),
            root.join("PoeAlarm-RustPreview").join("settings.json")
        );
        assert_eq!(
            release_settings_path_from(root),
            root.join("PoeAlarm").join("settings.json")
        );
        assert_ne!(
            preview_settings_path_from(root),
            release_settings_path_from(root)
        );
    }

    #[test]
    fn selected_rule_accessors_isolate_ocr_languages() {
        let mut profile = GameProfileSettings::default();
        profile.rules_for_mut("en").target_affix = "english target".to_owned();
        profile.rules_for_mut("zh-TW").target_affix = "繁中目標".to_owned();

        assert_eq!(profile.selected_rules().target_affix, "english target");
        profile.selected_rules_mut().rule_editor_mode = RuleEditorMode::Structured;
        assert_eq!(
            profile.rules_for("en").rule_editor_mode,
            RuleEditorMode::Structured
        );
        assert_eq!(
            profile.rules_for("zh-TW").rule_editor_mode,
            RuleEditorMode::Quick
        );

        profile.ocr_language = "ZH-tw".to_owned();
        assert_eq!(profile.selected_rules().target_affix, "繁中目標");
        profile.selected_rules_mut().target_affix = "繁中更新".to_owned();
        assert_eq!(profile.rules_for("zh-TW").target_affix, "繁中更新");
        assert_eq!(profile.rules_for("en").target_affix, "english target");
        assert_eq!(profile.rules_for("unsupported"), profile.rules_for("en"));
    }

    #[test]
    fn a_one_point_zero_file_still_loads_after_the_capture_region_was_dropped() {
        // Everyone upgrading from 1.0 has a CaptureRegion key on disk. It is
        // read by nothing now, and an unknown key must stay unremarkable —
        // neither an error nor a reason to discard the rest of the profile.
        let directory = TestDirectory::new();
        let path = directory.settings_path();
        fs::write(
            &path,
            r#"{
                "SchemaVersion": 4,
                "SelectedGameProfile": "Poe1",
                "UiLanguage": "zh-CN",
                "Profiles": {
                    "Poe1": {
                        "CaptureRegion": { "X": 12, "Y": 34, "Width": 560, "Height": 420 },
                        "OcrLanguage": "zh-TW",
                        "RuleProfiles": {
                            "TraditionalChinese": { "TargetAffix": "+#% 暴擊率" }
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let mut store = SettingsStore::new(&path);
        let settings = store.load();
        assert_eq!(settings.ui_language, "zh-CN");
        assert_eq!(settings.profiles.poe1.ocr_language, "zh-TW");
        assert_eq!(
            settings.profiles.poe1.selected_rules().target_affix,
            "+#% 暴擊率"
        );

        // Saving is what actually drops the key from disk, and it must not
        // drop anything else with it.
        store.save(&settings).unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let poe1 = saved["Profiles"]["Poe1"].as_object().unwrap();
        assert!(poe1.get("CaptureRegion").is_none());
        assert_eq!(poe1["OcrLanguage"], "zh-TW");
    }

    #[test]
    fn schema_one_loads_both_profiles_and_normalizes_defaults() {
        let directory = TestDirectory::new();
        let path = directory.settings_path();
        fs::write(
            &path,
            r#"{
                "schemaVersion": 1,
                "selectedgameprofile": "poe2",
                "profiles": {
                    "poe1": {
                        "targetaffix": "  poe1 target  ",
                        "captureregion": { "x": 12, "y": 34, "width": 560, "height": 420 },
                        "ocrlanguage": "ZH-tw"
                    },
                    "poe2": {
                        "targetaffix": "poe2 target",
                        "ocrlanguage": "not-supported"
                    }
                },
                "uilanguage": "en-US",
                "startmonitoringhotkey": "ctrl+alt+f10"
            }"#,
        )
        .unwrap();

        let mut store = SettingsStore::new(&path);
        let settings = store.load();
        assert_eq!(settings.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(settings.selected_game_profile, GameProfile::Poe2);
        assert_eq!(
            settings.profiles.poe1.selected_rules().target_affix,
            "poe1 target"
        );
        assert_eq!(settings.profiles.poe1.ocr_language, "zh-TW");
        assert_eq!(
            settings.profiles.poe2.selected_rules().target_affix,
            "poe2 target"
        );
        assert_eq!(settings.profiles.poe2.ocr_language, "en");
        assert_eq!(
            settings.profiles.poe1.rules_for("en"),
            &RuleProfileSettings::default()
        );
        assert_eq!(
            settings.profiles.poe2.rules_for("zh-TW"),
            &RuleProfileSettings::default()
        );
        assert_eq!(settings.ui_language, "en");
        assert_eq!(settings.start_monitoring_hot_key, "Ctrl+Alt+F10");
        assert_eq!(
            store.compatibility(),
            SettingsStoreCompatibility::Compatible
        );
    }

    #[test]
    fn schema_two_preserves_structured_rules_in_core_types() {
        let directory = TestDirectory::new();
        let path = directory.settings_path();
        fs::write(
            &path,
            r#"{
              "SchemaVersion": 2,
              "SelectedGameProfile": "Poe2",
              "Profiles": {
                "Poe1": {
                  "TargetAffix": "quick backup",
                  "RuleEditorMode": "Quick",
                  "StructuredRuleSet": { "SchemaVersion": 1, "Name": "backup", "Groups": [] }
                },
                "Poe2": {
                  "TargetAffix": "critical target",
                  "RuleEditorMode": "Structured",
                  "StructuredRuleSet": {
                    "SchemaVersion": 1,
                    "Name": "poe2 rules",
                    "Groups": [{
                      "Name": "critical result",
                      "Mode": "AtLeast",
                      "RequiredCount": 2,
                      "Conditions": [{
                        "Name": "critical chance",
                        "Template": "+#% to Critical Hit Chance",
                        "NumericConstraints": [{ "Mode": "AtLeast", "Minimum": 3.1 }]
                      }]
                    }]
                  }
                }
              }
            }"#,
        )
        .unwrap();

        let mut store = SettingsStore::new(&path);
        let settings = store.load();
        let poe1 = settings.profiles.poe1.selected_rules();
        let poe2 = settings.profiles.poe2.selected_rules();
        assert_eq!(poe1.rule_editor_mode, RuleEditorMode::Quick);
        assert_eq!(poe1.structured_rule_set.as_ref().unwrap().name, "backup");
        assert_eq!(poe2.rule_editor_mode, RuleEditorMode::Structured);
        let rules = poe2.structured_rule_set.as_ref().unwrap();
        assert_eq!(rules.name, "poe2 rules");
        assert_eq!(rules.groups[0].mode, ResultGroupMode::AtLeast);
        assert_eq!(rules.groups[0].required_count, 2);
        assert_eq!(
            rules.groups[0].conditions[0].numeric_constraints[0].minimum,
            Some("3.1".parse().unwrap())
        );
        assert_eq!(
            settings.profiles.poe1.rules_for("zh-TW"),
            &RuleProfileSettings::default()
        );
        assert_eq!(
            settings.profiles.poe2.rules_for("zh-TW"),
            &RuleProfileSettings::default()
        );
    }

    #[test]
    fn schema_three_ignores_retired_monitoring_policy_and_omits_it_on_save() {
        let directory = TestDirectory::new();
        let path = directory.settings_path();
        fs::write(
            &path,
            r#"{
              "SchemaVersion": 3,
              "SelectedGameProfile": "Poe2",
              "Profiles": {
                "Poe1": {
                  "TargetAffix": "legacy guarded poe1",
                  "OcrLanguage": "zh-TW",
                  "RuleEditorMode": "Quick",
                  "MonitoringPolicy": "Guarded"
                },
                "Poe2": {
                  "TargetAffix": "legacy guarded poe2",
                  "RuleEditorMode": "Structured",
                  "MonitoringPolicy": "Guarded",
                  "StructuredRuleSet": {
                    "SchemaVersion": 1,
                    "Name": "kept rules",
                    "Groups": []
                  }
                }
              }
            }"#,
        )
        .unwrap();

        let mut store = SettingsStore::new(&path);
        let settings = store.load();
        assert_eq!(
            settings.profiles.poe1.selected_rules().target_affix,
            "legacy guarded poe1"
        );
        assert_eq!(settings.profiles.poe1.ocr_language, "zh-TW");
        assert_eq!(
            settings.profiles.poe2.selected_rules().rule_editor_mode,
            RuleEditorMode::Structured
        );
        assert_eq!(
            settings
                .profiles
                .poe2
                .selected_rules()
                .structured_rule_set
                .as_ref()
                .unwrap()
                .name,
            "kept rules"
        );
        assert_eq!(
            settings.profiles.poe1.rules_for("en"),
            &RuleProfileSettings::default()
        );
        assert_eq!(
            settings.profiles.poe2.rules_for("zh-TW"),
            &RuleProfileSettings::default()
        );

        store.save(&settings).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("MonitoringPolicy"));
        assert!(saved.contains("StructuredRuleSet"));
        assert!(saved.contains("SchemaVersion"));
        assert!(!saved.contains("schemaVersion"));
    }

    #[test]
    fn old_flat_settings_migrate_only_into_poe1() {
        let directory = TestDirectory::new();
        let path = directory.settings_path();
        fs::write(
            &path,
            r#"{
              "TargetAffix": "  legacy poe1 target  ",
              "CaptureRegion": { "X": 12, "Y": 34, "Width": 560, "Height": 420 },
              "OcrScale": 1,
              "OcrLanguage": "zh-TW",
              "UiLanguage": "en",
              "KeepHudVisible": false
            }"#,
        )
        .unwrap();

        let mut store = SettingsStore::new(&path);
        let settings = store.load();
        assert_eq!(settings.selected_game_profile, GameProfile::Poe1);
        assert_eq!(
            settings.profiles.poe1.selected_rules().target_affix,
            "legacy poe1 target"
        );
        assert_eq!(settings.profiles.poe1.ocr_language, "zh-TW");
        assert_eq!(
            settings.profiles.poe1.selected_rules().rule_editor_mode,
            RuleEditorMode::Quick
        );
        assert!(
            settings
                .profiles
                .poe1
                .selected_rules()
                .structured_rule_set
                .is_none()
        );
        assert_eq!(
            settings.profiles.poe1.rules_for("en"),
            &RuleProfileSettings::default()
        );
        assert_eq!(settings.profiles.poe2, GameProfileSettings::default());
        assert!(!settings.keep_hud_visible);

        store.save(&settings).unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let root = saved.as_object().unwrap();
        assert!(root.get("Profiles").is_some());
        assert!(root.get("TargetAffix").is_none());
        assert!(root.get("CaptureRegion").is_none());
        assert!(root.get("OcrScale").is_none());
        assert!(root.get("OcrLanguage").is_none());
    }

    #[test]
    fn invalid_values_normalize_without_cross_profile_leaks() {
        let directory = TestDirectory::new();
        let path = directory.settings_path();
        fs::write(
            &path,
            r#"{
              "SchemaVersion": 3,
              "SelectedGameProfile": "Poe3",
              "UiLanguage": "fr",
              "HudPlacement": {
                "MonitorDeviceName": " DISPLAY1 ",
                "RelativeX": 0.5
              },
              "StartMonitoringHotKey": "Ctrl+F1",
              "Profiles": {
                "Poe1": {
                  "TargetAffix": " one ",
                  "CaptureRegion": { "X": 0, "Y": 0, "Width": 0, "Height": 20 },
                  "OcrLanguage": "zh-CN",
                  "RuleEditorMode": "Guarded"
                },
                "Poe2": { "TargetAffix": " two " }
              }
            }"#,
        )
        .unwrap();

        let mut store = SettingsStore::new(&path);
        let settings = store.load();
        assert_eq!(settings.selected_game_profile, GameProfile::Poe1);
        // 未知界面语言回退默认(英文优先);zh* 前缀仍归一化为 zh-CN。
        assert_eq!(settings.ui_language, "en");
        assert_eq!(normalize_ui_language("zh-TW"), "zh-CN");
        assert_eq!(settings.start_monitoring_hot_key, "Ctrl+Shift+F10");
        assert_eq!(settings.hud_placement, HudPlacement::default());
        assert_eq!(settings.profiles.poe1.selected_rules().target_affix, "one");
        assert_eq!(settings.profiles.poe2.selected_rules().target_affix, "two");
        assert_eq!(settings.profiles.poe1.ocr_language, "en");
        assert_eq!(
            settings.profiles.poe1.selected_rules().rule_editor_mode,
            RuleEditorMode::Quick
        );
    }

    #[test]
    fn future_schema_is_read_only_and_original_bytes_survive() {
        let directory = TestDirectory::new();
        let path = directory.settings_path();
        let original = br#"{
          "schemaversion": 1000,
          "SchemaVersion": 1,
          "FutureOnlyValue": "must survive byte-for-byte"
        }"#;
        fs::write(&path, original).unwrap();

        let mut store = SettingsStore::new(&path);
        assert_eq!(store.load(), AppSettings::default());
        assert!(store.is_read_only());
        assert_eq!(store.detected_schema_version(), Some(1000));
        assert!(matches!(
            store.save(&AppSettings::default()),
            Err(SettingsError::SchemaTooNew {
                detected: 1000,
                supported: CURRENT_SCHEMA_VERSION
            })
        ));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(directory.temporary_files().is_empty());
    }

    #[test]
    fn save_without_load_also_protects_future_schema() {
        let directory = TestDirectory::new();
        let path = directory.settings_path();
        let original = br#"{ "SchemaVersion": 999, "FutureOnlyValue": true }"#;
        fs::write(&path, original).unwrap();

        let mut store = SettingsStore::new(&path);
        let error = store.save(&AppSettings::default()).unwrap_err();
        assert!(matches!(
            error,
            SettingsError::SchemaTooNew {
                detected: 999,
                supported: CURRENT_SCHEMA_VERSION
            }
        ));
        assert!(store.is_read_only());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(directory.temporary_files().is_empty());
    }

    #[test]
    fn atomic_save_round_trips_game_and_language_profiles_with_pascal_case_rules() {
        let directory = TestDirectory::new();
        let path = directory.settings_path();
        fs::write(&path, br#"{ "SchemaVersion": 3, "Profiles": {} }"#).unwrap();

        let mut settings = AppSettings {
            selected_game_profile: GameProfile::Poe2,
            keep_hud_visible: false,
            custom_alert_sound_path: Some(r"D:\sounds\alarm.wav".to_owned()),
            start_monitoring_hot_key: "Alt+F10".to_owned(),
            ..AppSettings::default()
        };
        settings.profiles.poe1.ocr_language = "zh-TW".to_owned();
        settings.profiles.poe1.rules_for_mut("en").target_affix = "poe1 english target".to_owned();
        settings.profiles.poe1.rules_for_mut("zh-TW").target_affix =
            "poe1 traditional target".to_owned();
        settings.profiles.poe2 = GameProfileSettings {
            ocr_language: "en".to_owned(),
            rule_profiles: RuleProfileSettingsSet {
                english: RuleProfileSettings {
                    target_affix: "poe2 english target".to_owned(),
                    rule_editor_mode: RuleEditorMode::Structured,
                    structured_rule_set: Some(RuleSetDefinition {
                        schema_version: RULE_SET_SCHEMA_VERSION,
                        name: "crafting rules".to_owned(),
                        groups: vec![AcceptableResultGroup {
                            name: "critical result".to_owned(),
                            mode: ResultGroupMode::AtLeast,
                            required_count: 1,
                            conditions: vec![AffixCondition {
                                name: "critical chance".to_owned(),
                                template: "+#% to Critical Hit Chance".to_owned(),
                                numeric_constraints: vec![NumericConstraint::at_least(
                                    "3.1000000000000000000001".parse::<Decimal>().unwrap(),
                                )],
                            }],
                        }],
                    }),
                },
                traditional_chinese: RuleProfileSettings {
                    target_affix: "poe2 traditional target".to_owned(),
                    ..RuleProfileSettings::default()
                },
            },
        };

        let mut store = SettingsStore::new(&path);
        store.save(&settings).unwrap();
        assert!(directory.temporary_files().is_empty());

        let saved_text = fs::read_to_string(&path).unwrap();
        assert!(saved_text.contains("\"StructuredRuleSet\""));
        assert!(saved_text.contains("\"RequiredCount\""));
        assert!(saved_text.contains("\"NumericConstraints\""));
        assert!(saved_text.contains("3.1000000000000000000001"));
        assert!(saved_text.contains("\"RuleProfiles\""));
        assert!(saved_text.contains("\"English\""));
        assert!(saved_text.contains("\"TraditionalChinese\""));
        assert!(!saved_text.contains("\"structuredRuleSet\""));
        let saved_json: Value = serde_json::from_str(&saved_text).unwrap();
        assert_eq!(
            saved_json.get("SchemaVersion").and_then(Value::as_u64),
            Some(u64::from(CURRENT_SCHEMA_VERSION))
        );
        let saved_poe1 = saved_json
            .get("Profiles")
            .and_then(|profiles| profiles.get("Poe1"))
            .and_then(Value::as_object)
            .unwrap();
        assert!(saved_poe1.get("RuleProfiles").is_some());
        assert!(saved_poe1.get("TargetAffix").is_none());
        assert!(saved_poe1.get("RuleEditorMode").is_none());
        assert!(saved_poe1.get("StructuredRuleSet").is_none());

        let reloaded = store.load();
        assert_eq!(reloaded.selected_game_profile, GameProfile::Poe2);
        assert_eq!(
            reloaded.profiles.poe1.rules_for("en").target_affix,
            "poe1 english target"
        );
        assert_eq!(
            reloaded.profiles.poe1.rules_for("zh-TW").target_affix,
            "poe1 traditional target"
        );
        assert_eq!(reloaded.profiles.poe1.ocr_language, "zh-TW");
        assert_eq!(
            reloaded.profiles.poe2.rules_for("en").target_affix,
            "poe2 english target"
        );
        assert_eq!(
            reloaded.profiles.poe2.rules_for("zh-TW").target_affix,
            "poe2 traditional target"
        );
        let rules = reloaded
            .profiles
            .poe2
            .rules_for("en")
            .structured_rule_set
            .as_ref()
            .unwrap();
        assert_eq!(rules.groups[0].required_count, 1);
        assert_eq!(
            rules.groups[0].conditions[0].numeric_constraints[0].minimum,
            Some("3.1000000000000000000001".parse().unwrap())
        );
    }
}
