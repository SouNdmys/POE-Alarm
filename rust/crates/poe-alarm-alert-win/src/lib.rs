//! Native red match alert for POE Alarm.
//!
//! The alert is deliberately isolated from recognition and rule evaluation. A
//! dedicated native thread owns the blocking Win32 window and its message
//! pump. Runtime control methods enqueue bounded commands and never wait for
//! window presentation or acknowledgement.

#![forbid(unsafe_op_in_unsafe_fn)]

mod model;
#[cfg(not(windows))]
mod non_windows;
mod protocol;
mod service;
#[cfg(windows)]
mod win32;

pub use model::{
    AlertEvent, AlertFailure, AlertFailureKind, AlertId, AlertServiceConfig, AlertText,
    AlertTextError, AlertTrigger, AlertTriggerError, AlertTriggerStatus,
};
pub use service::BlockingAlertService;
