//! Exporting copies of photos (resize, convert, compress) as a background job.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use photo_core::export::{self, Options, Report};
use photo_core::library::{self, AssetSource};
use photo_core::watermark::Watermark;
use serde::Serialize;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, CmdResult, blocking};

/// Long edge of the watermark preview.
const PREVIEW_EDGE: u32 = 1024;

#[derive(Serialize, Clone)]
struct ProgressEvent {
    done: usize,
    total: usize,
}

#[derive(Serialize, Clone)]
struct FinishedEvent {
    report: Option<Report>,
    error: Option<String>,
}

/// Starts exporting; progress arrives as `export-progress`, the result as
/// `export-finished`. One export at a time.
#[tauri::command]
pub async fn export_photos(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    options: Options,
) -> CmdResult<()> {
    if state.exporting.swap(true, Ordering::SeqCst) {
        return Err("An export is already running".into());
    }
    state.export_cancel.store(false, Ordering::SeqCst);
    let state = state.inner().clone();
    std::thread::Builder::new()
        .name("export".into())
        .spawn(move || {
            let result = export::run(
                &state.library,
                &ids,
                &options,
                &state.export_cancel,
                &|done, total| {
                    let _ = app.emit("export-progress", ProgressEvent { done, total });
                },
            );
            state.exporting.store(false, Ordering::SeqCst);
            let (report, error) = match result {
                Ok(r) => (Some(r), None),
                Err(e) => (None, Some(e.to_string())),
            };
            let _ = app.emit("export-finished", FinishedEvent { report, error });
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Stops after the photos already being written.
#[tauri::command]
pub async fn cancel_export(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.export_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// Photo `id` with `watermark` drawn on it, as a JPEG about 1024 px across. The
/// decoded photo is kept, so moving a slider only redraws the mark.
#[tauri::command]
pub async fn watermark_preview(
    state: State<'_, Arc<AppState>>,
    id: i64,
    watermark: Watermark,
) -> CmdResult<Response> {
    let jpeg = blocking(&state, move |s| {
        let cached = s
            .watermark_base
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .filter(|(cached, _)| *cached == id);
        let base = match cached {
            Some((_, base)) => base,
            None => {
                let path = match library::asset_source(&s.library.read(), id)? {
                    Some(AssetSource::File { path, kind, .. }) if kind != 1 => path,
                    _ => {
                        return Err(photo_core::Error::Invalid(
                            "No photo file to preview on".into(),
                        ));
                    }
                };
                let base = Arc::new(export::preview_base(&path, PREVIEW_EDGE)?);
                *s.watermark_base.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some((id, base.clone()));
                base
            }
        };
        export::preview(&base, &watermark)
    })
    .await?;
    Ok(Response::new(jpeg))
}
