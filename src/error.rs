use thiserror::Error;

#[derive(Error, Debug)]
pub enum McrawError {
    #[error("File error: {0}")]
    FileError(String),

    #[error("Decode error: {0}")]
    DecodeError(String),

    #[error("Invalid MCRAW format: {0}")]
    FormatError(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Encoder error: {0}")]
    EncoderError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Decoder not available")]
    DecoderNotAvailable,
}

pub type McrawResult<T> = std::result::Result<T, McrawError>;
