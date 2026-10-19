use std::fmt;

/// Errors returned by the engine. The library never panics on bad input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// A configuration or model parameter is out of range.
    InvalidConfig(&'static str),
    /// Caller-supplied data (a transaction or observation) is malformed.
    InvalidInput(&'static str),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::InvalidConfig(msg) => write!(f, "invalid config: {msg}"),
            EngineError::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
        }
    }
}

impl std::error::Error for EngineError {}
