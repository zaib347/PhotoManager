//! The optional AI pack: installing and removing features, the background indexer
//! (search embeddings, faces, text), search by description and by text, and people.
//! Nothing here downloads or runs until the user installs a feature on the AI page.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use photo_ai::clip::Clip;
use photo_ai::faces::Faces;
use photo_ai::models::{Pack, Store};
use photo_ai::people::{People, Person};
use photo_ai::search::{self, Index, Progress};
use photo_ai::text;
use photo_ai::upscale::Upscaler;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, CmdResult, blocking};

/// While paused (a scan running), the indexer checks again this often.
const PAUSE_POLL: Duration = Duration::from_secs(20);
/// Photos each feature indexes before letting the next one have a turn.
const TURN: usize = 64;

/// Threads a model uses in the background: a quarter of the logical processors, so
/// the computer stays responsive (and more makes these small models slower).
fn background_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 4).clamp(1, 4))
}

/// Threads for something the user is waiting on (enlarging): the physical cores.
fn foreground_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 8))
}

pub(crate) struct AiState {
    store: Store,
    /// The feature being installed, if any; one at a time.
    installing: Mutex<Option<Pack>>,
    install_cancel: AtomicBool,
    clip: Mutex<Option<Arc<Clip>>>,
    /// Search embeddings in memory, dropped when indexing adds more.
    index: Mutex<Option<Arc<Index>>>,
    faces: Mutex<Option<Arc<Faces>>>,
    pub(crate) people: People,
    upscaler: Mutex<Option<Arc<Upscaler>>>,
    indexing: AtomicBool,
    index_cancel: AtomicBool,
    /// Why Windows can't read text here (None: it can), and the languages it can read;
    /// asked once per session.
    text_support: OnceLock<(Option<String>, Vec<String>)>,
}

/// Loads a model once and keeps it.
fn cached<T>(
    slot: &Mutex<Option<Arc<T>>>,
    load: impl FnOnce() -> photo_core::Result<T>,
) -> photo_core::Result<Arc<T>> {
    let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = slot.as_ref() {
        return Ok(m.clone());
    }
    let m = Arc::new(load()?);
    *slot = Some(m.clone());
    Ok(m)
}

fn clear<T>(slot: &Mutex<Option<T>>) {
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Asks Windows about text recognition on a thread of its own (its own COM apartment).
fn ask_text_support() -> (Option<String>, Vec<String>) {
    std::thread::spawn(|| (text::unavailable(), text::languages()))
        .join()
        .unwrap_or((Some("Text recognition failed to start".into()), Vec::new()))
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
            faces: Mutex::default(),
            people: People::default(),
            upscaler: Mutex::default(),
            indexing: AtomicBool::new(false),
            index_cancel: AtomicBool::new(false),
            text_support: OnceLock::new(),
        }
    }

    fn clip(&self) -> photo_core::Result<Arc<Clip>> {
        cached(&self.clip, || Clip::load(&self.store, background_threads()))
    }

    fn faces(&self) -> photo_core::Result<Arc<Faces>> {
        cached(&self.faces, || {
            Faces::load(&self.store, background_threads())
        })
    }

    /// The upscaler, for exports that enlarge.
    pub(crate) fn upscaler(&self) -> photo_core::Result<Arc<Upscaler>> {
        if !self.store.is_installed(Pack::Upscale) {
            return Err(photo_core::Error::Invalid(
                "Enlarging needs the Upscale feature: install it on the AI page".into(),
            ));
        }
        cached(&self.upscaler, || {
            Upscaler::load(&self.store, foreground_threads())
        })
    }

    fn text_support(&self) -> &(Option<String>, Vec<String>) {
        self.text_support.get_or_init(ask_text_support)
    }

    fn installed(&self, pack: Pack) -> bool {
        self.store.is_installed(pack)
    }

    /// Stops the indexer and waits (up to 30 s) for it to let go of its models.
    fn stop_indexer(&self) {
        self.index_cancel.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.indexing.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
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
    /// Why it can't be installed on this computer, if it can't.
    unavailable: Option<String>,
}

/// Indexing progress of each installed feature that indexes photos.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct IndexProgress {
    search: Option<Progress>,
    people: Option<Progress>,
    text: Option<Progress>,
}

fn index_progress(ai: &AiState, library: &photo_core::db::Db) -> photo_core::Result<IndexProgress> {
    let conn = &*library.read();
    Ok(IndexProgress {
        search: if ai.installed(Pack::Search) {
            Some(search::progress(conn)?)
        } else {
            None
        },
        people: if ai.installed(Pack::People) {
            Some(photo_ai::people::progress(conn)?)
        } else {
            None
        },
        text: if ai.installed(Pack::Text) {
            Some(text::progress(conn)?)
        } else {
            None
        },
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    features: Vec<FeatureStatus>,
    installing: Option<Pack>,
    progress: IndexProgress,
    indexing: bool,
    /// Why background indexing is waiting, if it is: "scan".
    paused: Option<&'static str>,
    /// Languages Windows reads text in.
    text_languages: Vec<String>,
}

/// Why background indexing waits right now: a folder scan, which reads the same disk.
fn pause_reason(state: &AppState) -> Option<&'static str> {
    (!crate::scans::running(state).is_empty()).then_some("scan")
}

#[tauri::command]
pub async fn ai_status(state: State<'_, Arc<AppState>>) -> CmdResult<AiStatus> {
    blocking(&state, |s| {
        let ai = &s.ai;
        let runtime_missing = !ai.installed(Pack::Runtime);
        let (text_unavailable, languages) = ai.text_support().clone();
        let features = Pack::FEATURES
            .iter()
            .map(|&p| {
                let runtime = p.needs_runtime();
                FeatureStatus {
                    id: p,
                    installed: ai.installed(p),
                    download_bytes: p.download_size()
                        + if runtime && runtime_missing {
                            Pack::Runtime.download_size()
                        } else {
                            0
                        },
                    disk_bytes: p.installed_size()
                        + if runtime {
                            Pack::Runtime.installed_size()
                        } else {
                            0
                        },
                    unavailable: if p == Pack::Text {
                        text_unavailable.clone()
                    } else {
                        None
                    },
                }
            })
            .collect();
        Ok(AiStatus {
            features,
            installing: *ai.installing.lock().unwrap_or_else(|e| e.into_inner()),
            progress: index_progress(ai, &s.library)?,
            indexing: ai.indexing.load(Ordering::SeqCst),
            paused: pause_reason(s),
            text_languages: languages,
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

/// Downloads and installs a feature (and the runtime with the first one that runs a
/// model), in the background; watch `ai-install-progress` and `ai-install-finished`.
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
    if pack.needs_runtime() {
        photo_ai::runtime::check_cpp_runtime().map_err(|e| e.to_string())?;
    }
    if pack == Pack::Text
        && let (Some(why), _) = ask_text_support()
    {
        return Err(why);
    }
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
            let runtime_total = if !pack.needs_runtime() || ai.installed(Pack::Runtime) {
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
                if pack.needs_runtime() {
                    ai.store
                        .install(Pack::Runtime, &ai.install_cancel, &|d, _| throttled(d))?;
                }
                ai.store.install(pack, &ai.install_cancel, &|d, _| {
                    throttled(runtime_total + d)
                })?;
                // Checks the files and loads the model now, so problems show here.
                match pack {
                    Pack::Search => drop(ai.clip()?),
                    Pack::People => drop(ai.faces()?),
                    Pack::Upscale => drop(ai.upscaler()?),
                    Pack::Text | Pack::Runtime => {}
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
            if error.is_none() {
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

/// Removes a feature: its model files and everything it made (search embeddings,
/// faces and people, text). The runtime goes with the last feature that needs it.
/// Returns false if some files can only be removed after a restart (the runtime stays
/// loaded until then).
#[tauri::command]
pub async fn ai_remove(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    pack: Pack,
) -> CmdResult<bool> {
    let done = blocking(&state, move |s| {
        let ai = &s.ai;
        // The indexer may be using the feature; it starts again below for the others.
        ai.stop_indexer();
        match pack {
            Pack::Search => {
                clear(&ai.clip);
                clear(&ai.index);
                s.library.write().execute("DELETE FROM ai_embeddings", [])?;
            }
            Pack::People => {
                clear(&ai.faces);
                ai.people.clear(&s.library, &s.thumbs)?;
            }
            Pack::Upscale => clear(&ai.upscaler),
            Pack::Text => text::clear(&s.library)?,
            Pack::Runtime => {}
        }
        let mut done = ai.store.remove(pack, false)?;
        let needed = Pack::FEATURES
            .iter()
            .any(|&p| p.needs_runtime() && ai.installed(p));
        if !needed {
            done &= ai
                .store
                .remove(Pack::Runtime, photo_ai::runtime::Runtime::is_loaded())?;
        }
        Ok(done)
    })
    .await?;
    run_indexer(&app, state.inner());
    Ok(done)
}

/// Photos matching what was typed, best first: those whose text has every word (when
/// text in photos is installed and `text` is set), then those that look like the
/// description (when search is installed). Empty when nothing matches.
#[tauri::command]
pub async fn ai_search(
    state: State<'_, Arc<AppState>>,
    query: String,
    text: bool,
) -> CmdResult<Vec<i64>> {
    blocking(&state, move |s| {
        let ai = &s.ai;
        let (by_look, by_text) = (ai.installed(Pack::Search), text && ai.installed(Pack::Text));
        if !by_look && !by_text {
            return Err(photo_core::Error::Invalid(
                "Search needs Search by description or Text in photos: install one on the AI page"
                    .into(),
            ));
        }
        let mut ids = if by_text {
            text::search(&s.library.read(), &query)?
        } else {
            Vec::new()
        };
        if by_look {
            let query_vec = ai.clip()?.embed_text(&query)?;
            let index = cached(&ai.index, || Index::load(&s.library.read()))?;
            let seen: std::collections::HashSet<i64> = ids.iter().copied().collect();
            ids.extend(
                index
                    .search(&query_vec)
                    .into_iter()
                    .map(|(id, _)| id)
                    .filter(|id| !seen.contains(id)),
            );
        }
        ids.truncate(search::MAX_RESULTS);
        Ok(ids)
    })
    .await
}

/// The ready-made searches ("Beaches", "Food"…): (name, description).
#[tauri::command]
pub fn ai_smart_albums() -> Vec<(&'static str, &'static str)> {
    search::SMART_ALBUMS.to_vec()
}

/// The text read in a photo, if text in photos is installed and found some.
#[tauri::command]
pub async fn ai_photo_text(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<Option<String>> {
    blocking(&state, move |s| {
        if !s.ai.installed(Pack::Text) {
            return Ok(None);
        }
        text::text_of(&s.library.read(), id)
    })
    .await
}

/// People, named first then by number of photos; `hidden` lists the hidden ones.
#[tauri::command]
pub async fn ai_people(state: State<'_, Arc<AppState>>, hidden: bool) -> CmdResult<Vec<Person>> {
    blocking(&state, move |s| {
        if !s.ai.installed(Pack::People) {
            return Ok(Vec::new());
        }
        s.ai.people.list(&s.library.read(), hidden)
    })
    .await
}

#[tauri::command]
pub async fn ai_person(state: State<'_, Arc<AppState>>, id: i64) -> CmdResult<Option<Person>> {
    blocking(&state, move |s| s.ai.people.get(&s.library.read(), id)).await
}

#[tauri::command]
pub async fn ai_rename_person(
    state: State<'_, Arc<AppState>>,
    id: i64,
    name: String,
) -> CmdResult<()> {
    blocking(&state, move |s| s.ai.people.rename(&s.library, id, &name)).await
}

/// Merges `others` into `into` (one person in several groups).
#[tauri::command]
pub async fn ai_merge_people(
    state: State<'_, Arc<AppState>>,
    into: i64,
    others: Vec<i64>,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        s.ai.people.merge(&s.library, into, &others)
    })
    .await
}

#[tauri::command]
pub async fn ai_hide_person(
    state: State<'_, Arc<AppState>>,
    id: i64,
    hidden: bool,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        s.ai.people.set_hidden(&s.library, id, hidden)
    })
    .await
}

/// "Not this person": takes photos out of a person for good. Returns how many faces.
#[tauri::command]
pub async fn ai_remove_from_person(
    state: State<'_, Arc<AppState>>,
    id: i64,
    ids: Vec<i64>,
) -> CmdResult<usize> {
    blocking(&state, move |s| {
        s.ai.people.remove_photos(&s.library, id, &ids)
    })
    .await
}

/// Uses a photo's face of the person as their picture.
#[tauri::command]
pub async fn ai_person_cover(
    state: State<'_, Arc<AppState>>,
    id: i64,
    asset_id: i64,
) -> CmdResult<()> {
    blocking(&state, move |s| {
        s.ai.people.set_cover(&s.library, id, asset_id)
    })
    .await
}

/// The named people in a photo: (id, name).
#[tauri::command]
pub async fn ai_photo_people(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> CmdResult<Vec<(i64, String)>> {
    blocking(&state, move |s| {
        if !s.ai.installed(Pack::People) {
            return Ok(Vec::new());
        }
        s.ai.people.in_photo(&s.library.read(), id)
    })
    .await
}

/// Indexes new photos in the background for each installed feature that needs it
/// (search, faces, text), taking turns. Waits while a scan runs; ends when everything
/// is done.
pub(crate) fn run_indexer(app: &AppHandle, state: &Arc<AppState>) {
    let ai = &state.ai;
    let wanted = [Pack::Search, Pack::People, Pack::Text]
        .iter()
        .any(|&p| ai.installed(p));
    if !wanted || ai.indexing.swap(true, Ordering::SeqCst) {
        return;
    }
    ai.index_cancel.store(false, Ordering::SeqCst);
    let (app, state) = (app.clone(), state.clone());
    std::thread::Builder::new()
        .name("ai-index".into())
        .spawn(move || {
            let ai = &state.ai;
            // Tells the page when indexing starts or stops waiting, and why.
            let shown = Cell::new(None);
            let paused = || {
                let reason = pause_reason(&state);
                if shown.replace(reason) != reason {
                    let _ = app.emit("ai-index-paused", reason);
                }
                reason.is_some()
            };
            let last = Cell::new(Instant::now());
            let report = || {
                if last.get().elapsed() >= Duration::from_secs(1) {
                    last.set(Instant::now());
                    if let Ok(p) = index_progress(ai, &state.library) {
                        let _ = app.emit("ai-index-progress", p);
                    }
                }
            };
            // A feature that fails stops for this run; the others carry on.
            let mut failed: Vec<Pack> = Vec::new();
            let fail = |failed: &mut Vec<Pack>, pack: Pack, e: photo_core::Error| {
                crate::diag::log(format!("AI indexing ({pack:?}) stopped: {e}"));
                failed.push(pack);
            };
            loop {
                if ai.index_cancel.load(Ordering::SeqCst) {
                    break;
                }
                if paused() {
                    std::thread::sleep(PAUSE_POLL);
                    continue;
                }
                let mut done = 0;
                let cancel = &ai.index_cancel;
                if ai.installed(Pack::Search) && !failed.contains(&Pack::Search) {
                    match ai.clip().and_then(|clip| {
                        search::index_pending(
                            &state.library,
                            &state.thumbs,
                            &clip,
                            TURN,
                            cancel,
                            &paused,
                            &report,
                        )
                    }) {
                        Ok(n) => {
                            if n > 0 {
                                clear(&ai.index);
                            }
                            done += n;
                        }
                        Err(e) => fail(&mut failed, Pack::Search, e),
                    }
                }
                if ai.installed(Pack::People) && !failed.contains(&Pack::People) {
                    match ai.faces().and_then(|faces| {
                        ai.people.index_pending(
                            &state.library,
                            &state.thumbs,
                            &faces,
                            TURN,
                            cancel,
                            &paused,
                            &report,
                        )
                    }) {
                        Ok(n) => {
                            if n > 0 {
                                let _ = app.emit("ai-people-changed", ());
                            }
                            done += n;
                        }
                        Err(e) => fail(&mut failed, Pack::People, e),
                    }
                }
                if ai.installed(Pack::Text) && !failed.contains(&Pack::Text) {
                    match text::index_pending(&state.library, TURN, cancel, &paused, &report) {
                        Ok(n) => done += n,
                        Err(e) => fail(&mut failed, Pack::Text, e),
                    }
                }
                if let Ok(p) = index_progress(ai, &state.library) {
                    let _ = app.emit("ai-index-progress", p);
                }
                // Everything readable is done (offline drives wait for the next run).
                if done == 0 && !paused() {
                    break;
                }
            }
            ai.indexing.store(false, Ordering::SeqCst);
            let _ = app.emit("ai-index-finished", ());
        })
        .expect("spawn AI indexer");
}
