//! Importing Google Takeout and iCloud metadata into a library folder, and undoing it.

use std::sync::Arc;

use photo_core::imports::{self, ImportInfo, Report};
use tauri::State;

use crate::cleanup_commands::no_scan_running;
use crate::{AppState, CmdResult, blocking};

/// Reads the export metadata in a library folder. Refused while a scan runs, since
/// it matches metadata to the files a scan indexes.
#[tauri::command]
pub async fn import_metadata(state: State<'_, Arc<AppState>>, root_id: i64) -> CmdResult<Report> {
    no_scan_running(&state)?;
    blocking(&state, move |s| {
        let report = imports::run(&s.library, root_id)?;
        *s.map_points.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(report)
    })
    .await
}

/// Imports into a folder, newest first.
#[tauri::command]
pub async fn list_imports(
    state: State<'_, Arc<AppState>>,
    root_id: i64,
) -> CmdResult<Vec<ImportInfo>> {
    blocking(&state, move |s| imports::list(&s.library.read(), root_id)).await
}

/// Takes back an import's changes. Returns how many were undone.
#[tauri::command]
pub async fn undo_import(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<usize> {
    blocking(&state, move |s| {
        let n = imports::undo(&s.library, id)?;
        *s.map_points.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(n)
    })
    .await
}
