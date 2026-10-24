//! The optional AI pack: installing and removing features, the background indexer,
//! and search by description. Nothing here downloads or runs until the user installs
//! a feature on the AI page.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use photo_ai::clip::Clip;
use photo_ai::models::{Pack, Store};
use photo_ai::search::{self, Index, Progress};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, CmdResult, blocking};

/// While paused (scan running, on battery), the indexer checks again this often.
const PAUSE_POLL: Duration = Duration::from_secs(20);

pub(crate) struct AiState {
    store: Store,
    /// The feature being installed, if any; one at a time.
    installing: Mutex<Option<Pack>>,
    install_cancel: AtomicBool,
    clip: Mutex<Option<Arc<Clip>>>,
    /// Search embeddings in memory, dropped when indexing adds more.
    index: Mutex<Option<Arc<Index>>>,
    indexing: AtomicBool,
    index_cancel: AtomicBool,
}

impl AiState {
    pub(crate) fn new(dir: PathBuf) -> AiState {
        let store = Store::new(dir);
        store.finish_removals();
        AiState {
            store,
            installing: Mutex::default(),
            install_cancel: AtomicBool::new(false),
            clip: Mutex::default(),
            index: Mutex::default(),
            indexing: AtomicBool::new(false),
            index_cancel: AtomicBool::new(false),
        }
    }

    fn clip(&self) -> photo_core::Result<Arc<Clip>> {
        let mut slot = self.clip.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = slot.as_ref() {
            return Ok(c.clone());
        }
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(8);
        let clip = Arc::new(Clip::load(&self.store, threads)?);
        *slot = Some(clip.clone());
        Ok(clip)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureStatus {
    id: Pack,
    installed: bool,
    /// Bytes to download, counting the shared runtime if it isn't installed yet.
    download_bytes: u64,
    disk_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    features: Vec<FeatureStatus>,
    installing: Option<Pack>,
    /// Search indexing, when search is installed.
    search: Option<Progress>,
    indexing: bool,
}

#[tauri::command]
pub async fn ai_status(state: State<'_, Arc<AppState>>) -> CmdResult<AiStatus> {
    blocking(&state, |s| {
        let ai = &s.ai;
        let runtime_missing = !ai.store.is_installed(Pack::Runtime);
        let features = Pack::FEATURES
            .iter()
            .map(|&p| FeatureStatus {
                id: p,
                installed: ai.store.is_installed(p),
                download_bytes: p.download_size()
                    + if runtime_missing {
                        Pack::Runtime.download_size()
                    } else {
                        0
                    },
                disk_bytes: p.installed_size() + Pack::Runtime.installed_size(),
            })
            .collect();
        let search = if ai.store.is_installed(Pack::Search) {
            Some(search::progress(&s.library.read())?)
        } else {
            None
        };
        Ok(AiStatus {
            features,
            installing: *ai.installing.lock().unwrap_or_else(|e| e.into_inner()),
            search,
            indexing: ai.indexing.load(Ordering::SeqCst),
        })
    })
    .await
}

#[derive(Serialize, Clone)]
struct InstallProgress {
    pack: Pack,
    done: u64,
    total: u64,
}

#[derive(Serialize, Clone)]
struct InstallFinished {
    pack: Pack,
    error: Option<String>,
}

/// Downloads and installs a feature (and the runtime with the first one), in the
/// background; watch `ai-install-progress` and `ai-install-finished`.
#[tauri::command]
pub async fn ai_install(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    pack: Pack,
) -> CmdResult<()> {
    if pack == Pack::Runtime {
        return Err("Install a feature; the runtime comes with it".into());
    }
    // Before downloading anything that couldn't run here.
    photo_ai::runtime::check_cpp_runtime().map_err(|e| e.to_string())?;
    {
        let mut installing = state
            .ai
            .installing
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if installing.is_some() {
            return Err("Another AI feature is being installed".into());
        }
        *installing = Some(pack);
    }
    state.ai.install_cancel.store(false, Ordering::SeqCst);
    let state = state.inner().clone();
    std::thread::Builder::new()
        .name("ai-install".into())
        .spawn(move || {
            let ai = &state.ai;
            let runtime_total = if ai.store.is_installed(Pack::Runtime) {
                0
            } else {
                Pack::Runtime.download_size()
            };
            let total = runtime_total + pack.download_size();
            let last = Cell::new(Instant::now());
            let report = |done: u64| {
                let _ = app.emit("ai-install-progress", InstallProgress { pack, done, total });
            };
            // At most four updates a second.
            let throttled = |done: u64| {
                if last.get().elapsed() >= Duration::from_millis(250) {
                    last.set(Instant::now());
                    report(done);
                }
            };
            let result = (|| {
                ai.store
                    .install(Pack::Runtime, &ai.install_cancel, &|d, _| throttled(d))?;
                ai.store.install(pack, &ai.install_cancel, &|d, _| {
                    throttled(runtime_total + d)
                })?;
                // Checks the files and loads the model now, so problems show here.
                if pack == Pack::Search {
                    ai.clip()?;
                }
                Ok::<(), photo_core::Error>(())
            })();
            *ai.installing.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let error = result.err().map(|e| e.to_string());
            if error.is_none() {
                report(total);
            }
            let _ = app.emit(
                "ai-install-finished",
                InstallFinished {
                    pack,
                    error: error.clone(),
                },
            );
            if error.is_none() && pack == Pack::Search {
                run_indexer(&app, &state);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Stops an install; what was downloaded is kept and resumed next time.
#[tauri::command]
pub async fn ai_cancel_install(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.ai.install_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// Removes a feature: its model files and everything it made (search embeddings).
/// The runtime goes with the last feature. Returns false if some files can only be
/// removed after a restart (the runtime stays loaded until then).
#[tauri::command]
pub async fn ai_remove(state: State<'_, Arc<AppState>>, pack: Pack) -> CmdResult<bool> {
    blocking(&state, move |s| {
        let ai = &s.ai;
        if pack == Pack::Search {
            ai.index_cancel.store(true, Ordering::SeqCst);
            // Wait for the indexer to let go of the model.
            let deadline = Instant::now() + Duration::from_secs(30);
            while ai.indexing.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            *ai.clip.lock().unwrap_or_else(|e| e.into_inner()) = None;
            *ai.index.lock().unwrap_or_else(|e| e.into_inner()) = None;
            s.library.write().execute("DELETE FROM ai_embeddings", [])?;
        }
        let mut done = ai.store.remove(pack, false)?;
        if Pack::FEATURES.iter().all(|&p| !ai.store.is_installed(p)) {
            done &= ai
                .store
                .remove(Pack::Runtime, photo_ai::runtime::Runtime::is_loaded())?;
        }
        Ok(done)
    })
    .await
}

/// Photos matching a description, best first. Empty when nothing stands out.
#[tauri::command]
pub async fn ai_search(state: State<'_, Arc<AppState>>, query: String) -> CmdResult<Vec<i64>> {
    blocking(&state, move |s| {
        let ai = &s.ai;
        if !ai.store.is_installed(Pack::Search) {
            return Err(photo_core::Error::Invalid(
                "Search by description isn't installed".into(),
            ));
        }
        let text = ai.clip()?.embed_text(&query)?;
        let index = {
            let mut slot = ai.index.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_ref() {
                Some(i) => i.clone(),
                None => {
                    let i = Arc::new(Index::load(&s.library.read())?);
                    *slot = Some(i.clone());
                    i
                }
            }
        };
        Ok(index.search(&text).into_iter().map(|(id, _)| id).collect())
    })
    .await
}

/// The ready-made searches ("Beaches", "Food"…): (name, description).
#[tauri::command]
pub fn ai_smart_albums() -> Vec<(&'static str, &'static str)> {
    search::SMART_ALBUMS.to_vec()
}

/// Embeds new photos in the background while search is installed. Waits while a scan
/// runs or the computer is on battery; ends when everything is indexed.
pub(crate) fn run_indexer(app: &AppHandle, state: &Arc<AppState>) {
    let ai = &state.ai;
    if !ai.store.is_installed(Pack::Search) || ai.indexing.swap(true, Ordering::SeqCst) {
        return;
    }
    ai.index_cancel.store(false, Ordering::SeqCst);
    let (app, state) = (app.clone(), state.clone());
    std::thread::Builder::new()
        .name("ai-index".into())
        .spawn(move || {
            let ai = &state.ai;
            let paused = || !crate::scans::running(&state).is_empty() || photo_ai::on_battery();
            let result = (|| -> photo_core::Result<()> {
                let clip = ai.clip()?;
                loop {
                    if ai.index_cancel.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    if paused() {
                        std::thread::sleep(PAUSE_POLL);
                        continue;
                    }
                    let last = Cell::new(Instant::now());
                    let done = search::index_pending(
                        &state.library,
                        &state.thumbs,
                        &clip,
                        &ai.index_cancel,
                        &paused,
                        &|| {
                            if last.get().elapsed() >= Duration::from_secs(1) {
                                last.set(Instant::now());
                                if let Ok(p) = search::progress(&state.library.read()) {
                                    let _ = app.emit("ai-index-progress", p);
                                }
                            }
                        },
                    )?;
                    if done > 0 {
                        *ai.index.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    }
                    let left = search::progress(&state.library.read())?;
                    let _ = app.emit("ai-index-progress", left);
                    // Everything readable is done (offline drives wait for the next run).
                    if done == 0 && !paused() {
                        return Ok(());
                    }
                }
            })();
            if let Err(e) = result {
                crate::diag::log(format!("AI indexing stopped: {e}"));
            }
            ai.indexing.store(false, Ordering::SeqCst);
            let _ = app.emit("ai-index-finished", ());
        })
        .expect("spawn AI indexer");
}
