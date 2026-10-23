//! Core library for PhotoManager: storage, indexing and media processing.
//!
//! This crate must stay free of Tauri and UI concerns so it can be tested with
//! `cargo test`, benchmarked from examples, and reused by other front ends.

pub mod assets;
pub mod collections;
pub mod dates;
pub mod db;
pub mod events;
pub mod hash;
pub mod library;
pub mod live;
pub mod media;
pub mod memories;
pub mod os_thumb;
pub mod places;
pub mod scan;
pub mod schema;
pub mod synth;
pub mod thumbs;
pub mod timeline;
pub mod video;
pub mod volume;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
