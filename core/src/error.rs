//! Error type shared by the conversion engine and its callers.

use std::path::PathBuf;

/// Result alias used across WorldPort.
pub type Result<T> = std::result::Result<T, WorldPortError>;

/// Errors that abort a conversion.
#[derive(Debug, thiserror::Error)]
pub enum WorldPortError {
    /// Filesystem failure with the path that caused it.
    #[error("io error at {}: {source}", path.display())]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying operating-system error.
        source: std::io::Error,
    },
    /// Malformed NBT payload.
    #[error("nbt error: {0}")]
    Nbt(String),
    /// Valid input that the current engine cannot represent.
    #[error("unsupported world data: {0}")]
    Unsupported(String),
    /// Input that does not look like the edition it claims to be.
    #[error("invalid world: {0}")]
    InvalidWorld(String),
    /// The caller asked to stop.
    #[error("conversion cancelled")]
    Cancelled,
    /// Anything else, with context.
    #[error("{0}")]
    Other(String),
}

impl WorldPortError {
    /// Wraps an IO error with its path.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Builds an NBT error.
    pub fn nbt(message: impl Into<String>) -> Self {
        Self::Nbt(message.into())
    }

    /// Builds an unsupported-data error.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }

    /// Builds an invalid-world error.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidWorld(message.into())
    }

    /// Builds a free-form error.
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    /// Returns true when the error only means "the caller cancelled".
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

impl From<bedrock_nbt::Error> for WorldPortError {
    fn from(value: bedrock_nbt::Error) -> Self {
        Self::Nbt(value.to_string())
    }
}

impl From<bedrock_world::BedrockWorldError> for WorldPortError {
    fn from(value: bedrock_world::BedrockWorldError) -> Self {
        Self::Other(format!("bedrock world: {value}"))
    }
}

/// Maps a `std::io::Result` into the engine result with path context.
pub fn io_result<T>(path: impl Into<PathBuf>, value: std::io::Result<T>) -> Result<T> {
    value.map_err(|source| WorldPortError::io(path, source))
}
