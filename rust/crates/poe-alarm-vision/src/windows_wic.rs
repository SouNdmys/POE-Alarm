use std::fmt;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::time::SystemTime;

use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::{Interface, PCWSTR};

use crate::{CaptureRegion, CapturedFrame, FrameError};

/// Process-local Windows Imaging Component decoder for screenshot replay.
///
/// The decoder converts PNG/JPEG inputs to the exact top-down BGRA8 contract used by live GDI
/// capture. A decoder stays on its creating thread because its COM apartment is thread-affine.
pub struct WicScreenshotDecoder {
    factory: IWICImagingFactory,
    _apartment: ComApartment,
}

impl WicScreenshotDecoder {
    pub fn new() -> Result<Self, ScreenshotDecodeError> {
        let apartment = ComApartment::enter()?;
        // SAFETY: COM is initialized for this thread and the requested class is an in-process WIC
        // factory. The resulting interface is owned by `self` and released before the apartment.
        let factory = unsafe {
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(
                |error| ScreenshotDecodeError::Windows {
                    operation: "create WIC imaging factory",
                    hresult: error.code().0,
                    message: error.message(),
                },
            )?
        };
        Ok(Self {
            factory,
            _apartment: apartment,
        })
    }

    pub fn decode(
        &self,
        path: impl AsRef<Path>,
        crop: Option<CaptureRegion>,
    ) -> Result<CapturedFrame, ScreenshotDecodeError> {
        let path = path.as_ref();
        if !path.is_file() {
            return Err(ScreenshotDecodeError::FileNotFound(path.to_owned()));
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !extension.eq_ignore_ascii_case("png")
            && !extension.eq_ignore_ascii_case("jpg")
            && !extension.eq_ignore_ascii_case("jpeg")
        {
            return Err(ScreenshotDecodeError::UnsupportedExtension(
                extension.to_owned(),
            ));
        }

        let wide_path = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: wide_path is NUL terminated and remains alive for the call.
        let decoder = unsafe {
            self.factory
                .CreateDecoderFromFilename(
                    PCWSTR(wide_path.as_ptr()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(|error| windows_error("open screenshot", error))?
        };
        // SAFETY: frame zero exists for supported still-image formats.
        let frame = unsafe {
            decoder
                .GetFrame(0)
                .map_err(|error| windows_error("decode screenshot frame", error))?
        };
        // SAFETY: factory and decoded frame are live COM interfaces on this apartment.
        let converter = unsafe {
            self.factory
                .CreateFormatConverter()
                .map_err(|error| windows_error("create WIC format converter", error))?
        };
        // SAFETY: the converter retains the source for this synchronous conversion. No palette is
        // needed for 32-bit BGRA output.
        unsafe {
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(|error| windows_error("convert screenshot to BGRA8", error))?;
        }

        let source: IWICBitmapSource = converter
            .cast()
            .map_err(|error| windows_error("open converted screenshot pixels", error))?;
        let mut source_width = 0;
        let mut source_height = 0;
        // SAFETY: valid output pointers for the live bitmap source.
        unsafe {
            source
                .GetSize(&mut source_width, &mut source_height)
                .map_err(|error| windows_error("read screenshot dimensions", error))?;
        }
        let region = crop.unwrap_or(
            CaptureRegion::new(0, 0, source_width, source_height)
                .map_err(ScreenshotDecodeError::Frame)?,
        );
        validate_crop(region, source_width, source_height)?;
        let stride = usize::try_from(region.width)
            .ok()
            .and_then(|width| width.checked_mul(crate::BYTES_PER_PIXEL))
            .ok_or(ScreenshotDecodeError::Frame(FrameError::DimensionsTooLarge))?;
        let byte_length = stride
            .checked_mul(region.height as usize)
            .ok_or(ScreenshotDecodeError::Frame(FrameError::DimensionsTooLarge))?;
        let mut pixels = vec![0; byte_length];
        let rectangle = windows::Win32::Graphics::Imaging::WICRect {
            X: region.x,
            Y: region.y,
            Width: i32::try_from(region.width)
                .map_err(|_| ScreenshotDecodeError::Frame(FrameError::DimensionsTooLarge))?,
            Height: i32::try_from(region.height)
                .map_err(|_| ScreenshotDecodeError::Frame(FrameError::DimensionsTooLarge))?,
        };
        // SAFETY: rectangle is validated within the source; stride and output length match BGRA8.
        unsafe {
            source
                .CopyPixels(&rectangle, stride as u32, &mut pixels)
                .map_err(|error| windows_error("copy screenshot BGRA pixels", error))?;
        }
        CapturedFrame::from_bgra(
            CaptureRegion::new(region.x, region.y, region.width, region.height)
                .map_err(ScreenshotDecodeError::Frame)?,
            stride,
            pixels,
            SystemTime::now(),
        )
        .map_err(ScreenshotDecodeError::Frame)
    }
}

fn validate_crop(
    region: CaptureRegion,
    source_width: u32,
    source_height: u32,
) -> Result<(), ScreenshotDecodeError> {
    if region.x < 0
        || region.y < 0
        || region.x as u32 >= source_width
        || region.y as u32 >= source_height
        || region.width > source_width - region.x as u32
        || region.height > source_height - region.y as u32
    {
        return Err(ScreenshotDecodeError::CropOutOfBounds {
            region,
            source_width,
            source_height,
        });
    }
    Ok(())
}

struct ComApartment {
    must_uninitialize: bool,
    _thread_affinity: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl ComApartment {
    fn enter() -> Result<Self, ScreenshotDecodeError> {
        // SAFETY: this decoder owns a balanced COM initialization on its creating thread.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_ok() {
            return Ok(Self {
                must_uninitialize: true,
                _thread_affinity: std::marker::PhantomData,
            });
        }
        Err(ScreenshotDecodeError::Windows {
            operation: "initialize COM for WIC",
            hresult: result.0,
            message: format!("HRESULT 0x{:08X}", result.0 as u32),
        })
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.must_uninitialize {
            // SAFETY: balanced with this object's successful CoInitializeEx on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

#[derive(Debug)]
pub enum ScreenshotDecodeError {
    FileNotFound(std::path::PathBuf),
    UnsupportedExtension(String),
    CropOutOfBounds {
        region: CaptureRegion,
        source_width: u32,
        source_height: u32,
    },
    Frame(FrameError),
    Windows {
        operation: &'static str,
        hresult: i32,
        message: String,
    },
}

impl fmt::Display for ScreenshotDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileNotFound(path) => {
                write!(formatter, "screenshot not found: {}", path.display())
            }
            Self::UnsupportedExtension(extension) => write!(
                formatter,
                "screenshot replay supports PNG and JPEG, not .{extension}"
            ),
            Self::CropOutOfBounds {
                region,
                source_width,
                source_height,
            } => write!(
                formatter,
                "crop {region:?} is outside screenshot {source_width}x{source_height}"
            ),
            Self::Frame(error) => error.fmt(formatter),
            Self::Windows {
                operation,
                hresult,
                message,
            } => write!(
                formatter,
                "{operation} failed with HRESULT 0x{:08X}: {message}",
                *hresult as u32
            ),
        }
    }
}

impl std::error::Error for ScreenshotDecodeError {}

fn windows_error(operation: &'static str, error: windows::core::Error) -> ScreenshotDecodeError {
    ScreenshotDecodeError::Windows {
        operation,
        hresult: error.code().0,
        message: error.message(),
    }
}
