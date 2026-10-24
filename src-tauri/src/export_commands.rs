//! Exporting copies of photos (resize, convert, compress) as a background job.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use photo_core::export::{self, Options, PreviewBase, Report};
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
        let base = cached_base(s, &s.watermark_base, id, PREVIEW_EDGE, true)?;
        export::preview(&base, &watermark)
    })
    .await?;
    Ok(Response::new(jpeg))
}

/// Photo `id` decoded upright with its turns (and its edit when `with_edit`), shrunk
/// to `edge`. Kept in `slot` so the next call for the same photo is instant.
pub(crate) fn cached_base(
    s: &AppState,
    slot: &Mutex<Option<(i64, Arc<PreviewBase>)>>,
    id: i64,
    edge: u32,
    with_edit: bool,
) -> photo_core::Result<Arc<PreviewBase>> {
    let cached = slot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .filter(|(cached, _)| *cached == id);
    if let Some((_, base)) = cached {
        return Ok(base);
    }
    let (path, mut look) = match library::asset_source(&s.library.read(), id)? {
        Some(AssetSource::File {
            path, look, kind, ..
        }) if kind != 1 => (path, look),
        _ => {
            return Err(photo_core::Error::Invalid(
                "No photo file to preview on".into(),
            ));
        }
    };
    if !with_edit {
        look.edit = None;
    }
    let base = Arc::new(export::preview_base(&path, &look, edge)?);
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, base.clone()));
    Ok(base)
}

/// Renders a collage at full size and saves it as a JPEG at `path` (chosen by the
/// user in a save dialog, which already asked about replacing a file).
#[tauri::command]
pub async fn save_collage(
    state: State<'_, Arc<AppState>>,
    collage: photo_core::collage::Collage,
    path: std::path::PathBuf,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        photo_core::collage::save(&s.library, &collage, &path)
    })
    .await
}
