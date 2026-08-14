//! Native Windows application shell for POE Alarm.
//!
//! The platform-neutral state machine emits typed background commands. The
//! Win32 layer only renders state, captures user intent, and posts asynchronous
//! completion messages back to the UI thread.

#![forbid(unsafe_op_in_unsafe_fn)]

mod alert_cue;
mod localization;
mod model;
pub mod theme;

#[cfg(not(windows))]
mod non_windows;
#[cfg(windows)]
mod win32;

pub use localization::{TextCatalog, UiText};
pub use model::{
    AppState, BackgroundCommand, BackgroundCompletion, ConditionEdit, EditorError, EditorForm,
    Game, GlobalEdit, GroupEdit, MonitorStatus, MonitoringConfig, NumericConstraintEdit,
    OcrLanguage, Operation, RegionEdit, RuleMode, UiAction, UiAvailability, UiLanguage, UiNotice,
    WorkFailure, WorkOutcome,
};
pub use poe_alarm_core::{NumericConstraintMode, ResultGroupMode};

#[cfg(not(windows))]
pub use non_windows::{run, run_self_test};
#[cfg(windows)]
pub use win32::{run, run_self_test};
