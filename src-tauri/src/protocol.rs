use std::borrow::Cow;

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext};

use crate::state::AppState;

type ProtocolResponse = Response<Cow<'static, [u8]>>;

pub fn asset_response<R: Runtime>(ctx: UriSchemeContext<'_, R>, request: Request<Vec<u8>>) -> ProtocolResponse {
    file_response(ctx, request, FileKind::Asset)
}

pub fn template_response<R: Runtime>(ctx: UriSchemeContext<'_, R>, request: Request<Vec<u8>>) -> ProtocolResponse {
    file_response(ctx, request, FileKind::Template)
}

enum FileKind {
    Asset,
    Template,
}

fn file_response<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    kind: FileKind,
) -> ProtocolResponse {
    let id = request
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    if id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "missing resource id");
    }
    let state = ctx.app_handle().state::<AppState>();
    let resolved = match kind {
        FileKind::Asset => state
            .core()
            .asset_file(id)
            .map(|file| (file.path, file.mime_type)),
        FileKind::Template => state
            .core()
            .template_file(id)
            .map(|file| (file.path, file.mime_type)),
    };
    let Ok((path, mime_type)) = resolved else {
        return error_response(StatusCode::NOT_FOUND, "resource not found");
    };
    if !path.exists() {
        return error_response(StatusCode::NOT_FOUND, "resource file not found");
    }
    match std::fs::read(path) {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime_type)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
            .body(Cow::Owned(bytes))
            .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to read resource"),
    }
}

fn error_response(status: StatusCode, message: &'static str) -> ProtocolResponse {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Cow::Borrowed(message.as_bytes()))
        .unwrap()
}
