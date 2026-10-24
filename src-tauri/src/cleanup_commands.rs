//! Commands for cleanup: removing photos and duplicate copies, and the operations
//! that removed files.

use std::collections::HashMap;
use std::sync::Arc;

use photo_core::duplicates::{self, Group, Overview};
use photo_core::fileops::{self, OpSummary, Report};
use photo_core::organize::{self, Layout, Preview};
use photo_core::{events, similar, trips};
use tauri::State;

use crate::{AppState, CmdResult, blocking, scans};

/// Operations shown under "Recently removed".
const RECENT_OPS: usize = 50;

/// Scans write file rows while they walk, so files never move during one.
pub(crate) fn no_scan_running(state: &AppState) -> CmdResult<()> {
    if scans::running(state).is_empty() {
        Ok(())
    } else {
        Err("Wait for the scan to finish first".into())
    }
}

/// After photos left the library or came back: events, trips and similar groups
/// follow (a few seconds at a million photos).
pub(crate) fn refresh_groupings(s: &AppState) -> photo_core::Result<()> {
    events::detect(&mut s.library.write())?;
    trips::detect(&mut s.library.write())?;
    similar::detect(&s.library)
}

/// Moves every file of these photos to the trash folder next to it (undoable from
/// Recently removed).
#[tauri::command]
pub async fn remove_photos(state: State<'_, Arc<AppState>>, ids: Vec<i64>) -> CmdResult<Report> {
    no_scan_running(&state)?;
    blocking(&state, move |s| {
        let files = similar::files_of_assets(&s.library.read(), &ids)?;
        let report = fileops::trash(&s.library, &files, "removed", false)?;
        refresh_groupings(s)?;
        Ok(report)
    })
    .await
}

#[tauri::command]
pub async fn duplicate_overview(state: State<'_, Arc<AppState>>) -> CmdResult<Overview> {
    blocking(&state, |s| duplicates::overview(&s.library.read())).await
}

#[tauri::command]
pub async fn duplicate_groups(
    state: State<'_, Arc<AppState>>,
    offset: usize,
    limit: usize,
) -> CmdResult<Vec<Group>> {
    blocking(&state, move |s| {
        duplicates::groups(&s.library.read(), offset, limit.min(500))
    })
    .await
}

/// Moves the extra copies of duplicated photos to the trash folder next to them.
/// `keep` overrides the suggested copy (asset id → file id); `only` limits it to some
/// photos. One copy of every photo always stays.
#[tauri::command]
pub async fn remove_duplicates(
    state: State<'_, Arc<AppState>>,
    only: Option<Vec<i64>>,
    keep: HashMap<i64, i64>,
) -> CmdResult<Report> {
    no_scan_running(&state)?;
    blocking(&state, move |s| {
        let extras = duplicates::extras(&s.library.read(), only.as_deref(), &keep)?;
        fileops::trash(&s.library, &extras, "duplicates", true)
    })
    .await
}

/// Recent operations: removals (`"trash"`, the default) or organizer moves (`"move"`).
#[tauri::command]
pub async fn list_file_ops(
    state: State<'_, Arc<AppState>>,
    kind: Option<String>,
) -> CmdResult<Vec<OpSummary>> {
    let kind = match kind.as_deref() {
        Some("move") => fileops::Kind::Move,
        _ => fileops::Kind::Trash,
    };
    blocking(&state, move |s| {
        fileops::list(&s.library.read(), kind, RECENT_OPS)
    })
    .await
}

/// What organizing a library folder would move, without moving anything.
#[tauri::command]
pub async fn organize_preview(
    state: State<'_, Arc<AppState>>,
    root_id: i64,
    layout: Layout,
    include_undated: bool,
) -> CmdResult<Preview> {
    blocking(&state, move |s| {
        organize::preview(&s.library.read(), root_id, layout, include_undated)
    })
    .await
}

/// Moves a library folder's files into folders by date (undoable).
#[tauri::command]
pub async fn organize_run(
    state: State<'_, Arc<AppState>>,
    root_id: i64,
    layout: Layout,
    include_undated: bool,
) -> CmdResult<Report> {
    no_scan_running(&state)?;
    blocking(&state, move |s| {
        organize::run(&s.library, root_id, layout, include_undated)
    })
    .await
}

#[tauri::command]
pub async fn undo_file_op(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<Report> {
    no_scan_running(&state)?;
    blocking(&state, move |s| {
        let report = fileops::undo(&s.library, id)?;
        refresh_groupings(s)?;
        Ok(report)
    })
    .await
}

/// Sends an operation's files on to the Recycle Bin / Trash, where the drive has one.
#[tauri::command]
pub async fn empty_file_op(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<Report> {
    blocking(&state, move |s| fileops::empty(&s.library, id)).await
}
