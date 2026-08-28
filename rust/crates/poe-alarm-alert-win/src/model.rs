use std::fmt;
use std::time::Duration;

#[cfg(test)]
use poe_alarm_platform_win::MouseButtons;
use poe_alarm_platform_win::{RectI, ValidatedWave};

/// Stable identity assigned to one accepted alert trigger.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct AlertId(pub u64);

/// Caller-localized text shown by the alert. This crate never mixes languages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlertText {
    pub title: String,
    pub detail: String,
    pub button: String,
    pub notice: String,
    pub footer: String,
}

impl AlertText {
    pub const MAXIMUM_TITLE_CHARACTERS: usize = 160;
    pub const MAXIMUM_DETAIL_CHARACTERS: usize = 2_048;
    pub const MAXIMUM_BUTTON_CHARACTERS: usize = 200;
    pub const MAXIMUM_NOTICE_CHARACTERS: usize = 200;
    pub const MAXIMUM_FOOTER_CHARACTERS: usize = 200;

    pub fn new(
        title: impl Into<String>,
        detail: impl Into<String>,
        button: impl Into<String>,
        notice: impl Into<String>,
        footer: impl Into<String>,
    ) -> Result<Self, AlertTextError> {
        let title = normalize_text(title.into());
        let detail = normalize_text(detail.into());
        let button = normalize_text(button.into());
        let notice = normalize_text(notice.into());
        let footer = normalize_text(footer.into());
        validate_field("title", &title, Self::MAXIMUM_TITLE_CHARACTERS)?;
        validate_field("detail", &detail, Self::MAXIMUM_DETAIL_CHARACTERS)?;
        validate_field("button", &button, Self::MAXIMUM_BUTTON_CHARACTERS)?;
        validate_field("notice", &notice, Self::MAXIMUM_NOTICE_CHARACTERS)?;
        validate_field("footer", &footer, Self::MAXIMUM_FOOTER_CHARACTERS)?;
        Ok(Self {
            title,
            detail,
            button,
            notice,
            footer,
        })
    }
}

fn normalize_text(value: String) -> String {
    value.replace('\0', "").trim().to_owned()
}

fn validate_field(
    field: &'static str,
    value: &str,
    maximum_characters: usize,
) -> Result<(), AlertTextError> {
    if value.is_empty() {
        return Err(AlertTextError::Empty { field });
    }
    let actual_characters = value.chars().count();
    if actual_characters > maximum_characters {
        return Err(AlertTextError::TooLong {
            field,
            maximum_characters,
            actual_characters,
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlertTextError {
    Empty {
        field: &'static str,
    },
    TooLong {
        field: &'static str,
        maximum_characters: usize,
        actual_characters: usize,
    },
}

impl fmt::Display for AlertTextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { field } => write!(formatter, "alert {field} is empty"),
            Self::TooLong {
                field,
                maximum_characters,
                actual_characters,
            } => write!(
                formatter,
                "alert {field} has {actual_characters} characters; maximum is {maximum_characters}"
            ),
        }
    }
}

impl std::error::Error for AlertTextError {}

/// Immutable service preferences. A validated local WAV is required so a
/// production alert never discovers an invalid sound after a match.
#[derive(Clone, Debug)]
pub struct AlertServiceConfig {
    pub wave: ValidatedWave,
    /// `true` includes the red alert in screenshots and screen recordings.
    pub allow_overlay_capture: bool,
    /// Maximum time from trigger acceptance to a verified blocking HWND.
    pub presentation_timeout: Duration,
    #[cfg(test)]
    pub(crate) stall_first_presentation: bool,
    #[cfg(test)]
    pub(crate) physical_buttons_override: Option<MouseButtons>,
    #[cfg(test)]
    pub(crate) force_display_affinity_failure: bool,
}

impl AlertServiceConfig {
    pub const DEFAULT_PRESENTATION_TIMEOUT: Duration = Duration::from_millis(750);

    #[must_use]
    pub fn new(wave: ValidatedWave) -> Self {
        Self {
            wave,
            allow_overlay_capture: true,
            presentation_timeout: Self::DEFAULT_PRESENTATION_TIMEOUT,
            #[cfg(test)]
            stall_first_presentation: false,
            #[cfg(test)]
            physical_buttons_override: None,
            #[cfg(test)]
            force_display_affinity_failure: false,
        }
    }
}

/// One red-alert request. `anchor_region` chooses the monitor that hosts the
/// central card while the blocking red shield still covers the full virtual desktop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlertTrigger {
    pub text: AlertText,
    pub anchor_region: Option<RectI>,
}

impl AlertTrigger {
    #[must_use]
    pub const fn new(text: AlertText) -> Self {
        Self {
            text,
            anchor_region: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlertTriggerStatus {
    Accepted(AlertId),
    AlreadyLatched(AlertId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlertTriggerError {
    Stopping,
    WorkerUnavailable,
    CommandQueueBusy,
}

impl fmt::Display for AlertTriggerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stopping => "the alert service is stopping",
            Self::WorkerUnavailable => "the alert worker is unavailable",
            Self::CommandQueueBusy => "the alert command queue is busy",
        })
    }
}

impl std::error::Error for AlertTriggerError {}

/// Machine-readable alert failure category. Human-facing localization belongs
/// to the caller; `detail` is diagnostic text for logs and support reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlertFailureKind {
    UnsupportedPlatform,
    AlreadyInUse,
    ThreadStart,
    Dispatch,
    PresentationTimeout,
    WindowCreation,
    WindowPresentation,
    HitTestVerification,
    DisplayAffinity,
    SoundPlayback,
    InternalProtocol,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlertFailure {
    pub kind: AlertFailureKind,
    pub detail: String,
}

impl AlertFailure {
    pub(crate) fn new(kind: AlertFailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for AlertFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for AlertFailure {}

/// Events are delivered through a non-blocking receiver owned by
/// [`BlockingAlertService`](crate::BlockingAlertService).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlertEvent {
    Presented {
        alert_id: AlertId,
    },
    SoundFailed {
        alert_id: AlertId,
        failure: AlertFailure,
    },
    Acknowledged {
        alert_id: AlertId,
    },
    Failed {
        alert_id: AlertId,
        failure: AlertFailure,
    },
    Stopped,
}
