//! Similar photos: background hashing and the commands behind the Similar page.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use photo_core::fileops::{self, Report};
use photo_core::similar::{self, Group, Overview};
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, CmdResult, blocking, scans};

/// Hashes photos that need it, then regroups, in the background. Does nothing while a
/// scan runs (scans stop it, and start it again when they finish) or if it's already
/// running.
pub fn run_in_background(app: &AppHandle, state: &Arc<AppState>) {
    if !scans::running(state).is_empty() || state.hashing.swap(true, Ordering::SeqCst) {
        return;
    }
    state.hash_cancel.store(false, Ordering::SeqCst);
    let (app, state) = (app.clone(), state.clone());
    std::thread::Builder::new()
        .name("similar".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let hashed = similar::hash_pending(&state.library, &state.thumbs, &state.hash_cancel)
                .and_then(|n| {
                    similar::detect(&state.library)?;
                    Ok(n)
                });
            match hashed {
                Ok(0) => {}
                Ok(n) => crate::diag::log(format!("{n} photos hashed in {:?}", started.elapsed())),
                Err(e) => crate::diag::log(format!("similar photos failed: {e}")),
            }
            // Groups can be shown now; then the slower assessment of their photos,
            // which refines the suggested best shots.
            let _ = app.emit("similar-changed", ());
            let started = std::time::Instant::now();
            match similar::assess_pending(&state.library, &state.hash_cancel) {
                Ok(0) => {}
                Ok(n) => {
                    crate::diag::log(format!("{n} shots assessed in {:?}", started.elapsed()));
                    let _ = app.emit("similar-changed", ());
                }
                Err(e) => crate::diag::log(format!("shot assessment failed: {e}")),
            }
            state.hashing.store(false, Ordering::SeqCst);
        })
        .expect("spawn similar thread");
}

#[tauri::command]
pub async fn similar_overview(state: State<'_, Arc<AppState>>) -> CmdResult<Overview> {
    blocking(&state, |s| similar::overview(&s.library.read())).await
}

#[tauri::command]
pub async fn similar_groups(
    state: State<'_, Arc<AppState>>,
    offset: usize,
    limit: usize,
) -> CmdResult<Vec<Group>> {
    blocking(&state, move |s| {
        similar::groups(&s.library.read(), offset, limit.min(200))
    })
    .await
}

/// Moves every file of these photos to the trash folder next to it (undoable).
#[tauri::command]
pub async fn remove_similar(state: State<'_, Arc<AppState>>, ids: Vec<i64>) -> CmdResult<Report> {
    if !scans::running(&state).is_empty() {
        return Err("Wait for the scan to finish first".into());
    }
    blocking(&state, move |s| {
        let files = similar::files_of_assets(&s.library.read(), &ids)?;
        let report = fileops::trash(&s.library, &files, "similar", false)?;
        crate::cleanup_commands::refresh_groupings(s)?;
        Ok(report)
    })
    .await
}

/// Keeps every photo of a group; it isn't shown again.
#[tauri::command]
pub async fn keep_similar(state: State<'_, Arc<AppState>>, ids: Vec<i64>) -> CmdResult<()> {
    blocking(&state, move |s| similar::dismiss(&s.library, &ids)).await
}
