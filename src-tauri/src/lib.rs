mod protocol;
mod scans;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use photo_core::db::Db;
use photo_core::{library, schema, synth, thumbs, timeline};
use tauri::{AppHandle, Emitter, Manager, State};

const VOLUME_POLL: Duration = Duration::from_secs(5);

pub(crate) struct AppState {
    library: Db,
    thumbs: Db,
    /// Running scans by root id, with their cancel flags.
    scans: Mutex<HashMap<i64, Arc<AtomicBool>>>,
    /// Full-resolution decodes for thumbnail misses, bounded to the CPU count.
    decode_pool: rayon::ThreadPool,
}

type CmdResult<T> = Result<T, String>;

/// Runs database work off the main thread. Non-async Tauri commands execute on the
/// UI thread, so every command that touches SQLite goes through here.
async fn blocking<T, F>(state: &Arc<AppState>, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(&AppState) -> photo_core::Result<T> + Send + 'static,
{
    let state = state.clone();
    tauri::async_runtime::spawn_blocking(move || f(&state))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn library_stats(state: State<'_, Arc<AppState>>) -> CmdResult<timeline::Stats> {
    blocking(&state, |s| timeline::stats(&s.library.read())).await
}

#[tauri::command]
async fn get_layout(state: State<'_, Arc<AppState>>) -> CmdResult<timeline::Layout> {
    blocking(&state, |s| timeline::layout(&s.library.read())).await
}

#[tauri::command]
async fn get_day_assets(
    state: State<'_, Arc<AppState>>,
    days: Vec<i64>,
) -> CmdResult<Vec<timeline::DayAssets>> {
    blocking(&state, move |s| {
        timeline::day_assets(&s.library.read(), &days)
    })
    .await
}

#[tauri::command]
async fn list_roots(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<library::RootInfo>> {
    blocking(&state, |s| {
        library::refresh_volumes(&s.library.write())?;
        library::list_roots(&s.library.read())
    })
    .await
}

#[tauri::command]
async fn scanning_roots(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<i64>> {
    Ok(scans::running(&state))
}

/// Adds a folder to the library and starts scanning it.
#[tauri::command]
async fn add_root(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: PathBuf,
) -> CmdResult<i64> {
    let root_id = blocking(&state, move |s| {
        library::add_root(&s.library.write(), &path)
    })
    .await?;
    scans::start(&app, &state, root_id);
    Ok(root_id)
}

#[tauri::command]
async fn remove_root(state: State<'_, Arc<AppState>>, root_id: i64) -> CmdResult<()> {
    if scans::running(&state).contains(&root_id) {
        return Err("Stop the scan before removing this folder".into());
    }
    blocking(&state, move |s| {
        let removed = library::remove_root(&mut s.library.write(), root_id)?;
        let mut store = s.thumbs.write();
        let tx = store.transaction()?;
        for id in removed {
            thumbs::delete(&tx, id)?;
        }
        tx.commit()?;
        Ok(())
    })
    .await
}

#[tauri::command]
async fn scan_root(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    root_id: i64,
) -> CmdResult<bool> {
    Ok(scans::start(&app, &state, root_id))
}

#[tauri::command]
async fn cancel_scan(state: State<'_, Arc<AppState>>, root_id: i64) -> CmdResult<()> {
    scans::cancel(&state, root_id);
    Ok(())
}

/// Replaces synthetic test assets with `count` new ones. Returns elapsed milliseconds.
/// Thumbnails are cleared wholesale because asset ids can be reused after deletion;
/// real thumbnails regenerate on demand.
#[tauri::command]
async fn generate_synthetic(state: State<'_, Arc<AppState>>, count: usize) -> CmdResult<u64> {
    blocking(&state, move |s| {
        let started = Instant::now();
        thumbs::clear(&s.thumbs.write())?;
        let mut library = s.library.write();
        synth::clear(&library)?;
        synth::generate(&mut library, count, 1)?;
        Ok(started.elapsed().as_millis() as u64)
    })
    .await
}

#[tauri::command]
async fn clear_library(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    blocking(&state, |s| {
        thumbs::clear(&s.thumbs.write())?;
        synth::clear(&s.library.write())
    })
    .await
}

/// Watches for drives being plugged in or removed and tells the UI.
fn poll_volumes(app: AppHandle, state: Arc<AppState>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(VOLUME_POLL);
            if let Ok(true) = library::refresh_volumes(&state.library.write()) {
                let _ = app.emit("volumes-changed", ());
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let library = Db::open(&dir.join("library.sqlite"), 4, schema::migrate_library)?;
            let thumbs = Db::open(&dir.join("thumbs.sqlite"), 4, schema::migrate_thumbs)?;
            let decode_pool = rayon::ThreadPoolBuilder::new()
                .num_threads(std::thread::available_parallelism().map_or(4, |n| n.get()))
                .thread_name(|i| format!("thumb-decode-{i}"))
                .build()?;
            let state = Arc::new(AppState {
                library,
                thumbs,
                scans: Mutex::default(),
                decode_pool,
            });

            library::refresh_volumes(&state.library.write())?;
            for root_id in library::take_interrupted_scans(&state.library.write())? {
                scans::start(app.handle(), &state, root_id);
            }
            poll_volumes(app.handle().clone(), state.clone());
            app.manage(state);
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("thumb", protocol::handle)
        .invoke_handler(tauri::generate_handler![
            library_stats,
            get_layout,
            get_day_assets,
            list_roots,
            scanning_roots,
            add_root,
            remove_root,
            scan_root,
            cancel_scan,
            generate_synthetic,
            clear_library
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
