use std::path::Path;

use ort::session::{builder::SessionBuilder, Session};

use super::error::OcrError;

pub type BuilderFn = fn(SessionBuilder) -> Result<SessionBuilder, ort::Error>;

/// Loads a session from a controlled file path with caller supplied options.
/// Unlike the origin there is no default option set: the sidecar always
/// decides threads, arena and optimisation level explicitly.
pub fn load_session(path: &Path, configure: BuilderFn) -> Result<Session, OcrError> {
    let builder = configure(Session::builder()?)?;
    Ok(builder.commit_from_file(path)?)
}

pub fn first_input_name(session: &Session) -> Result<String, OcrError> {
    session
        .inputs
        .first()
        .map(|input| input.name.clone())
        .ok_or_else(|| OcrError::ModelOutput("模型没有输入".into()))
}
