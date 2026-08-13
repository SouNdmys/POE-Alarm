//! Native OCR orchestration for POE Alarm.
//!
//! The monitor owns capture and the one-per-frame blue mask/fingerprint pass. This crate consumes
//! that prepared evidence and keeps the established recognition profiles isolated:
//!
//! - POE1 English: scale-2 physical bands with a bounded WinRT band cache;
//! - Traditional Chinese: one combined mask/layout pass with bounded, strictly verified recovery;
//! - POE2 English: scale-1 bands, an independent confirmation pass and at most two localized
//!   Paddle work units.
//!
//! Quick and Structured state are deliberately separate. Structured recognition processes a
//! compiled target set once; it never loops through the Quick entry point once per target.

#![forbid(unsafe_code)]

mod adapter;
mod backend;
mod projection;

pub use adapter::{
    GameVersion, PaddleBackendConfig, ProductionRecognizer, RecognitionAdapter, RecognitionError,
    RecognitionLanguage, RecognitionProfile,
};
