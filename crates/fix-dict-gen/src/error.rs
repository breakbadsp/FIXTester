use std::fmt;

/// A generator failure. The generator stops at the first one.
#[derive(Debug)]
pub enum GenError {
    Usage(String),
    Io {
        context: String,
        source: std::io::Error,
    },
    Xml(String),
    /// The repository data breaks a rule that the generator relies on.
    Data(String),
    Rustfmt(String),
}

impl fmt::Display for GenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(text) => write!(f, "usage error: {text}"),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::Xml(text) => write!(f, "XML error: {text}"),
            Self::Data(text) => write!(f, "repository data error: {text}"),
            Self::Rustfmt(text) => write!(f, "rustfmt error: {text}"),
        }
    }
}

impl std::error::Error for GenError {}

impl GenError {
    pub fn data(text: impl Into<String>) -> Self {
        Self::Data(text.into())
    }
}
