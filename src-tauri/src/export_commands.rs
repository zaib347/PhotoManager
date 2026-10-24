//! Exporting copies of photos (resize, convert, compress) as a background job.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use photo_core::export::{self, Options, Report};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, CmdResult};

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
