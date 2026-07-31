use serde::Serialize;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub detail: Option<serde_json::Value>,
}

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(
        code: impl Into<String>,
        message: impl Into<String>,
        detail: serde_json::Value,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: Some(detail),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::new("io_error", error.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(error: rusqlite::Error) -> Self {
        Self::new("sqlite_error", error.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(error: serde_json::Error) -> Self {
        Self::new("json_error", error.to_string())
    }
}
