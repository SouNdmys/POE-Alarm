use std::path::PathBuf;
use std::time::Duration;

use poe_alarm_monitoring::{MonitorSnapshot, MonitorState};
use poe_alarm_platform_win::{HotKeyConfig, StartMonitoringHotKey};
use poe_alarm_settings::{AppSettings, HudPlacement};
use poe_alarm_vision::CaptureRegion;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RuntimeGeneration(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RuntimeRequestId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeOperation {
    Startup,
    Start,
    Monitoring,
    Stop,
    Screenshot,
    Alert,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RuntimeState {
    #[default]
    Idle,
    Starting,
    Monitoring,
    TestingScreenshot,
    MatchFound,
    Faulted,
    ShuttingDown,
    Stopped,
}

#[derive(Clone, Debug)]
pub struct ScreenshotRequest {
    pub request_id: RuntimeRequestId,
    pub settings: AppSettings,
    pub path: PathBuf,
}

impl ScreenshotRequest {
    #[must_use]
    pub fn new(
        request_id: RuntimeRequestId,
        settings: AppSettings,
        path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            request_id,
            settings,
            path: path.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum RuntimeCommand {
    Start { settings: AppSettings },
    Stop,
    Screenshot(ScreenshotRequest),
    CancelScreenshot { request_id: RuntimeRequestId },
    AlertAck,
    Shutdown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlertCopy {
    pub title: String,
    pub button: String,
}

impl AlertCopy {
    pub(crate) fn for_ui_language(language: &str) -> Self {
        if language.eq_ignore_ascii_case("en") {
            Self {
                title: "Target found".to_owned(),
                button: "Acknowledge".to_owned(),
            }
        } else {
            Self {
                title: "已命中目标".to_owned(),
                button: "确认".to_owned(),
            }
        }
    }
}

/// Window-thread-owned configuration compiled alongside the monitor plan.
///
/// Global hot-key registration and HUD HWND ownership deliberately remain on
/// the native UI/message-loop thread. The runtime never creates a second
/// message loop that could steal those messages.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledUiBindings {
    pub hot_keys: HotKeyConfig,
    pub keep_hud_visible: bool,
    pub hud_placement: HudPlacement,
    pub allow_overlay_capture: bool,
}

impl CompiledUiBindings {
    pub(crate) fn from_settings(settings: &AppSettings) -> Self {
        Self {
            hot_keys: HotKeyConfig {
                start: StartMonitoringHotKey::parse_or_default(Some(
                    settings.start_monitoring_hot_key.as_str(),
                )),
            },
            keep_hud_visible: settings.keep_hud_visible,
            hud_placement: settings.hud_placement.clone(),
            allow_overlay_capture: settings.allow_overlay_capture,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectionSummary {
    pub detail: String,
    pub lines: Vec<String>,
    pub matched_group: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenshotEvaluation {
    pub is_match: bool,
    pub detail: Option<String>,
    pub matched_group: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenshotReport {
    pub request_id: RuntimeRequestId,
    pub generation: RuntimeGeneration,
    pub requested_region: CaptureRegion,
    pub used_full_image_fallback: bool,
    pub lines: Vec<String>,
    pub load_elapsed: Duration,
    pub preprocessing_elapsed: Duration,
    pub recognition_elapsed: Duration,
    pub evaluation_elapsed: Duration,
    pub evaluation: ScreenshotEvaluation,
}

#[derive(Clone, Debug)]
pub enum RuntimeEvent {
    Ready,
    StateChanged {
        generation: Option<RuntimeGeneration>,
        state: RuntimeState,
    },
    SettingsCompiled {
        generation: RuntimeGeneration,
        profile: RecognitionProfile,
        region: CaptureRegion,
        ui: CompiledUiBindings,
    },
    MonitorSnapshot {
        generation: RuntimeGeneration,
        snapshot: MonitorSnapshot,
    },
    MatchFound {
        generation: RuntimeGeneration,
        detection: DetectionSummary,
    },
    ScreenshotCompleted(ScreenshotReport),
    ScreenshotCancelled {
        request_id: RuntimeRequestId,
    },
    AlertPresented {
        generation: RuntimeGeneration,
    },
    AlertAcknowledged {
        generation: RuntimeGeneration,
    },
    /// The blocking red overlay remains active; only audio playback failed.
    AlertSoundFailed {
        generation: RuntimeGeneration,
        detail: String,
    },
    Fault {
        generation: Option<RuntimeGeneration>,
        operation: RuntimeOperation,
        detail: String,
    },
    ShutdownComplete {
        elapsed: Duration,
        within_one_second: bool,
    },
}

impl RuntimeEvent {
    /// Maps the monitor's public snapshot state onto the UI-facing runtime state.
    #[must_use]
    pub fn monitor_state(snapshot: &MonitorSnapshot) -> RuntimeState {
        match snapshot.state {
            MonitorState::Idle => RuntimeState::Idle,
            MonitorState::Running => RuntimeState::Monitoring,
            MonitorState::MatchFound => RuntimeState::MatchFound,
            MonitorState::Faulted => RuntimeState::Faulted,
        }
    }
}
