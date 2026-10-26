//! The Map view: the bundled world map, photo clusters, city search and placing
//! photos by hand.

use std::sync::Arc;

use photo_core::map::{self, Area, CityInfo, Cluster, Points};
use serde::Serialize;
use tauri::State;
use tauri::ipc::Response;

use crate::{AppState, CmdResult, blocking};

/// The packed world map (Natural Earth), drawn by the frontend.
#[tauri::command]
pub fn map_data() -> Response {
    Response::new(map::data().to_vec())
}

/// Town labels: (name, lat, lon, population), biggest first.
#[tauri::command]
pub async fn map_cities() -> CmdResult<Vec<(String, f32, f32, u32)>> {
    tauri::async_runtime::spawn_blocking(map::city_labels)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapSummary {
    /// Photos and videos with a location.
    count: usize,
    /// Where they are, to frame the map on.
    bounds: Option<Area>,
}

/// Loads the locations of every visible photo, for the clusters. Called when the map
/// opens, so it shows the library as it is now.
#[tauri::command]
pub async fn map_load(state: State<'_, Arc<AppState>>) -> CmdResult<MapSummary> {
    blocking(&state, |s| {
        let points = Points::load(&s.library.read())?;
        let summary = MapSummary {
            count: points.len(),
            bounds: points.bounds(),
        };
        *s.map_points.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(points));
        Ok(summary)
    })
    .await
}

/// Clusters at `zoom` inside the Web Mercator rectangle (`x0`, `y0`)–(`x1`, `y1`).
#[tauri::command]
pub async fn map_clusters(
    state: State<'_, Arc<AppState>>,
    zoom: f64,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
) -> CmdResult<Vec<Cluster>> {
    blocking(&state, move |s| {
        let cached = s
            .map_points
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let points = match cached {
            Some(p) => p,
            None => {
                let p = Arc::new(Points::load(&s.library.read())?);
                *s.map_points.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.clone());
                p
            }
        };
        Ok(points.clusters(zoom, x0, y0, x1, y1))
    })
    .await
}

#[tauri::command]
pub async fn search_cities(query: String) -> CmdResult<Vec<CityInfo>> {
    tauri::async_runtime::spawn_blocking(move || map::search_cities(&query, 12))
        .await
        .map_err(|e| e.to_string())
}

/// Places photos at (`lat`, `lon`). Returns how many were placed.
#[tauri::command]
pub async fn set_location(
    state: State<'_, Arc<AppState>>,
    ids: Vec<i64>,
    lat: f64,
    lon: f64,
) -> CmdResult<usize> {
    blocking(&state, move |s| {
        let n = map::set_location(&s.library, &ids, lat, lon)?;
        *s.map_points.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(n)
    })
    .await
}

/// Takes photos off the map. Returns how many had a location.
#[tauri::command]
pub async fn clear_location(state: State<'_, Arc<AppState>>, ids: Vec<i64>) -> CmdResult<usize> {
    blocking(&state, move |s| {
        let n = map::clear_location(&s.library, &ids)?;
        *s.map_points.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(n)
    })
    .await
}
