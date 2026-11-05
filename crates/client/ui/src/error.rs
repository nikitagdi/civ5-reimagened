//! Errors for the `ui` crate.

/// UI framework error.
#[derive(Debug, thiserror::Error)]
pub enum UiError {
    /// Lua failure (script load, execution, API registration).
    #[error("Lua error: {0}")]
    Lua(String),
    /// A file is not in the UI file set.
    #[error("file {0:?} not found in the UI file set")]
    FileNotFound(String),
    /// Invalid XML in a context file.
    #[error(transparent)]
    Xml(#[from] crate::xml::XmlError),
    /// Asset layer error.
    #[error(transparent)]
    Asset(#[from] assets::AssetError),
}
