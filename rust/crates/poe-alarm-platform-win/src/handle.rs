use std::fmt;
use std::num::NonZeroIsize;

/// Non-null native top-level window handle.
///
/// The wrapper is platform-neutral so application crates do not need to expose
/// `windows::Win32::Foundation::HWND` in their public types.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct NativeWindowHandle(NonZeroIsize);

impl NativeWindowHandle {
    /// Wraps an existing native window handle.
    ///
    /// # Safety
    ///
    /// `raw` must be a live HWND owned by the caller for every operation that
    /// receives the returned value. The wrapper does not extend its lifetime.
    pub const unsafe fn from_raw(raw: isize) -> Result<Self, crate::PlatformError> {
        match NonZeroIsize::new(raw) {
            Some(value) => Ok(Self(value)),
            None => Err(crate::PlatformError::InvalidWindowHandle),
        }
    }

    pub(crate) const fn from_known_valid(raw: NonZeroIsize) -> Self {
        Self(raw)
    }

    /// Returns the wrapped HWND value without transferring ownership.
    #[must_use]
    pub const fn as_raw(self) -> isize {
        self.0.get()
    }
}

impl fmt::Debug for NativeWindowHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("NativeWindowHandle")
            .field(&format_args!("0x{:X}", self.as_raw() as usize))
            .finish()
    }
}
