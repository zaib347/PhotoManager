//! Core library for PhotoManager: storage, indexing and media processing.
//!
//! This crate must stay free of Tauri and UI concerns so it can be tested with
//! `cargo test`, benchmarked from examples, and reused by other front ends.

pub mod assets;
pub mod collage;
pub mod collections;
pub mod dates;
pub mod db;
pub mod duplicates;
pub mod edit;
pub mod events;
pub mod export;
pub mod fileops;
pub mod hash;
pub mod imports;
pub mod jpeg;
pub mod library;
pub mod live;
pub mod map;
pub mod media;
pub mod memories;
pub mod movie;
pub mod organize;
pub mod os_thumb;
pub mod places;
pub mod raw;
pub mod rename;
pub mod rotate;
pub mod scan;
pub mod schema;
pub mod similar;
pub mod synth;
pub mod thumbs;
pub mod timeline;
pub mod trips;
pub mod video;
pub mod volume;
pub mod watermark;
pub mod xmp;

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
