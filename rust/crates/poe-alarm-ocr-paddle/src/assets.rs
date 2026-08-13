use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::PaddleError;

pub const MODEL_FILE_NAME: &str = "PP-OCRv5_mobile_rec.onnx";
pub const DICTIONARY_FILE_NAME: &str = "ppocrv5_dict.txt";
pub const EXPECTED_MODEL_BYTES: usize = 16_534_782;
pub const EXPECTED_DICTIONARY_BYTES: usize = 74_012;
pub const EXPECTED_DICTIONARY_ENTRIES: usize = 18_383;
pub const EXPECTED_CLASS_COUNT: usize = 18_385;
pub const EXPECTED_MODEL_SHA256: &str =
    "DA72DC72CA4DC220DF0DFDE68C1DEDC31C58D3E76A25871122E5056227D50092";
pub const EXPECTED_DICTIONARY_SHA256: &str =
    "D1979E9F794C464C0D2E0B70A7FE14DD978E9DC644C0E71F14158CDF8342AF1B";
pub const EXPECTED_DICTIONARY_FIRST: &str = "　";
pub const EXPECTED_DICTIONARY_LAST: &str = "🕧";

/// Paths to the model and dictionary distributed with POE Alarm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaddleAssetPaths {
    pub model: PathBuf,
    pub dictionary: PathBuf,
}

impl PaddleAssetPaths {
    pub fn new(model: impl Into<PathBuf>, dictionary: impl Into<PathBuf>) -> Self {
        Self {
            model: model.into(),
            dictionary: dictionary.into(),
        }
    }

    /// Resolves the checked-in official assets when running from this repository.
    pub fn source_tree() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let asset_root = root.join("src/PoeAlarm.App/Assets/Ocr");
        Self::new(
            asset_root.join(MODEL_FILE_NAME),
            asset_root.join(DICTIONARY_FILE_NAME),
        )
    }
}

/// Validated official model bytes and an exact, whitespace-preserving dictionary.
#[derive(Clone, Debug)]
pub struct PaddleAssets {
    model: Arc<[u8]>,
    dictionary_bytes: Arc<[u8]>,
    dictionary: Arc<[String]>,
    model_sha256: String,
    dictionary_sha256: String,
}

impl PaddleAssets {
    pub fn load(paths: &PaddleAssetPaths) -> Result<Self, PaddleError> {
        let model = fs::read(&paths.model).map_err(|error| PaddleError::io(&paths.model, error))?;
        let dictionary = fs::read(&paths.dictionary)
            .map_err(|error| PaddleError::io(&paths.dictionary, error))?;
        Self::from_bytes(model, dictionary)
    }

    pub fn load_source_tree() -> Result<Self, PaddleError> {
        Self::load(&PaddleAssetPaths::source_tree())
    }

    /// Validates bytes against the immutable hashes recorded for POE Alarm 1.0.
    pub fn from_bytes(model: Vec<u8>, dictionary_bytes: Vec<u8>) -> Result<Self, PaddleError> {
        let model_hash = sha256_hex(&model);
        let dictionary_hash = sha256_hex(&dictionary_bytes);
        validate_length_and_hash(
            MODEL_FILE_NAME,
            &model,
            EXPECTED_MODEL_BYTES,
            &model_hash,
            EXPECTED_MODEL_SHA256,
        )?;
        validate_length_and_hash(
            DICTIONARY_FILE_NAME,
            &dictionary_bytes,
            EXPECTED_DICTIONARY_BYTES,
            &dictionary_hash,
            EXPECTED_DICTIONARY_SHA256,
        )?;

        let dictionary = parse_dictionary(&dictionary_bytes)?;
        if dictionary.len() != EXPECTED_DICTIONARY_ENTRIES {
            return Err(PaddleError::InvalidAsset(format!(
                "dictionary contains {} entries; expected {EXPECTED_DICTIONARY_ENTRIES}",
                dictionary.len()
            )));
        }
        if dictionary.first().map(String::as_str) != Some(EXPECTED_DICTIONARY_FIRST) {
            return Err(PaddleError::InvalidAsset(
                "dictionary first entry is not U+3000 IDEOGRAPHIC SPACE".to_owned(),
            ));
        }
        if dictionary.last().map(String::as_str) != Some(EXPECTED_DICTIONARY_LAST) {
            return Err(PaddleError::InvalidAsset(
                "dictionary final entry is not U+1F567 CLOCK FACE TWELVE-THIRTY".to_owned(),
            ));
        }

        Ok(Self {
            model: model.into(),
            dictionary_bytes: dictionary_bytes.into(),
            dictionary: dictionary.into(),
            model_sha256: model_hash,
            dictionary_sha256: dictionary_hash,
        })
    }

    pub fn model(&self) -> &[u8] {
        &self.model
    }

    pub fn dictionary_bytes(&self) -> &[u8] {
        &self.dictionary_bytes
    }

    pub fn dictionary(&self) -> &[String] {
        &self.dictionary
    }

    pub fn model_sha256(&self) -> &str {
        &self.model_sha256
    }

    pub fn dictionary_sha256(&self) -> &str {
        &self.dictionary_sha256
    }
}

fn validate_length_and_hash(
    name: &str,
    bytes: &[u8],
    expected_length: usize,
    actual_hash: &str,
    expected_hash: &str,
) -> Result<(), PaddleError> {
    if bytes.len() != expected_length {
        return Err(PaddleError::InvalidAsset(format!(
            "{name} is {} bytes; expected {expected_length}",
            bytes.len()
        )));
    }
    if actual_hash != expected_hash {
        return Err(PaddleError::InvalidAsset(format!(
            "{name} SHA-256 is {actual_hash}; expected {expected_hash}"
        )));
    }
    Ok(())
}

fn parse_dictionary(bytes: &[u8]) -> Result<Vec<String>, PaddleError> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        PaddleError::InvalidAsset(format!("dictionary is not valid UTF-8: {error}"))
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // `str::lines` matches StreamReader.ReadLine for LF/CRLF and does not invent
    // an extra empty entry for a final line terminator. Entries are never trimmed.
    Ok(text.lines().map(str::to_owned).collect())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0F)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_assets_match_the_immutable_manifest() {
        let assets = PaddleAssets::load_source_tree().unwrap();
        assert_eq!(assets.model().len(), EXPECTED_MODEL_BYTES);
        assert_eq!(assets.dictionary_bytes().len(), EXPECTED_DICTIONARY_BYTES);
        assert_eq!(assets.model_sha256(), EXPECTED_MODEL_SHA256);
        assert_eq!(assets.dictionary_sha256(), EXPECTED_DICTIONARY_SHA256);
        assert_eq!(assets.dictionary().len(), EXPECTED_DICTIONARY_ENTRIES);
        assert_eq!(assets.dictionary()[0], EXPECTED_DICTIONARY_FIRST);
        assert_eq!(
            assets.dictionary()[EXPECTED_DICTIONARY_ENTRIES - 1],
            EXPECTED_DICTIONARY_LAST
        );
    }

    #[test]
    fn dictionary_parser_preserves_whitespace_and_crlf() {
        let parsed = parse_dictionary(b"\xEF\xBB\xBF\xE3\x80\x80\r\n \r\nlast\r\n").unwrap();
        assert_eq!(parsed, ["　", " ", "last"]);
    }
}
