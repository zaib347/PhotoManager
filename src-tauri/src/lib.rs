mod cleanup_commands;
mod collection_commands;
mod diag;
mod event_commands;
mod export_commands;
mod memory_commands;
mod protocol;
mod scans;
mod similar_commands;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use photo_core::db::Db;
use photo_core::timeline::{Filter, View};
use photo_core::{
    assets, events, fileops, library, places, schema, synth, thumbs, timeline, trips,
};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

const VOLUME_POLL: Duration = Duration::from_secs(5);

pub(crate) struct AppState {
    library: Db,
    thumbs: Db,
    /// Running scans by root id, with their cancel flags.
    scans: Mutex<HashMap<i64, Arc<AtomicBool>>>,
    /// Full-resolution decodes for thumbnail misses, bounded to the CPU count.
    decode_pool: rayon::ThreadPool,
    /// Assets no decoder could thumbnail this session, so scrolling past them doesn't
    /// retry slow OS calls. Cleared on restart (a codec may have been installed).
    thumb_failures: Mutex<std::collections::HashSet<i64>>,
    /// The similar-photos hashing pass is running / should stop (a scan started).
    hashing: AtomicBool,
    hash_cancel: AtomicBool,
    /// An export is running / should stop.
    exporting: AtomicBool,
    export_cancel: AtomicBool,
    /// The photo the export panel previews watermarks on, decoded once.
    watermark_base: Mutex<Option<(i64, Arc<photo_core::export::PreviewBase>)>>,
}

impl AppState {
    pub(crate) fn os_thumb_failed(&self, asset_id: i64) -> bool {
        self.thumb_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&asset_id)
    }

    pub(crate) fn remember_os_thumb_failure(&self, asset_id: i64) {
        self.thumb_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(asset_id);
    }
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

/// Replaces synthetic test assets with `count` new ones, then gives them places and
/// events like a scan would. Returns elapsed milliseconds.
/// Thumbnails are cleared wholesale because asset ids can be reused after deletion;
/// real thumbnails regenerate on demand.
#[tauri::command]
async fn generate_synthetic(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    count: usize,
) -> CmdResult<u64> {
    let ms = blocking(&state, move |s| {
        let started = Instant::now();
        thumbs::clear(&s.thumbs.write())?;
        {
            let mut library = s.library.write();
            synth::clear(&library)?;
            synth::generate(&mut library, count, 1)?;
        }
        places::assign(&s.library)?;
        events::detect(&mut s.library.write())?;
        trips::detect(&mut s.library.write())?;
        Ok(started.elapsed().as_millis() as u64)
    })
    .await?;
    let _ = app.emit("events-changed", ());
    Ok(ms)
}

#[tauri::command]
async fn clear_library(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    blocking(&state, |s| {
        thumbs::clear(&s.thumbs.write())?;
        synth::clear(&s.library.write())?;
        events::detect(&mut s.library.write())?;
        trips::detect(&mut s.library.write())
    })
    .await?;
    let _ = app.emit("events-changed", ());
    Ok(())
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
                thumb_failures: Mutex::default(),
                hashing: AtomicBool::new(false),
                hash_cancel: AtomicBool::new(false),
                exporting: AtomicBool::new(false),
                export_cancel: AtomicBool::new(false),
                watermark_base: Mutex::default(),
            });

            library::refresh_volumes(&state.library.write())?;
            // Before scans resume, so they see the files where the journal says they are.
            match fileops::recover(&state.library) {
                Ok(0) => {}
                Ok(n) => diag::log(format!("{n} interrupted file operations settled")),
                Err(e) => diag::log(format!("file operation recovery failed: {e}")),
            }
            for root_id in library::take_interrupted_scans(&state.library.write())? {
                scans::start(app.handle(), &state, root_id);
            }
            event_commands::catch_up(app.handle(), &state);
            similar_commands::run_in_background(app.handle(), &state);
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
            memory_commands::on_this_day,
            memory_commands::date_range,
            memory_commands::shift_dates,
            memory_commands::restore_dates,
            memory_commands::asset_ids,
            event_commands::list_events,
            event_commands::get_event,
            event_commands::rename_event,
            event_commands::merge_events,
            event_commands::split_event,
            event_commands::create_event,
            event_commands::remove_event,
            event_commands::list_trips,
            event_commands::get_trip,
            event_commands::rename_trip,
            event_commands::remove_trip,
            cleanup_commands::remove_photos,
            export_commands::export_photos,
            export_commands::cancel_export,
            export_commands::watermark_preview,
            cleanup_commands::duplicate_overview,
            cleanup_commands::duplicate_groups,
            cleanup_commands::remove_duplicates,
            cleanup_commands::list_file_ops,
            cleanup_commands::undo_file_op,
            cleanup_commands::empty_file_op,
            cleanup_commands::organize_preview,
            cleanup_commands::organize_run,
            similar_commands::similar_overview,
            similar_commands::similar_groups,
            similar_commands::remove_similar,
            similar_commands::keep_similar,
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
