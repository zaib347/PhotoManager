//! Importing Google Takeout and iCloud metadata into a library folder, and undoing it;
//! and taking an export straight from its zips: unzip, add the folder, scan, import.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use photo_core::imports::{self, ImportInfo, Report};
use photo_core::{library, unzip};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::cleanup_commands::no_scan_running;
use crate::{AppState, CmdResult, blocking, scans};

const SCAN_UNFINISHED: &str =
    "The scan didn't finish, so nothing was imported. Rescan the folder, then use Import… on it.";

/// The end of unzipping. `root_id` is the library folder now being scanned, when the
/// export was unzipped in full.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UnzipFinished {
    root_id: Option<i64>,
    summary: Option<unzip::Summary>,
    error: Option<String>,
}

/// The end of the import that follows the scan of an unzipped export.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ZipImportFinished {
    root_id: i64,
    report: Option<Report>,
    error: Option<String>,
}

/// Unzips a Google Takeout or iCloud export's `zips` into the folder `dest`, adds it to
/// the library (or uses it, when it is a library folder already), scans it and imports
/// the export's metadata. Runs in the background: watch `unzip-progress`,
/// `unzip-finished`, the folder's scan events, then `zip-import-finished`.
#[tauri::command]
pub async fn import_export_zips(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    zips: Vec<PathBuf>,
    dest: PathBuf,
) -> CmdResult<()> {
    if zips.is_empty() {
        return Err("Choose the export's zip files".into());
    }
    if !dest.is_dir() {
        return Err(format!("{} is not a folder", dest.display()));
    }
    // Refuse a folder inside or around a library folder before unzipping anything.
    let check = dest.clone();
    let existing = blocking(&state, move |s| library::root_at(&s.library.read(), &check)).await?;
    if let Some(root_id) = existing
        && scans::running(&state).contains(&root_id)
    {
        return Err("That folder is being scanned; try again when the scan ends".into());
    }
    if state.unzipping.swap(true, Ordering::SeqCst) {
        return Err("An export is already being unzipped".into());
    }
    state.unzip_cancel.store(false, Ordering::SeqCst);

    let worker_failed = state.inner().clone();
    let state = state.inner().clone();
    std::thread::Builder::new()
        .name("unzip".into())
        .spawn(move || {
            let result = unzip::extract(&zips, &dest, &state.unzip_cancel, &|p| {
                let _ = app.emit("unzip-progress", p);
            });
            state.unzipping.store(false, Ordering::SeqCst);
            let finish = |root_id, summary, error| {
                let _ = app.emit(
                    "unzip-finished",
                    UnzipFinished {
                        root_id,
                        summary,
                        error,
                    },
                );
            };
            let summary = match result {
                Ok(s) if s.completed => s,
                Ok(s) => return finish(None, Some(s), None),
                Err(e) => return finish(None, None, Some(e.to_string())),
            };
            let root_id = match existing {
                Some(id) => Ok(id),
                None => library::add_root(&state.library.write(), &dest),
            };
            let root_id = match root_id {
                Ok(id) => id,
                Err(e) => return finish(None, Some(summary), Some(e.to_string())),
            };
            finish(Some(root_id), Some(summary), None);
            let imported = move |app: &AppHandle, report: Option<Report>, error: Option<String>| {
                let _ = app.emit(
                    "zip-import-finished",
                    ZipImportFinished {
                        root_id,
                        report,
                        error,
                    },
                );
            };
            let started = scans::start_then(&app, &state, root_id, move |app, state, scanned| {
                if scanned.is_none() {
                    return imported(app, None, Some(SCAN_UNFINISHED.into()));
                }
                match imports::run(&state.library, root_id) {
                    Ok(r) => {
                        *state.map_points.lock().unwrap_or_else(|e| e.into_inner()) = None;
                        imported(app, Some(r), None);
                    }
                    Err(e) => imported(app, None, Some(e.to_string())),
                }
            });
            if !started {
                imported(&app, None, Some(SCAN_UNFINISHED.into()));
            }
        })
        .map_err(|e| {
            worker_failed.unzipping.store(false, Ordering::SeqCst);
            e.to_string()
        })?;
    Ok(())
}

/// Stops unzipping; files already unzipped stay, and a second try skips them.
#[tauri::command]
pub async fn cancel_unzip(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.unzip_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

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
