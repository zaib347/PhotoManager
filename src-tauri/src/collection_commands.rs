//! Commands for albums and tags.

use std::sync::Arc;

use photo_core::collections::{self, AlbumInfo, TagInfo};
use tauri::State;

use crate::{AppState, CmdResult, blocking};

#[tauri::command]
pub async fn list_albums(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<AlbumInfo>> {
    blocking(&state, |s| collections::list_albums(&s.library.read())).await
}

#[tauri::command]
pub async fn create_album(
    state: State<'_, Arc<AppState>>,
    name: String,
    ids: Vec<i64>,
) -> CmdResult<i64> {
    blocking(&state, move |s| {
        collections::create_album(&mut s.library.write(), &name, &ids)
    })
    .await
}

#[tauri::command]
pub async fn rename_album(state: State<'_, Arc<AppState>>, id: i64, name: String) -> CmdResult<()> {
    blocking(&state, move |s| {
        collections::rename_album(&s.library.write(), id, &name)
    })
    .await
}

#[tauri::command]
pub async fn delete_album(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<()> {
    blocking(&state, move |s| {
        collections::delete_album(&s.library.write(), id)
    })
    .await
}

#[tauri::command]
pub async fn add_to_album(
    state: State<'_, Arc<AppState>>,
    album_id: i64,
    ids: Vec<i64>,
) -> CmdResult<usize> {
    blocking(&state, move |s| {
        collections::add_to_album(&mut s.library.write(), album_id, &ids)
    })
    .await
}

#[tauri::command]
pub async fn remove_from_album(
    state: State<'_, Arc<AppState>>,
    album_id: i64,
    ids: Vec<i64>,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        collections::remove_from_album(&mut s.library.write(), album_id, &ids)
    })
    .await
}

#[tauri::command]
pub async fn set_album_cover(
    state: State<'_, Arc<AppState>>,
    album_id: i64,
    asset_id: i64,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        collections::set_album_cover(&s.library.write(), album_id, asset_id)
    })
    .await
}

#[tauri::command]
pub async fn list_tags(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<TagInfo>> {
    blocking(&state, |s| collections::list_tags(&s.library.read())).await
}

#[tauri::command]
pub async fn add_tag(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    name: String,
) -> CmdResult<i64> {
    blocking(&state, move |s| {
        collections::add_tag(&mut s.library.write(), &ids, &name)
    })
    .await
}

#[tauri::command]
pub async fn remove_tag(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    tag_id: i64,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        collections::remove_tag(&mut s.library.write(), &ids, tag_id)
    })
    .await
}

#[tauri::command]
pub async fn rename_tag(state: State<'_, Arc<AppState>>, id: i64, name: String) -> CmdResult<()> {
    blocking(&state, move |s| {
        collections::rename_tag(&s.library.write(), id, &name)
    })
    .await
}

#[tauri::command]
pub async fn delete_tag(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<()> {
    blocking(&state, move |s| {
        collections::delete_tag(&s.library.write(), id)
    })
    .await
}
