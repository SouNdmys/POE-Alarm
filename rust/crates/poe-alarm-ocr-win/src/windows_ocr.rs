use std::collections::HashMap;
use std::ptr;
use std::time::Instant;

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapBufferAccessMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::Buffer;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
};
use windows::Win32::System::WinRT::{
    IMemoryBufferByteAccess, RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize,
};
use windows::core::Interface;

use crate::{
    OcrCapabilities, OcrError, OcrLane, OcrLanguagePreference, OcrPixelFormat, OcrRecognition,
    OwnedBgraImage, RecognizedLine,
};

/// Private backend owned exclusively by the process-lifetime actor thread.
pub(crate) struct OcrBackend {
    _apartment: ApartmentGuard,
    capabilities: OcrCapabilities,
    engines: HashMap<(OcrLane, OcrLanguagePreference), DirectOcrEngine>,
}

impl OcrBackend {
    pub(crate) fn initialize() -> Result<Self, OcrError> {
        let apartment = ApartmentGuard::enter()?;
        // OCR is the click-to-alert critical path. Above-normal affects only this sleeping actor
        // while it is materializing a result; the system WinRT worker remains OS-managed. Failure
        // is harmless (for example under a restricted token), so startup remains portable.
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) };
        let capabilities = query_capabilities()?;
        Ok(Self {
            _apartment: apartment,
            capabilities,
            engines: HashMap::new(),
        })
    }

    pub(crate) fn capabilities(&mut self) -> Result<OcrCapabilities, OcrError> {
        Ok(self.capabilities.clone())
    }

    pub(crate) fn recognize_on(
        &mut self,
        lane: OcrLane,
        preference: OcrLanguagePreference,
        image: OwnedBgraImage,
    ) -> Result<OcrRecognition, OcrError> {
        let key = (lane, preference);
        if !self.engines.contains_key(&key) {
            let engine = DirectOcrEngine::new(&key.1, &self.capabilities)?;
            self.engines.insert(key.clone(), engine);
        }
        self.engines
            .get_mut(&key)
            .expect("engine was inserted above")
            .recognize(image)
    }
}

struct DirectOcrEngine {
    engine: OcrEngine,
    language_tag: String,
    maximum_image_dimension: u32,
    bitmap: Option<SoftwareBitmap>,
    bitmap_dimensions: (usize, usize),
    gray_buffer: Option<Buffer>,
    gray_capacity: usize,
    _thread_affinity: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl DirectOcrEngine {
    fn new(
        preference: &OcrLanguagePreference,
        capabilities: &OcrCapabilities,
    ) -> Result<Self, OcrError> {
        let available = available_languages()?;
        let selected = select_language(preference, available).ok_or_else(|| {
            OcrError::LanguageUnavailable {
                requested: preference.requested_label().to_owned(),
                available: capabilities.available_language_tags.clone(),
            }
        })?;
        let engine = OcrEngine::TryCreateFromLanguage(&selected.language)
            .map_err(|error| winrt_error("create Windows OCR engine", error))?;
        let actual_tag = engine
            .RecognizerLanguage()
            .and_then(|language| language.LanguageTag())
            .map_err(|error| winrt_error("query selected Windows OCR language", error))?
            .to_string_lossy();
        Ok(Self {
            engine,
            language_tag: actual_tag,
            maximum_image_dimension: capabilities.maximum_image_dimension,
            bitmap: None,
            bitmap_dimensions: (0, 0),
            gray_buffer: None,
            gray_capacity: 0,
            _thread_affinity: std::marker::PhantomData,
        })
    }

    fn recognize(&mut self, image: OwnedBgraImage) -> Result<OcrRecognition, OcrError> {
        let started = Instant::now();
        if image.width() > self.maximum_image_dimension as usize
            || image.height() > self.maximum_image_dimension as usize
        {
            return Err(OcrError::ImageExceedsOcrLimit {
                width: image.width(),
                height: image.height(),
                maximum: self.maximum_image_dimension,
            });
        }
        self.prepare_gray_bitmap(&image)?;
        let bitmap = self
            .bitmap
            .as_ref()
            .expect("prepare_gray_bitmap installs a bitmap");
        let operation = self
            .engine
            .RecognizeAsync(bitmap)
            .map_err(|error| winrt_error("start Windows OCR recognition", error))?;
        let result = operation
            .get()
            .map_err(|error| winrt_error("complete Windows OCR recognition", error))?;
        let winrt_lines = result
            .Lines()
            .map_err(|error| winrt_error("read Windows OCR lines", error))?;
        let line_count = winrt_lines
            .Size()
            .map_err(|error| winrt_error("count Windows OCR lines", error))?;
        let mut lines = Vec::with_capacity(line_count as usize);
        let mut line_items = vec![None; line_count as usize];
        let copied_lines = winrt_lines
            .GetMany(0, &mut line_items)
            .map_err(|error| winrt_error("read Windows OCR lines", error))?;
        if copied_lines != line_count {
            return Err(OcrError::WinRt {
                operation: "read complete Windows OCR line collection",
                hresult: 0,
                message: format!("Windows returned {copied_lines} of {line_count} OCR lines"),
            });
        }
        for line in line_items.into_iter().flatten() {
            let text = line
                .Text()
                .map_err(|error| winrt_error("read Windows OCR line text", error))?
                .to_string_lossy();
            let winrt_words = line
                .Words()
                .map_err(|error| winrt_error("read Windows OCR words", error))?;
            let word_count = winrt_words
                .Size()
                .map_err(|error| winrt_error("count Windows OCR words", error))?;
            let mut word_items = vec![None; word_count as usize];
            let copied_words = winrt_words
                .GetMany(0, &mut word_items)
                .map_err(|error| winrt_error("read Windows OCR words", error))?;
            if copied_words != word_count {
                return Err(OcrError::WinRt {
                    operation: "read complete Windows OCR word collection",
                    hresult: 0,
                    message: format!("Windows returned {copied_words} of {word_count} OCR words"),
                });
            }
            let mut left = f32::INFINITY;
            let mut top = f32::INFINITY;
            let mut right = f32::NEG_INFINITY;
            let mut bottom = f32::NEG_INFINITY;
            for word in word_items.into_iter().flatten() {
                let bounds = word
                    .BoundingRect()
                    .map_err(|error| winrt_error("read Windows OCR word bounds", error))?;
                left = left.min(bounds.X);
                top = top.min(bounds.Y);
                right = right.max(bounds.X + bounds.Width);
                bottom = bottom.max(bounds.Y + bounds.Height);
            }
            if !left.is_finite() {
                continue;
            }
            lines.push(RecognizedLine {
                text,
                left,
                top,
                width: right - left,
                height: bottom - top,
            });
        }
        let elapsed = started.elapsed();
        Ok(OcrRecognition {
            language_tag: self.language_tag.clone(),
            elapsed,
            lines,
        })
    }

    fn prepare_gray_bitmap(&mut self, image: &OwnedBgraImage) -> Result<(), OcrError> {
        let width = image.width();
        let height = image.height();
        if self.bitmap_dimensions != (width, height) {
            if let Some(previous) = self.bitmap.take() {
                let _ = previous.Close();
            }
            self.bitmap = Some(
                SoftwareBitmap::Create(BitmapPixelFormat::Gray8, width as i32, height as i32)
                    .map_err(|error| winrt_error("create reusable Gray8 SoftwareBitmap", error))?,
            );
            self.bitmap_dimensions = (width, height);
        }

        let pixel_count = width
            .checked_mul(height)
            .ok_or(OcrError::DimensionsTooLarge)?;
        if self.gray_capacity < pixel_count {
            self.gray_buffer = Some(
                Buffer::Create(
                    u32::try_from(pixel_count).map_err(|_| OcrError::DimensionsTooLarge)?,
                )
                .map_err(|error| winrt_error("create reusable Gray8 input buffer", error))?,
            );
            self.gray_capacity = pixel_count;
        }
        let gray_buffer = self
            .gray_buffer
            .as_ref()
            .expect("Gray8 input buffer was initialized above");
        gray_buffer
            .SetLength(u32::try_from(pixel_count).map_err(|_| OcrError::DimensionsTooLarge)?)
            .map_err(|error| winrt_error("size reusable Gray8 input buffer", error))?;
        let gray_access: windows::Win32::System::WinRT::IBufferByteAccess = gray_buffer
            .cast()
            .map_err(|error| winrt_error("access reusable Gray8 input buffer", error))?;
        // SAFETY: the WinRT buffer stays alive for this copy and was allocated with at least
        // `pixel_count` bytes. Rows are written to a validated packed Gray8 layout.
        let gray_destination = unsafe {
            gray_access
                .Buffer()
                .map_err(|error| winrt_error("get reusable Gray8 input memory", error))?
        };
        if gray_destination.is_null() {
            return Err(OcrError::WinRt {
                operation: "validate reusable Gray8 input memory",
                hresult: 0,
                message: "Windows returned a null input buffer".to_owned(),
            });
        }
        for row in 0..height {
            let source_start = row * image.stride();
            // SAFETY: row offsets stay inside `pixel_count`; each destination has `width` bytes.
            let destination_row =
                unsafe { std::slice::from_raw_parts_mut(gray_destination.add(row * width), width) };
            match image.format() {
                OcrPixelFormat::Gray8 => {
                    destination_row
                        .copy_from_slice(&image.pixels()[source_start..source_start + width]);
                }
                OcrPixelFormat::Bgra8 => {
                    let source = &image.pixels()[source_start..source_start + width * 4];
                    for (pixel, intensity) in source.chunks_exact(4).zip(destination_row) {
                        *intensity = pixel[0];
                    }
                }
            }
        }

        let bitmap = self
            .bitmap
            .as_ref()
            .expect("bitmap dimensions were initialized above");
        let buffer = bitmap
            .LockBuffer(BitmapBufferAccessMode::Write)
            .map_err(|error| winrt_error("lock reusable Gray8 SoftwareBitmap", error))?;
        let plane = buffer
            .GetPlaneDescription(0)
            .map_err(|error| winrt_error("query Gray8 bitmap plane", error))?;
        let reference = buffer
            .CreateReference()
            .map_err(|error| winrt_error("reference Gray8 bitmap memory", error))?;
        let access: IMemoryBufferByteAccess = reference
            .cast()
            .map_err(|error| winrt_error("access Gray8 bitmap memory", error))?;
        let mut destination = ptr::null_mut();
        let mut capacity = 0_u32;
        // SAFETY: `reference`, `buffer`, and `bitmap` remain alive for the complete copy. The
        // validated plane bounds below keep every write within the reported native capacity.
        unsafe {
            access
                .GetBuffer(&mut destination, &mut capacity)
                .map_err(|error| winrt_error("get Gray8 bitmap memory", error))?;
        }
        if destination.is_null()
            || plane.StartIndex < 0
            || plane.Stride < width as i32
            || plane.Height < height as i32
        {
            return Err(OcrError::WinRt {
                operation: "validate Gray8 bitmap memory",
                hresult: 0,
                message: "Windows returned an invalid bitmap plane".to_owned(),
            });
        }
        let start = plane.StartIndex as usize;
        let destination_stride = plane.Stride as usize;
        let required = start
            .checked_add(destination_stride.saturating_mul(height.saturating_sub(1)))
            .and_then(|offset| offset.checked_add(width))
            .ok_or(OcrError::DimensionsTooLarge)?;
        if required > capacity as usize {
            return Err(OcrError::WinRt {
                operation: "validate Gray8 bitmap capacity",
                hresult: 0,
                message: format!(
                    "Windows reported {capacity} bytes for a bitmap requiring {required}"
                ),
            });
        }

        if destination_stride == width {
            // SAFETY: both buffers have at least `pixel_count` readable/writable bytes and are
            // distinct WinRT allocations kept alive for the copy.
            unsafe {
                ptr::copy_nonoverlapping(gray_destination, destination.add(start), pixel_count);
            }
        } else {
            for row in 0..height {
                // SAFETY: source and destination row bounds were validated above.
                unsafe {
                    ptr::copy_nonoverlapping(
                        gray_destination.add(row * width),
                        destination.add(start + row * destination_stride),
                        width,
                    );
                }
            }
        }
        Ok(())
    }
}

fn query_capabilities() -> Result<OcrCapabilities, OcrError> {
    let maximum_image_dimension = OcrEngine::MaxImageDimension()
        .map_err(|error| winrt_error("query Windows OCR image limit", error))?;
    Ok(OcrCapabilities {
        available_language_tags: available_languages()?
            .into_iter()
            .map(|candidate| candidate.tag)
            .collect(),
        maximum_image_dimension,
    })
}

struct LanguageCandidate {
    language: Language,
    tag: String,
}

fn available_languages() -> Result<Vec<LanguageCandidate>, OcrError> {
    let languages = OcrEngine::AvailableRecognizerLanguages()
        .map_err(|error| winrt_error("enumerate installed Windows OCR languages", error))?;
    let language_count = languages
        .Size()
        .map_err(|error| winrt_error("count installed Windows OCR languages", error))?;
    let mut result = Vec::with_capacity(language_count as usize);
    for index in 0..language_count {
        let language = languages
            .GetAt(index)
            .map_err(|error| winrt_error("read installed Windows OCR language", error))?;
        let tag = language
            .LanguageTag()
            .map_err(|error| winrt_error("read Windows OCR language tag", error))?
            .to_string_lossy();
        result.push(LanguageCandidate { language, tag });
    }
    result.sort_by(|left, right| left.tag.cmp(&right.tag));
    Ok(result)
}

fn select_language(
    preference: &OcrLanguagePreference,
    candidates: Vec<LanguageCandidate>,
) -> Option<LanguageCandidate> {
    candidates
        .into_iter()
        .filter_map(|candidate| {
            language_score(preference, &candidate.tag).map(|score| (score, candidate))
        })
        .max_by(|(left_score, left), (right_score, right)| {
            left_score
                .cmp(right_score)
                .then_with(|| right.tag.cmp(&left.tag))
        })
        .map(|(_, candidate)| candidate)
}

fn language_score(preference: &OcrLanguagePreference, tag: &str) -> Option<u8> {
    let normalized = tag.to_ascii_lowercase();
    match preference {
        OcrLanguagePreference::English => match normalized.as_str() {
            "en-us" => Some(100),
            "en-gb" => Some(95),
            value if value == "en" || value.starts_with("en-") => Some(80),
            _ => None,
        },
        OcrLanguagePreference::TraditionalChinese => match normalized.as_str() {
            "zh-hant-tw" => Some(120),
            "zh-tw" => Some(115),
            "zh-hant-hk" => Some(110),
            "zh-hk" => Some(105),
            "zh-hant-mo" | "zh-mo" => Some(100),
            value if value.starts_with("zh-") && value.contains("hant") => Some(90),
            value
                if value.starts_with("zh-")
                    && (value.ends_with("-tw")
                        || value.ends_with("-hk")
                        || value.ends_with("-mo")) =>
            {
                Some(80)
            }
            _ => None,
        },
        OcrLanguagePreference::Exact(expected) if expected.eq_ignore_ascii_case(tag) => Some(255),
        OcrLanguagePreference::Exact(_) => None,
    }
}

struct ApartmentGuard {
    must_uninitialize: bool,
    _thread_affinity: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl ApartmentGuard {
    fn enter() -> Result<Self, OcrError> {
        // SAFETY: production owns this apartment on one process-lifetime worker
        // thread. The guard would balance an early initialization failure.
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Ok(Self {
                must_uninitialize: true,
                _thread_affinity: std::marker::PhantomData,
            }),
            Err(error) if error.code() == RPC_E_CHANGED_MODE => Ok(Self {
                must_uninitialize: false,
                _thread_affinity: std::marker::PhantomData,
            }),
            Err(error) => Err(winrt_error("initialize Windows Runtime apartment", error)),
        }
    }
}

impl Drop for ApartmentGuard {
    fn drop(&mut self) {
        if self.must_uninitialize {
            // SAFETY: the actor backend cannot move away from its owning thread.
            unsafe { RoUninitialize() };
        }
    }
}

fn winrt_error(operation: &'static str, error: windows::core::Error) -> OcrError {
    OcrError::WinRt {
        operation,
        hresult: error.code().0,
        message: error.message(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_scoring_prefers_expected_regional_recognizers() {
        assert!(
            language_score(&OcrLanguagePreference::English, "en-US")
                > language_score(&OcrLanguagePreference::English, "en-AU")
        );
        assert!(
            language_score(&OcrLanguagePreference::TraditionalChinese, "zh-Hant-TW")
                > language_score(&OcrLanguagePreference::TraditionalChinese, "zh-HK")
        );
        assert_eq!(
            language_score(&OcrLanguagePreference::TraditionalChinese, "zh-Hans-CN"),
            None
        );
    }
}
