use std::fmt;

/// Failure returned by a native platform service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlatformError {
    /// The service is intentionally unavailable on this operating system.
    UnsupportedPlatform { capability: &'static str },
    /// A caller supplied a null native window handle.
    InvalidWindowHandle,
    /// A process-wide native resource is already owned by another instance.
    AlreadyInUse { capability: &'static str },
    /// A Win32 API call failed.
    Win32 {
        operation: &'static str,
        code: u32,
        message: String,
    },
    /// A native service thread could not be created or initialized.
    Thread {
        operation: &'static str,
        detail: String,
    },
}

impl PlatformError {
    #[cfg(not(windows))]
    pub(crate) fn unsupported(capability: &'static str) -> Self {
        Self::UnsupportedPlatform { capability }
    }

    /// Returns the underlying Win32 error code when one is available.
    #[must_use]
    pub fn win32_code(&self) -> Option<u32> {
        match self {
            Self::Win32 { code, .. } => Some(*code),
            _ => None,
        }
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform { capability } => {
                write!(formatter, "{capability} is only available on Windows")
            }
            Self::InvalidWindowHandle => formatter.write_str("the native window handle is null"),
            Self::AlreadyInUse { capability } => {
                write!(formatter, "{capability} is already in use in this process")
            }
            Self::Win32 {
                operation,
                code,
                message,
            } => write!(
                formatter,
                "{operation} failed with Win32 error {code}: {message}"
            ),
            Self::Thread { operation, detail } => {
                write!(formatter, "{operation} failed: {detail}")
            }
        }
    }
}

impl std::error::Error for PlatformError {}
