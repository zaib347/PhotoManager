//! Commands for memories and date corrections.

use std::sync::Arc;

use photo_core::dates::{self, WallRange};
use photo_core::memories::{self, DayMemory};
use photo_core::timeline::{self, Filter, View};
use tauri::State;

use crate::{AppState, CmdResult, blocking};

#[tauri::command]
pub async fn on_this_day(state: State<'_, Arc<AppState>>, today: i64) -> CmdResult<Vec<DayMemory>> {
    blocking(&state, move |s| {
        memories::on_this_day(&s.library.read(), today)
    })
    .await
}

#[tauri::command]
pub async fn date_range(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
) -> CmdResult<Option<WallRange>> {
    blocking(&state, move |s| dates::wall_range(&s.library.read(), &ids)).await
}

#[tauri::command]
pub async fn shift_dates(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    delta_ms: i64,
) -> CmdResult<usize> {
    blocking(&state, move |s| {
        dates::shift(&mut s.library.write(), &ids, delta_ms)
    })
    .await
}

#[tauri::command]
pub async fn restore_dates(state: State<'_, Arc<AppState>>, ids: Vec<i64>) -> CmdResult<usize> {
    blocking(&state, move |s| {
        dates::restore(&mut s.library.write(), &ids)
    })
    .await
}

#[tauri::command]
pub async fn asset_ids(
    state: State<'_, Arc<AppState>>,
    view: Option<View>,
    filter: Option<Filter>,
) -> CmdResult<Vec<i64>> {
    blocking(&state, move |s| {
        timeline::asset_ids(
            &s.library.read(),
            view.unwrap_or_default(),
            &filter.unwrap_or_default(),
        )
    })
    .await
}
