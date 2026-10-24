//! Memory slideshows: choosing their photos, the music file, and saving videos in the
//! background (one at a time, with progress and Stop, like export).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use photo_core::movie::{self, Options, Report};
use serde::Serialize;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, CmdResult, blocking};

/// Audio files the slideshow can play, and their biggest size.
const AUDIO: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "wma"];
const MAX_AUDIO: u64 = 200 << 20;

/// The photos a slideshow of `ids` plays, in order.
#[tauri::command]
pub async fn slideshow_pick(state: State<'_, Arc<AppState>>, ids: Vec<i64>) -> CmdResult<Vec<i64>> {
    blocking(&state, move |s| {
        movie::pick(&s.library.read(), &ids, movie::MAX_PHOTOS)
    })
    .await
}

/// A music file's bytes, for playing under the slideshow.
#[tauri::command]
pub async fn read_audio(path: PathBuf) -> CmdResult<Response> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if !AUDIO.contains(&ext.as_str()) {
        return Err("That isn't a music file this can play".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        if size > MAX_AUDIO {
            return Err("That music file is too big".to_owned());
        }
        std::fs::read(&path)
            .map(Response::new)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone)]
struct ProgressEvent {
    done: usize,
    total: usize,
}

#[derive(Serialize, Clone)]
struct FinishedEvent {
    report: Option<Report>,
    error: Option<String>,
}

/// Starts saving a slideshow as an MP4 at `path`; watch `video-progress` and
/// `video-finished`.
#[tauri::command]
pub async fn make_video(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    options: Options,
    path: PathBuf,
) -> CmdResult<()> {
    if state.making_video.swap(true, Ordering::SeqCst) {
        return Err("A video is already being made".into());
    }
    state.video_cancel.store(false, Ordering::SeqCst);
    let state = state.inner().clone();
    std::thread::Builder::new()
        .name("video".into())
        .spawn(move || {
            let result = movie::make(
                &state.library,
                &ids,
                &options,
                &path,
                &state.video_cancel,
                &|done, total| {
                    let _ = app.emit("video-progress", ProgressEvent { done, total });
                },
            );
            state.making_video.store(false, Ordering::SeqCst);
            let (report, error) = match result {
                Ok(r) => (Some(r), None),
                Err(e) => (None, Some(e.to_string())),
            };
            let _ = app.emit("video-finished", FinishedEvent { report, error });
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Stops the video being made (its partial file is removed).
#[tauri::command]
pub async fn cancel_video(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.video_cancel.store(true, Ordering::SeqCst);
    Ok(())
}
