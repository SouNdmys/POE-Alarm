use std::fmt;
use std::io;
use std::path::PathBuf;

/// Failure returned by the offline Paddle recognition backend.
#[derive(Debug)]
pub enum PaddleError {
    Io { path: PathBuf, source: io::Error },
    InvalidAsset(String),
    InvalidConfiguration(String),
    InvalidImage(String),
    RuntimeLoad(String),
    Onnx(ort::Error),
    ModelContract(String),
    WorkerStart(String),
    WorkerUnavailable,
}

impl PaddleError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for PaddleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "could not read '{}': {source}", path.display())
            }
            Self::InvalidAsset(message) => write!(formatter, "invalid PP-OCRv5 asset: {message}"),
            Self::InvalidConfiguration(message) => {
                write!(formatter, "invalid Paddle OCR configuration: {message}")
            }
            Self::InvalidImage(message) => write!(formatter, "invalid OCR image: {message}"),
            Self::RuntimeLoad(message) => {
                write!(formatter, "could not load ONNX Runtime: {message}")
            }
            Self::Onnx(source) => write!(formatter, "ONNX Runtime failed: {source}"),
            Self::ModelContract(message) => {
                write!(formatter, "PP-OCRv5 model contract mismatch: {message}")
            }
            Self::WorkerStart(message) => {
                write!(formatter, "could not start Paddle OCR worker: {message}")
            }
            Self::WorkerUnavailable => formatter.write_str("the Paddle OCR worker is unavailable"),
        }
    }
}

impl std::error::Error for PaddleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Onnx(source) => Some(source),
            _ => None,
        }
    }
}

impl From<ort::Error> for PaddleError {
    fn from(source: ort::Error) -> Self {
        Self::Onnx(source)
    }
}

impl From<ort::Error<ort::session::builder::SessionBuilder>> for PaddleError {
    fn from(source: ort::Error<ort::session::builder::SessionBuilder>) -> Self {
        Self::Onnx(source.into())
    }
}
