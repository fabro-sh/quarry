#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("Automerge: {0}")]
    Automerge(#[from] automerge::AutomergeError),
    #[error("Invalid document record: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Unknown {kind}: {id}")]
    NotFound { kind: &'static str, id: String },
    #[error("{kind} already exists: {id}")]
    AlreadyExists { kind: &'static str, id: String },
    #[error("Invalid document: {0}")]
    Invalid(String),
    #[error("Invalid UTF-16 offset: {0}")]
    InvalidOffset(usize),
    #[error("The target changed: {0}")]
    Conflict(String),
}

pub type Result<T> = std::result::Result<T, DocumentError>;
