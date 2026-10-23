mod collection_commands;
mod diag;
mod protocol;
mod scans;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use photo_core::db::Db;
use photo_core::timeline::{Filter, View};
use photo_core::{assets, library, schema, synth, thumbs, timeline};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

const VOLUME_POLL: Duration = Duration::from_secs(5);

pub(crate) struct AppState {
    library: Db,
    thumbs: Db,
    /// Running scans by root id, with their cancel flags.
    scans: Mutex<HashMap<i64, Arc<AtomicBool>>>,
    /// Full-resolution decodes for thumbnail misses, bounded to the CPU count.
    decode_pool: rayon::ThreadPool,
}

pub(crate) type CmdResult<T> = Result<T, String>;

/// Runs database work off the main thread. Non-async Tauri commands execute on the
/// UI thread, so every command that touches SQLite goes through here.
pub(crate) async fn blocking<T, F>(state: &Arc<AppState>, f: F) -> CmdResult<T>
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
async fn get_layout(
    state: State<'_, Arc<AppState>>,
    view: Option<View>,
    filter: Option<Filter>,
) -> CmdResult<timeline::Layout> {
    blocking(&state, move |s| {
        timeline::layout(
            &s.library.read(),
            view.unwrap_or_default(),
            &filter.unwrap_or_default(),
        )
    })
    .await
}

#[tauri::command]
async fn get_day_assets(
    state: State<'_, Arc<AppState>>,
    view: Option<View>,
    filter: Option<Filter>,
    days: Vec<i64>,
) -> CmdResult<Vec<timeline::DayAssets>> {
    blocking(&state, move |s| {
        timeline::day_assets(
            &s.library.read(),
            view.unwrap_or_default(),
            &filter.unwrap_or_default(),
            &days,
        )
    })
    .await
}

#[tauri::command]
async fn filter_options(state: State<'_, Arc<AppState>>) -> CmdResult<timeline::FilterOptions> {
    blocking(&state, |s| timeline::filter_options(&s.library.read())).await
}

#[tauri::command]
async fn get_asset(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> CmdResult<Option<assets::AssetDetails>> {
    blocking(&state, move |s| assets::details(&s.library.read(), id)).await
}

#[tauri::command]
async fn set_favorite(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    favorite: bool,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        assets::set_favorite(&mut s.library.write(), &ids, favorite)
    })
    .await
}

#[tauri::command]
async fn set_rating(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    rating: Option<u8>,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        assets::set_rating(&mut s.library.write(), &ids, rating)
    })
    .await
}

#[tauri::command]
async fn set_hidden(state: State<'_, Arc<AppState>>, ids: Vec<i64>, hidden: bool) -> CmdResult<()> {
    blocking(&state, move |s| {
        assets::set_hidden(&mut s.library.write(), &ids, hidden)
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
    diag::install_panic_hook();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            diag::init(&dir);
            diag::log(format!("starting {}", app.package_info().version));
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
        .register_asynchronous_uri_scheme_protocol("thumb", protocol::handle_thumb)
        .register_asynchronous_uri_scheme_protocol("media", protocol::handle_media)
        .invoke_handler(tauri::generate_handler![
            library_stats,
            get_layout,
            get_day_assets,
            get_asset,
            filter_options,
            set_favorite,
            set_rating,
            set_hidden,
            collection_commands::list_albums,
            collection_commands::create_album,
            collection_commands::rename_album,
            collection_commands::delete_album,
            collection_commands::add_to_album,
            collection_commands::remove_from_album,
            collection_commands::set_album_cover,
            collection_commands::list_tags,
            collection_commands::add_tag,
            collection_commands::remove_tag,
            collection_commands::rename_tag,
            collection_commands::delete_tag,
            list_roots,
            scanning_roots,
            add_root,
            remove_root,
            scan_root,
            cancel_scan,
            generate_synthetic,
            clear_library
        ])
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(e) => {
            diag::log(format!("startup failed: {e}"));
            std::process::exit(1);
        }
    };
    app.run(|_, event| match event {
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { .. },
            ..
        } => diag::log(format!("window {label} close requested")),
        RunEvent::ExitRequested { code, .. } => {
            diag::log(format!("exit requested (code {code:?})"))
        }
        RunEvent::Exit => diag::log("exit"),
        _ => {}
    });
}
