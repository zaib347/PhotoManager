//! Commands for events: automatic groups of photos taken close together.

use std::sync::Arc;

use photo_core::events::{self, EventSummary};
use photo_core::places;
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, CmdResult, blocking};

/// Looks up places and groups events once for an existing library, after upgrading to
/// a version with them. Later scans keep both up to date themselves.
pub fn catch_up(app: &AppHandle, state: &Arc<AppState>) {
    let (app, state) = (app.clone(), state.clone());
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut needed = matches!(events::needs_detection(&state.library.read()), Ok(true));
        if matches!(places::needs_assign(&state.library.read()), Ok(true)) {
            match places::assign(&state.library) {
                Ok(n) => {
                    crate::diag::log(format!("{n} places looked up in {:?}", started.elapsed()));
                    // Events take their default names from places.
                    needed = true;
                }
                Err(e) => crate::diag::log(format!("place lookup failed: {e}")),
            }
        }
        if !needed {
            return;
        }
        let started = std::time::Instant::now();
        match events::detect(&mut state.library.write()) {
            Ok(()) => {
                crate::diag::log(format!("events detected in {:?}", started.elapsed()));
                let _ = app.emit("events-changed", ());
            }
            Err(e) => crate::diag::log(format!("event detection failed: {e}")),
        }
    });
}

#[tauri::command]
pub async fn list_events(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<EventSummary>> {
    blocking(&state, move |s| events::list(&s.library.read())).await
}

#[tauri::command]
pub async fn get_event(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> CmdResult<Option<EventSummary>> {
    blocking(&state, move |s| events::get(&s.library.read(), id)).await
}

#[tauri::command]
pub async fn rename_event(
    state: State<'_, Arc<AppState>>,
    id: i64,
    title: String,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        events::rename(&mut s.library.write(), id, &title)
    })
    .await
}

#[tauri::command]
pub async fn merge_events(state: State<'_, Arc<AppState>>, ids: Vec<i64>) -> CmdResult<i64> {
    blocking(&state, move |s| events::merge(&mut s.library.write(), &ids)).await
}

#[tauri::command]
pub async fn split_event(
    state: State<'_, Arc<AppState>>,
    id: i64,
    at_asset: i64,
) -> CmdResult<i64> {
    blocking(&state, move |s| {
        events::split(&mut s.library.write(), id, at_asset)
    })
    .await
}

#[tauri::command]
pub async fn create_event(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    title: String,
) -> CmdResult<i64> {
    blocking(&state, move |s| {
        events::create(&mut s.library.write(), &ids, &title)
    })
    .await
}

#[tauri::command]
pub async fn remove_event(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<()> {
    blocking(&state, move |s| events::remove(&mut s.library.write(), id)).await
}
