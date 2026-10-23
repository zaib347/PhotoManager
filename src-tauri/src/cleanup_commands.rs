//! Commands for cleanup: duplicate copies and the operations that removed files.

use std::collections::HashMap;
use std::sync::Arc;

use photo_core::duplicates::{self, Group, Overview};
use photo_core::fileops::{self, OpSummary, Report};
use tauri::State;

use crate::{AppState, CmdResult, blocking, scans};

/// Operations shown under "Recently removed".
const RECENT_OPS: usize = 50;

/// Scans write file rows while they walk, so files never move during one.
fn no_scan_running(state: &AppState) -> CmdResult<()> {
    if scans::running(state).is_empty() {
        Ok(())
    } else {
        Err("Wait for the scan to finish first".into())
    }
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

#[tauri::command]
pub async fn list_file_ops(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<OpSummary>> {
    blocking(&state, |s| fileops::list(&s.library.read(), RECENT_OPS)).await
}

#[tauri::command]
pub async fn undo_file_op(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<Report> {
    no_scan_running(&state)?;
    blocking(&state, move |s| fileops::undo(&s.library, id)).await
}

/// Sends an operation's files on to the Recycle Bin / Trash, where the drive has one.
#[tauri::command]
pub async fn empty_file_op(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<Report> {
    blocking(&state, move |s| fileops::empty(&s.library, id)).await
}
