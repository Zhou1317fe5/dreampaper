use std::borrow::Cow;

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::core::{thumb, Core};
use crate::state::AppState;

type ProtocolResponse = Response<Cow<'static, [u8]>>;

pub fn asset_response<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    file_response(ctx, request, FileKind::Asset, responder);
}

pub fn template_response<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    file_response(ctx, request, FileKind::Template, responder);
}

#[derive(Clone, Copy)]
enum FileKind {
    Asset,
    Template,
}

fn file_response<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    kind: FileKind,
    responder: UriSchemeResponder,
) {
    let id = request
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    if id.is_empty() {
        responder.respond(error_response(StatusCode::BAD_REQUEST, "missing resource id"));
        return;
    }
    let width = thumb::requested_width(request.uri().query());
    let core = ctx.app_handle().state::<AppState>().core_arc();

    // Nothing below belongs on the webview's calling thread: resolving the id
    // opens a fresh SQLite connection, and reading — or rescaling — the file is
    // slower still. Switching a history filter remounts a gridful of <img> at
    // once, and paying that burst inline is what made the page stutter.
    std::thread::spawn(move || {
        responder.respond(serve(&core, &id, kind, width));
    });
}

fn serve(core: &Core, id: &str, kind: FileKind, width: Option<u32>) -> ProtocolResponse {
    let resolved = match kind {
        FileKind::Asset => core.asset_file(id).map(|file| (file.path, file.mime_type)),
        FileKind::Template => core.template_file(id).map(|file| (file.path, file.mime_type)),
    };
    let Ok((path, mime_type)) = resolved else {
        return error_response(StatusCode::NOT_FOUND, "resource not found");
    };
    let (path, mime_type) = match width {
        Some(width) => match thumb::scaled(&core.app_data, id, &path, width) {
            (scaled, true) => (scaled, "image/jpeg".to_string()),
            (original, false) => (original, mime_type),
        },
        None => (path, mime_type),
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
            .unwrap_or_else(|_| {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build failed")
            }),
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
