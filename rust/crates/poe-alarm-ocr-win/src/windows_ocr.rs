use std::collections::HashMap;
use std::ptr;
use std::time::Instant;

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapBufferAccessMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
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
        let previous = replace_sized_resource(
            &mut self.bitmap,
            &mut self.bitmap_dimensions,
            (width, height),
            || {
                SoftwareBitmap::Create(BitmapPixelFormat::Gray8, width as i32, height as i32)
                    .map_err(|error| winrt_error("create reusable Gray8 SoftwareBitmap", error))
            },
        )?;
        if let Some(previous) = previous {
            let _ = previous.Close();
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
            || plane.Width < width as i32
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
        let plane_length = destination_stride
            .checked_mul(height.saturating_sub(1))
            .and_then(|offset| offset.checked_add(width))
            .ok_or(OcrError::DimensionsTooLarge)?;
        let required = start
            .checked_add(plane_length)
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

        // SAFETY: the bitmap, lock, reference, and access objects remain alive through the copy;
        // the native capacity and plane bounds above prove this slice is wholly writable.
        let destination =
            unsafe { std::slice::from_raw_parts_mut(destination.add(start), plane_length) };
        copy_image_to_gray_plane(image, destination, destination_stride)?;
        Ok(())
    }
}

/// Replaces a dimension-bound resource transactionally: a failed allocation leaves both the
/// previous resource and its dimensions intact. This keeps the bitmap cache usable if a resize
/// fails and the next frame returns to the previous size.
fn replace_sized_resource<T, E>(
    resource: &mut Option<T>,
    dimensions: &mut (usize, usize),
    requested_dimensions: (usize, usize),
    create: impl FnOnce() -> Result<T, E>,
) -> Result<Option<T>, E> {
    if resource.is_some() && *dimensions == requested_dimensions {
        return Ok(None);
    }

    let replacement = create()?;
    let previous = resource.replace(replacement);
    *dimensions = requested_dimensions;
    Ok(previous)
}

fn copy_image_to_gray_plane(
    image: &OwnedBgraImage,
    destination: &mut [u8],
    destination_stride: usize,
) -> Result<(), OcrError> {
    let width = image.width();
    let height = image.height();
    if destination_stride < width {
        return Err(OcrError::WinRt {
            operation: "validate Gray8 bitmap memory",
            hresult: 0,
            message: "Windows returned a bitmap stride smaller than its width".to_owned(),
        });
    }
    let required = destination_stride
        .checked_mul(height.saturating_sub(1))
        .and_then(|offset| offset.checked_add(width))
        .ok_or(OcrError::DimensionsTooLarge)?;
    if destination.len() < required {
        return Err(OcrError::WinRt {
            operation: "validate Gray8 bitmap capacity",
            hresult: 0,
            message: format!(
                "Windows exposed {} bytes for a bitmap plane requiring {required}",
                destination.len()
            ),
        });
    }

    match image.format() {
        OcrPixelFormat::Gray8 => {
            for row in 0..height {
                let source_start = row * image.stride();
                let destination_start = row * destination_stride;
                destination[destination_start..destination_start + width]
                    .copy_from_slice(&image.pixels()[source_start..source_start + width]);
            }
        }
        OcrPixelFormat::Bgra8 => {
            for row in 0..height {
                let source_start = row * image.stride();
                let destination_start = row * destination_stride;
                let source = &image.pixels()[source_start..source_start + width * 4];
                let destination_row =
                    &mut destination[destination_start..destination_start + width];
                for (pixel, intensity) in source.chunks_exact(4).zip(destination_row) {
                    *intensity = pixel[0];
                }
            }
        }
    }
    Ok(())
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
    fn direct_gray_plane_copy_preserves_bgra_and_native_strides() {
        let image = OwnedBgraImage::new(
            3,
            2,
            16,
            vec![
                10, 11, 12, 13, 20, 21, 22, 23, 30, 31, 32, 33, 201, 202, 203, 204, 40, 41, 42, 43,
                50, 51, 52, 53, 60, 61, 62, 63, 211, 212, 213, 214,
            ],
        )
        .unwrap();
        let mut destination = vec![0xEE; 8];

        copy_image_to_gray_plane(&image, &mut destination, 5).unwrap();

        assert_eq!(destination, [10, 20, 30, 0xEE, 0xEE, 40, 50, 60]);
    }

    #[test]
    fn direct_gray_plane_copy_preserves_gray8_and_native_strides() {
        let image = OwnedBgraImage::gray8(3, 2, 4, vec![1, 2, 3, 201, 4, 5, 6, 202]).unwrap();
        let mut destination = vec![0xEE; 9];

        copy_image_to_gray_plane(&image, &mut destination, 6).unwrap();

        assert_eq!(destination, [1, 2, 3, 0xEE, 0xEE, 0xEE, 4, 5, 6]);
    }

    #[test]
    fn direct_gray_plane_copy_rejects_invalid_destination_layouts() {
        let image = OwnedBgraImage::gray8(3, 2, 3, vec![1, 2, 3, 4, 5, 6]).unwrap();

        assert!(matches!(
            copy_image_to_gray_plane(&image, &mut [0; 6], 2),
            Err(OcrError::WinRt {
                operation: "validate Gray8 bitmap memory",
                ..
            })
        ));
        assert!(matches!(
            copy_image_to_gray_plane(&image, &mut [0; 6], 4),
            Err(OcrError::WinRt {
                operation: "validate Gray8 bitmap capacity",
                ..
            })
        ));
    }

    #[test]
    fn failed_resource_resize_preserves_resource_and_dimension_cache() {
        let mut resource = Some("old bitmap");
        let mut dimensions = (320, 96);

        let resize: Result<Option<&str>, &str> =
            replace_sized_resource(&mut resource, &mut dimensions, (640, 192), || {
                Err("allocation failed")
            });

        assert_eq!(resize, Err("allocation failed"));
        assert_eq!(resource, Some("old bitmap"));
        assert_eq!(dimensions, (320, 96));

        let mut create_called = false;
        let reuse = replace_sized_resource(&mut resource, &mut dimensions, (320, 96), || {
            create_called = true;
            Ok::<_, &str>("unexpected bitmap")
        })
        .unwrap();
        assert_eq!(reuse, None);
        assert!(!create_called);
        assert_eq!(resource, Some("old bitmap"));
    }

    #[test]
    fn missing_resource_is_rebuilt_even_when_dimension_cache_matches() {
        let mut resource = None;
        let mut dimensions = (320, 96);

        let previous = replace_sized_resource(&mut resource, &mut dimensions, (320, 96), || {
            Ok::<_, ()>("replacement bitmap")
        })
        .unwrap();

        assert_eq!(previous, None);
        assert_eq!(resource, Some("replacement bitmap"));
        assert_eq!(dimensions, (320, 96));
    }

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
