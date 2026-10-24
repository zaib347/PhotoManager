//! The photo editor: live previews, auto-enhance, and saving or reverting an edit.
//! Edits are recipes in the library; files are never rewritten.

use std::sync::Arc;

use photo_core::edit::{self, PasteWhat, Recipe};
use tauri::State;
use tauri::ipc::Response;

use crate::export_commands::cached_base;
use crate::{AppState, CmdResult, blocking};

/// Long edge of the editor's working image.
const EDITOR_EDGE: u32 = 1600;

/// The saved edit of a photo, if any.
#[tauri::command]
pub async fn edit_get(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<Option<Recipe>> {
    blocking(&state, move |s| edit::get(&s.library.read(), id)).await
}

/// Photo `id` with `recipe` applied, as a JPEG about 1600 px across. The photo is
/// decoded once (turned, unedited) and kept while the sliders move.
#[tauri::command]
pub async fn edit_preview(
    state: State<'_, Arc<AppState>>,
    id: i64,
    recipe: Recipe,
) -> CmdResult<Response> {
    let jpeg = blocking(&state, move |s| {
        let base = cached_base(s, &s.edit_base, id, EDITOR_EDGE, false)?;
        edit::preview_jpeg(base.image(), &recipe)
    })
    .await?;
    Ok(Response::new(jpeg))
}

/// Light and colour settings suggested for photo `id`; crop and straightening are
/// kept from `recipe`.
#[tauri::command]
pub async fn edit_auto(
    state: State<'_, Arc<AppState>>,
    id: i64,
    recipe: Recipe,
) -> CmdResult<Recipe> {
    blocking(&state, move |s| {
        let base = cached_base(s, &s.edit_base, id, EDITOR_EDGE, false)?;
        Ok(edit::auto_enhance(base.image(), &recipe))
    })
    .await
}

/// Saves an edit, or with `None` reverts to the original. The thumbnail is made
/// again from the file on its next request. Returns the new edit version.
#[tauri::command]
pub async fn edit_save(
    state: State<'_, Arc<AppState>>,
    id: i64,
    recipe: Option<Recipe>,
) -> CmdResult<i64> {
    blocking(&state, move |s| {
        let version = edit::save(&s.library.write(), id, recipe.as_ref())?;
        photo_core::thumbs::delete(&s.thumbs.write(), id)?;
        // The watermark preview shows the edit; decode it again next time.
        *s.watermark_base.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(version)
    })
    .await
}

/// Pastes a copied edit onto photos `ids` (videos are skipped), replacing their edits.
/// Their thumbnails are made again on their next request. Returns (id, new edit
/// version) for each photo pasted onto.
#[tauri::command]
pub async fn edit_paste(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    recipe: Recipe,
    what: PasteWhat,
) -> CmdResult<Vec<(i64, i64)>> {
    blocking(&state, move |s| {
        let pasted = edit::paste(&mut s.library.write(), &ids, &recipe, what)?;
        let mut thumbs = s.thumbs.write();
        let tx = thumbs.transaction()?;
        for &(id, _) in &pasted {
            photo_core::thumbs::delete(&tx, id)?;
        }
        tx.commit()?;
        *s.watermark_base.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(pasted)
    })
    .await
}
