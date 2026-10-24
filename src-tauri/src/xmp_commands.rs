//! XMP sidecars: settings, photo descriptions, and the background sidecar writer.

use std::sync::Arc;
use std::time::Duration;

use photo_core::xmp::{self, Settings};
use tauri::State;

use crate::{AppState, CmdResult, blocking};

/// How often the writer looks for queued changes.
const WRITE_POLL: Duration = Duration::from_secs(3);

#[tauri::command]
pub async fn xmp_settings(state: State<'_, Arc<AppState>>) -> CmdResult<Settings> {
    blocking(&state, |s| xmp::settings(&s.library.read())).await
}

/// Turning writing on queues every photo with a rating, favorite, tag or description.
#[tauri::command]
pub async fn set_xmp_settings(
    state: State<'_, Arc<AppState>>,
    settings: Settings,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        xmp::set_settings(&mut s.library.write(), settings)
    })
    .await
}

/// Sets a photo's description; empty clears it.
#[tauri::command]
pub async fn set_description(
    state: State<'_, Arc<AppState>>,
    id: i64,
    text: String,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        xmp::set_description(&s.library.write(), id, Some(&text))
    })
    .await
}

/// Writes queued sidecars in the background while writing is turned on.
pub fn run_writer(state: &Arc<AppState>) {
    let state = state.clone();
    std::thread::Builder::new()
        .name("xmp-writer".into())
        .spawn(move || {
            loop {
                std::thread::sleep(WRITE_POLL);
                match xmp::write_pending(&state.library, 200) {
                    Ok(r) if r.failed > 0 => crate::diag::log(format!(
                        "XMP sidecars: {} written, {} could not be written",
                        r.written, r.failed
                    )),
                    Ok(_) => {}
                    Err(e) => crate::diag::log(format!("XMP sidecars failed: {e}")),
                }
            }
        })
        .expect("spawn xmp writer thread");
}
