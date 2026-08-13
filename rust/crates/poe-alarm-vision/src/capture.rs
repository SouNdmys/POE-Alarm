use std::fmt;

use crate::{CaptureRegion, CapturedFrame, FrameError};

/// A capture backend that can reuse the caller-owned frame allocation.
pub trait ScreenCapture {
    fn capture_into(
        &mut self,
        region: CaptureRegion,
        destination: &mut CapturedFrame,
    ) -> Result<(), CaptureError>;

    fn capture(&mut self, region: CaptureRegion) -> Result<CapturedFrame, CaptureError> {
        let mut frame = CapturedFrame::default();
        self.capture_into(region, &mut frame)?;
        Ok(frame)
    }
}

#[derive(Debug)]
pub enum CaptureError {
    Frame(FrameError),
    Os { operation: &'static str, code: i32 },
    PlatformUnavailable(&'static str),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frame(error) => error.fmt(formatter),
            Self::Os { operation, code } => {
                write!(
                    formatter,
                    "{operation} failed with operating-system error {code}"
                )
            }
            Self::PlatformUnavailable(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Frame(error) => Some(error),
            _ => None,
        }
    }
}

impl From<FrameError> for CaptureError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}
