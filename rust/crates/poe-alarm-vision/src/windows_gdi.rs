use std::ffi::c_void;
use std::ptr;
use std::time::SystemTime;

use crate::{CaptureError, CaptureRegion, CapturedFrame, ScreenCapture};

const DIB_RGB_COLORS: u32 = 0;
const SRCCOPY: u32 = 0x00CC_0020;
const CAPTUREBLT: u32 = 0x4000_0000;
const BI_RGB: u32 = 0;
const HGDI_ERROR: isize = -1;

type Handle = *mut c_void;
type Hdc = Handle;
type Hbitmap = Handle;
type Hgdiobj = Handle;

/// Reusable Windows desktop capture using one top-down 32-bit DIB section.
///
/// The native bitmap is retained while its dimensions stay unchanged. Use
/// `capture_into` with a retained `CapturedFrame` to reuse the Rust pixel buffer too.
pub struct GdiScreenCapture {
    screen_dc: Hdc,
    memory_dc: Hdc,
    bitmap: Hbitmap,
    previous_object: Hgdiobj,
    pixel_address: *mut c_void,
    width: u32,
    height: u32,
    stride: usize,
}

impl GdiScreenCapture {
    pub fn new() -> Self {
        Self {
            screen_dc: ptr::null_mut(),
            memory_dc: ptr::null_mut(),
            bitmap: ptr::null_mut(),
            previous_object: ptr::null_mut(),
            pixel_address: ptr::null_mut(),
            width: 0,
            height: 0,
            stride: 0,
        }
    }

    fn ensure_resources(&mut self, width: u32, height: u32) -> Result<(), CaptureError> {
        if !self.bitmap.is_null() && self.width == width && self.height == height {
            return Ok(());
        }
        self.release_resources();

        let result = (|| {
            // SAFETY: A null HWND requests the desktop device context.
            self.screen_dc = unsafe { get_dc(ptr::null_mut()) };
            if self.screen_dc.is_null() {
                return Err(last_os_error("GetDC"));
            }

            // SAFETY: screen_dc was checked above and remains owned until release.
            self.memory_dc = unsafe { create_compatible_dc(self.screen_dc) };
            if self.memory_dc.is_null() {
                return Err(last_os_error("CreateCompatibleDC"));
            }

            self.stride = (width as usize)
                .checked_mul(crate::BYTES_PER_PIXEL)
                .ok_or(CaptureError::Frame(crate::FrameError::DimensionsTooLarge))?;
            let byte_length = self
                .stride
                .checked_mul(height as usize)
                .ok_or(CaptureError::Frame(crate::FrameError::DimensionsTooLarge))?;
            let size_image = u32::try_from(byte_length)
                .map_err(|_| CaptureError::Frame(crate::FrameError::DimensionsTooLarge))?;
            let bitmap_width = i32::try_from(width)
                .map_err(|_| CaptureError::Frame(crate::FrameError::DimensionsTooLarge))?;
            let bitmap_height = i32::try_from(height)
                .map_err(|_| CaptureError::Frame(crate::FrameError::DimensionsTooLarge))?;
            let info = BitmapInfo {
                header: BitmapInfoHeader {
                    size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                    width: bitmap_width,
                    height: -bitmap_height,
                    planes: 1,
                    bit_count: 32,
                    compression: BI_RGB,
                    size_image,
                    x_pels_per_meter: 0,
                    y_pels_per_meter: 0,
                    clr_used: 0,
                    clr_important: 0,
                },
                colors: [0],
            };

            // SAFETY: info is initialized for a 32-bit top-down DIB; pixel_address is an out pointer.
            self.bitmap = unsafe {
                create_dib_section(
                    self.screen_dc,
                    &info,
                    DIB_RGB_COLORS,
                    &mut self.pixel_address,
                    ptr::null_mut(),
                    0,
                )
            };
            if self.bitmap.is_null() || self.pixel_address.is_null() {
                return Err(last_os_error("CreateDIBSection"));
            }

            // SAFETY: memory_dc and bitmap are valid GDI handles created above.
            self.previous_object = unsafe { select_object(self.memory_dc, self.bitmap) };
            if self.previous_object.is_null() || self.previous_object as isize == HGDI_ERROR {
                return Err(last_os_error("SelectObject"));
            }
            self.width = width;
            self.height = height;
            Ok(())
        })();

        if result.is_err() {
            self.release_resources();
        }
        result
    }

    fn release_resources(&mut self) {
        // SAFETY: Each call is guarded by its handle's validity and follows GDI ownership order.
        unsafe {
            if !self.previous_object.is_null()
                && self.previous_object as isize != HGDI_ERROR
                && !self.memory_dc.is_null()
            {
                let _ = select_object(self.memory_dc, self.previous_object);
            }
            if !self.bitmap.is_null() {
                let _ = delete_object(self.bitmap);
            }
            if !self.memory_dc.is_null() {
                let _ = delete_dc(self.memory_dc);
            }
            if !self.screen_dc.is_null() {
                let _ = release_dc(ptr::null_mut(), self.screen_dc);
            }
        }
        self.screen_dc = ptr::null_mut();
        self.memory_dc = ptr::null_mut();
        self.bitmap = ptr::null_mut();
        self.previous_object = ptr::null_mut();
        self.pixel_address = ptr::null_mut();
        self.width = 0;
        self.height = 0;
        self.stride = 0;
    }
}

impl Default for GdiScreenCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenCapture for GdiScreenCapture {
    fn capture_into(
        &mut self,
        region: CaptureRegion,
        destination: &mut CapturedFrame,
    ) -> Result<(), CaptureError> {
        self.ensure_resources(region.width, region.height)?;
        let width = i32::try_from(region.width)
            .map_err(|_| CaptureError::Frame(crate::FrameError::DimensionsTooLarge))?;
        let height = i32::try_from(region.height)
            .map_err(|_| CaptureError::Frame(crate::FrameError::DimensionsTooLarge))?;
        // SAFETY: Both DCs and the selected DIB are live; dimensions match the allocated bitmap.
        let copied = unsafe {
            bit_blt(
                self.memory_dc,
                0,
                0,
                width,
                height,
                self.screen_dc,
                region.x,
                region.y,
                SRCCOPY | CAPTUREBLT,
            )
        };
        // CAPTUREBLT synchronously waits on every layered window in the session.
        // If a layered window's owning thread is not pumping (e.g. the GPUI main
        // window throttled while the game is foreground), BitBlt fails with
        // ERROR_TIMEOUT (1460). Retry once with plain SRCCOPY: the game itself is
        // not a layered window, so the affix pixels are captured identically.
        let copied = if copied == 0 {
            // SAFETY: identical to the call above, minus the CAPTUREBLT flag.
            unsafe {
                bit_blt(
                    self.memory_dc,
                    0,
                    0,
                    width,
                    height,
                    self.screen_dc,
                    region.x,
                    region.y,
                    SRCCOPY,
                )
            }
        } else {
            copied
        };
        if copied == 0 {
            let error = last_os_error("BitBlt");
            self.release_resources();
            return Err(error);
        }

        let byte_length = self.stride * region.height as usize;
        let destination_pixels = destination.prepare_for_capture(region, SystemTime::now())?;
        // SAFETY: CreateDIBSection allocated at least byte_length bytes and destination has exact size.
        unsafe {
            ptr::copy_nonoverlapping(
                self.pixel_address.cast::<u8>(),
                destination_pixels.as_mut_ptr(),
                byte_length,
            );
        }
        Ok(())
    }
}

impl Drop for GdiScreenCapture {
    fn drop(&mut self) {
        self.release_resources();
    }
}

fn last_os_error(operation: &'static str) -> CaptureError {
    CaptureError::Os {
        operation,
        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
    }
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 1],
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDC(window_handle: Handle) -> Hdc;
    fn ReleaseDC(window_handle: Handle, device_context: Hdc) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(device_context: Hdc) -> Hdc;
    fn DeleteDC(device_context: Hdc) -> i32;
    fn CreateDIBSection(
        device_context: Hdc,
        bitmap_info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut c_void,
        section: Handle,
        offset: u32,
    ) -> Hbitmap;
    fn SelectObject(device_context: Hdc, value: Hgdiobj) -> Hgdiobj;
    fn DeleteObject(value: Hgdiobj) -> i32;
    fn BitBlt(
        destination: Hdc,
        destination_x: i32,
        destination_y: i32,
        width: i32,
        height: i32,
        source: Hdc,
        source_x: i32,
        source_y: i32,
        raster_operation: u32,
    ) -> i32;
}

unsafe fn get_dc(window_handle: Handle) -> Hdc {
    // SAFETY: Forwarded directly to the declared Win32 ABI.
    unsafe { GetDC(window_handle) }
}

unsafe fn release_dc(window_handle: Handle, device_context: Hdc) -> i32 {
    // SAFETY: Forwarded directly to the declared Win32 ABI.
    unsafe { ReleaseDC(window_handle, device_context) }
}

unsafe fn create_compatible_dc(device_context: Hdc) -> Hdc {
    // SAFETY: Forwarded directly to the declared Win32 ABI.
    unsafe { CreateCompatibleDC(device_context) }
}

unsafe fn delete_dc(device_context: Hdc) -> i32 {
    // SAFETY: Forwarded directly to the declared Win32 ABI.
    unsafe { DeleteDC(device_context) }
}

unsafe fn create_dib_section(
    device_context: Hdc,
    bitmap_info: *const BitmapInfo,
    usage: u32,
    bits: *mut *mut c_void,
    section: Handle,
    offset: u32,
) -> Hbitmap {
    // SAFETY: Forwarded directly to the declared Win32 ABI.
    unsafe { CreateDIBSection(device_context, bitmap_info, usage, bits, section, offset) }
}

unsafe fn select_object(device_context: Hdc, value: Hgdiobj) -> Hgdiobj {
    // SAFETY: Forwarded directly to the declared Win32 ABI.
    unsafe { SelectObject(device_context, value) }
}

unsafe fn delete_object(value: Hgdiobj) -> i32 {
    // SAFETY: Forwarded directly to the declared Win32 ABI.
    unsafe { DeleteObject(value) }
}

// The wrapper intentionally preserves the Win32 BitBlt ABI one-for-one.
#[allow(clippy::too_many_arguments)]
unsafe fn bit_blt(
    destination: Hdc,
    destination_x: i32,
    destination_y: i32,
    width: i32,
    height: i32,
    source: Hdc,
    source_x: i32,
    source_y: i32,
    raster_operation: u32,
) -> i32 {
    // SAFETY: Forwarded directly to the declared Win32 ABI.
    unsafe {
        BitBlt(
            destination,
            destination_x,
            destination_y,
            width,
            height,
            source,
            source_x,
            source_y,
            raster_operation,
        )
    }
}
