//! Background scan runner: one thread per root, progress pushed to the UI as events.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use photo_core::library;
use photo_core::scan::{Progress, Scanner, Summary};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::AppState;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent<'a> {
    root_id: i64,
    #[serde(flatten)]
    progress: &'a Progress,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FinishedEvent {
    root_id: i64,
    summary: Option<Summary>,
    error: Option<String>,
}

/// Starts a scan unless one is already running for this root. Returns whether it started.
pub fn start(app: &AppHandle, state: &Arc<AppState>, root_id: i64) -> bool {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut scans = state.scans.lock().unwrap_or_else(|e| e.into_inner());
        if scans.contains_key(&root_id) {
            return false;
        }
        scans.insert(root_id, cancel.clone());
    }
    // Hashing would compete for the disk; it starts again when the scans finish.
    state.hash_cancel.store(true, Ordering::SeqCst);

    let (app, state) = (app.clone(), state.clone());
    std::thread::Builder::new()
        .name(format!("scan-{root_id}"))
        .spawn(move || {
            let job = library::start_scan_job(&state.library.write(), root_id).ok();
            let scanner = Scanner {
                library: &state.library,
                thumbs: &state.thumbs,
                cancel: &cancel,
            };
            let result = scanner.scan_root(root_id, &|progress| {
                let _ = app.emit("scan-progress", ProgressEvent { root_id, progress });
            });

            let job_state = match &result {
                Ok(s) if s.completed => "done",
                Ok(_) => "cancelled",
                Err(_) => "failed",
            };
            if let Some(job) = job {
                let _ = library::finish_job(&state.library.write(), job, job_state);
            }
            state
                .scans
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&root_id);

            let (summary, error) = match result {
                Ok(s) => (Some(s), None),
                Err(e) => (None, Some(e.to_string())),
            };
            let _ = app.emit(
                "scan-finished",
                FinishedEvent {
                    root_id,
                    summary,
                    error,
                },
            );
            crate::similar_commands::run_in_background(&app, &state);
            crate::ai_commands::run_indexer(&app, &state);
        })
        .expect("spawn scan thread");
    true
}

pub fn cancel(state: &AppState, root_id: i64) {
    if let Some(flag) = state
        .scans
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&root_id)
    {
        flag.store(true, Ordering::Relaxed);
    }
}

pub fn running(state: &AppState) -> Vec<i64> {
    state
        .scans
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .copied()
        .collect()
}
