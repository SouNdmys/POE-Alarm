//! Pixel hot path for POE Alarm.
//!
//! The crate owns the platform-neutral BGRA frame contract, the POE blue-text
//! mask and semantic fingerprint, physical-line detection, and a reusable GDI
//! capture implementation on Windows. OCR and rule matching deliberately live
//! outside this crate.

#![forbid(unsafe_op_in_unsafe_fn)]

mod bands;
mod capture;
mod frame;
mod mask;

#[cfg(windows)]
mod windows_gdi;
#[cfg(windows)]
mod windows_wic;

pub use bands::{
    BandDetection, BandDetectionError, BandDetectionSettings, BgraCropBuffer, CropMetadata,
    CroppedMask, PhysicalBandDetector, PhysicalBandIdentity, PhysicalLineBand,
    combined_crop_metadata, crop_mask_bgra,
};
pub use capture::{CaptureError, ScreenCapture};
pub use frame::{BYTES_PER_PIXEL, CaptureRegion, CapturedFrame, FrameError, PixelRect};
pub use mask::{
    BlueMaskIntensityMode, BlueMaskSettings, BlueTextMask, FNV1A_64_OFFSET_BASIS, FNV1A_64_PRIME,
    build_blue_mask, build_blue_mask_into, fnv1a64,
};

#[cfg(windows)]
pub use windows_gdi::GdiScreenCapture;
#[cfg(windows)]
pub use windows_wic::{ScreenshotDecodeError, WicScreenshotDecoder};
