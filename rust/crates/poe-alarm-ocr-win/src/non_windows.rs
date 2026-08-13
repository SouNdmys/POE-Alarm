use crate::{
    OcrCapabilities, OcrError, OcrLane, OcrLanguagePreference, OcrRecognition, OwnedBgraImage,
};

pub(crate) struct OcrBackend;

impl OcrBackend {
    pub(crate) fn initialize() -> Result<Self, OcrError> {
        Err(OcrError::PlatformUnavailable)
    }

    pub(crate) fn capabilities(&mut self) -> Result<OcrCapabilities, OcrError> {
        Err(OcrError::PlatformUnavailable)
    }

    pub(crate) fn recognize_on(
        &mut self,
        _lane: OcrLane,
        _preference: OcrLanguagePreference,
        _image: OwnedBgraImage,
    ) -> Result<OcrRecognition, OcrError> {
        Err(OcrError::PlatformUnavailable)
    }
}
