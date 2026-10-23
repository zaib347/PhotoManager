//! `thumb://<asset_id>`: thumbnails served straight from Rust, bypassing IPC.

use std::sync::Arc;

use photo_core::library::{self, AssetSource};
use photo_core::thumbs;
use tauri::http::{Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::AppState;

/// Placeholders stand in while a file is offline or not decodable yet, so they must
/// not be cached by the webview; real thumbnails are immutable per asset.
enum Thumb {
    Cached(Vec<u8>),
    Temporary(Vec<u8>),
}

pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let state = ctx.app_handle().state::<Arc<AppState>>().inner().clone();
    let Ok(asset_id) = request.uri().path().trim_start_matches('/').parse::<i64>() else {
        responder.respond(status(StatusCode::BAD_REQUEST));
        return;
    };
    // Cache hits are cheap and answered immediately; decodes go to a bounded pool so
    // fast scrolling can't start hundreds of full-resolution decodes at once.
    tauri::async_runtime::spawn_blocking(move || {
        match thumbs::get(&state.thumbs.read(), asset_id) {
            Ok(Some(data)) => responder.respond(jpeg(Thumb::Cached(data))),
            Ok(None) => {
                let pool_state = state.clone();
                state.decode_pool.spawn(move || {
                    responder.respond(match generate(&pool_state, asset_id) {
                        Ok(Some(thumb)) => jpeg(thumb),
                        Ok(None) => status(StatusCode::NOT_FOUND),
                        Err(_) => status(StatusCode::INTERNAL_SERVER_ERROR),
                    });
                });
            }
            Err(_) => responder.respond(status(StatusCode::INTERNAL_SERVER_ERROR)),
        }
    });
}

fn generate(state: &AppState, asset_id: i64) -> photo_core::Result<Option<Thumb>> {
    let Some(source) = library::asset_source(&state.library.read(), asset_id)? else {
        return Ok(None);
    };
    Ok(Some(match source {
        AssetSource::File {
            path,
            orientation,
            kind: 0,
        } => match thumbs::from_file(&path, orientation) {
            Ok(data) => {
                thumbs::put(&state.thumbs.write(), asset_id, &data)?;
                Thumb::Cached(data)
            }
            // HEIC/RAW decoders arrive in M5; video poster frames in M2.
            Err(_) => Thumb::Temporary(thumbs::render_placeholder(asset_id, false)?),
        },
        AssetSource::File { kind, .. } | AssetSource::Unavailable { kind } => {
            Thumb::Temporary(thumbs::render_placeholder(asset_id, kind == 1)?)
        }
        AssetSource::NoFiles { kind } => {
            let data = thumbs::render_placeholder(asset_id, kind == 1)?;
            thumbs::put(&state.thumbs.write(), asset_id, &data)?;
            Thumb::Cached(data)
        }
    }))
}

fn jpeg(thumb: Thumb) -> Response<Vec<u8>> {
    let (data, cache) = match thumb {
        Thumb::Cached(d) => (d, "max-age=31536000, immutable"),
        Thumb::Temporary(d) => (d, "no-store"),
    };
    Response::builder()
        .header("Content-Type", "image/jpeg")
        .header("Cache-Control", cache)
        .body(data)
        .expect("valid response")
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .body(Vec::new())
        .expect("valid response")
}
