//! Custom URI schemes that serve image bytes straight from Rust, bypassing IPC:
//! - `thumb://<asset_id>`: grid thumbnails
//! - `media://<asset_id>`: the original for the viewer (byte ranges for video seeking),
//!   or a decoded preview when the webview can't display the format itself

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use photo_core::library::{self, AssetSource};
use photo_core::{live, media, thumbs};
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::AppState;

/// Largest slice returned per range request, so huge videos never load into memory.
const MAX_CHUNK: u64 = 8 << 20;

/// Placeholders stand in while a file is offline or not decodable yet, so they must
/// not be cached by the webview; real thumbnails are immutable per asset.
enum Thumb {
    Cached(Vec<u8>),
    Temporary(Vec<u8>),
}

fn parse_request<R: Runtime>(
    ctx: &UriSchemeContext<'_, R>,
    request: &Request<Vec<u8>>,
) -> (Arc<AppState>, Option<i64>) {
    let state = ctx.app_handle().state::<Arc<AppState>>().inner().clone();
    let id = request.uri().path().trim_start_matches('/').parse().ok();
    (state, id)
}

pub fn handle_thumb<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let (state, Some(asset_id)) = parse_request(&ctx, &request) else {
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
                    responder.respond(match generate_thumb(&pool_state, asset_id) {
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

fn generate_thumb(state: &AppState, asset_id: i64) -> photo_core::Result<Option<Thumb>> {
    let Some(source) = library::asset_source(&state.library.read(), asset_id)? else {
        return Ok(None);
    };
    Ok(Some(match source {
        AssetSource::File {
            path,
            orientation,
            look,
            kind,
        } => {
            // Our own decoders first for photos (fast, exact); the OS for video frames
            // and for photo formats we can't read (HEIC/RAW with an installed codec).
            let generated = if state.os_thumb_failed(asset_id) {
                None
            } else if kind == 0 {
                thumbs::from_file(&path, orientation, &look)
                    .or_else(|_| thumbs::from_os(&path, &look))
                    .ok()
            } else {
                thumbs::from_os(&path, &Default::default()).ok()
            };
            match generated {
                Some(data) => {
                    thumbs::put(&state.thumbs.write(), asset_id, &data)?;
                    Thumb::Cached(data)
                }
                None => {
                    state.remember_os_thumb_failure(asset_id);
                    Thumb::Temporary(thumbs::render_placeholder(asset_id, kind == 1)?)
                }
            }
        }
        AssetSource::Unavailable { kind } => {
            Thumb::Temporary(thumbs::render_placeholder(asset_id, kind == 1)?)
        }
        AssetSource::NoFiles { kind } => {
            let data = thumbs::render_placeholder(asset_id, kind == 1)?;
            thumbs::put(&state.thumbs.write(), asset_id, &data)?;
            Thumb::Cached(data)
        }
    }))
}

pub fn handle_media<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let (state, Some(asset_id)) = parse_request(&ctx, &request) else {
        responder.respond(status(StatusCode::BAD_REQUEST));
        return;
    };
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    // media://<photo id>?live serves the clip of a Live Photo.
    let live = request
        .uri()
        .query()
        .is_some_and(|q| q.split('&').any(|p| p == "live"));

    tauri::async_runtime::spawn_blocking(move || {
        let asset_id = if live {
            match live::clip_of(&state.library.read(), asset_id) {
                Ok(Some(clip)) => clip,
                _ => {
                    responder.respond(status(StatusCode::NOT_FOUND));
                    return;
                }
            }
        } else {
            asset_id
        };
        let source = library::asset_source(&state.library.read(), asset_id);
        let Ok(Some(AssetSource::File {
            path,
            orientation,
            look,
            kind,
        })) = source
        else {
            responder.respond(status(StatusCode::NOT_FOUND));
            return;
        };
        let web = media::web_mime(&path);
        if let Some(mime) = web.filter(|_| look.is_plain() || kind == 1) {
            responder.respond(
                serve_file(&path, mime, range.as_deref())
                    .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR)),
            );
        } else if kind == 0 {
            state.decode_pool.spawn(move || {
                // A turned or edited photo the webview could show is decoded at full
                // size, so zooming in stays sharp.
                let preview = if web.is_some() {
                    thumbs::full_adjusted(&path, orientation, &look)
                } else {
                    thumbs::preview(&path, orientation, &look)
                }
                .or_else(|_| thumbs::preview_from_os(&path, &look));
                responder.respond(match preview {
                    Ok(data) => Response::builder()
                        .header(header::CONTENT_TYPE, "image/jpeg")
                        .header(header::CACHE_CONTROL, "no-store")
                        .body(data)
                        .expect("valid response"),
                    Err(_) => status(StatusCode::UNSUPPORTED_MEDIA_TYPE),
                });
            });
        } else {
            responder.respond(status(StatusCode::UNSUPPORTED_MEDIA_TYPE));
        }
    });
}

fn serve_file(path: &Path, mime: &str, range: Option<&str>) -> std::io::Result<Response<Vec<u8>>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let requested = range.and_then(|r| parse_range(r, len));
    // Webviews always send Range for media; cap unranged video reads anyway.
    let slice = requested
        .or_else(|| (mime.starts_with("video/") && len > MAX_CHUNK).then_some((0, len - 1)));

    let builder = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-cache");
    Ok(match slice {
        Some((start, end)) => {
            let end = end.min(start + MAX_CHUNK - 1);
            let mut data = vec![0; (end - start + 1) as usize];
            file.seek(SeekFrom::Start(start))?;
            file.read_exact(&mut data)?;
            builder
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
                .body(data)
        }
        None => {
            let mut data = Vec::with_capacity(len as usize);
            file.read_to_end(&mut data)?;
            builder.body(data)
        }
    }
    .expect("valid response"))
}

/// Parses a single `bytes=` range into inclusive (start, end) within `len`.
fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header
        .trim()
        .strip_prefix("bytes=")?
        .split(',')
        .next()?
        .trim();
    let (start, end) = spec.split_once('-')?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            (len.checked_sub(n.min(len))?, len.checked_sub(1)?)
        }
        (s, "") => (s.parse().ok()?, len.checked_sub(1)?),
        (s, e) => (
            s.parse().ok()?,
            e.parse::<u64>().ok()?.min(len.checked_sub(1)?),
        ),
    };
    (start <= end && start < len).then_some((start, end))
}

fn jpeg(thumb: Thumb) -> Response<Vec<u8>> {
    let (data, cache) = match thumb {
        Thumb::Cached(d) => (d, "max-age=31536000, immutable"),
        Thumb::Temporary(d) => (d, "no-store"),
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, cache)
        .body(data)
        .expect("valid response")
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .body(Vec::new())
        .expect("valid response")
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn byte_ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=900-5000", 1000), Some((900, 999)));
        assert_eq!(
            parse_range("bytes=1000-", 1000),
            None,
            "starts past the end"
        );
        assert_eq!(parse_range("bytes=50-10", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=0-", 0), None, "empty file");
    }
}
