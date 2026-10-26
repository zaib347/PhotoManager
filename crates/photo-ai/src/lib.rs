//! PhotoManager's optional AI features. Nothing here runs, and nothing is downloaded,
//! until the user installs a feature; models then run on this computer's processor only
//! (photos never leave it).
//!
//! Kept out of `photo-core` so the core stays small and testable without models.

pub mod clip;
pub mod faces;
pub mod models;
pub mod people;
pub mod picture;
pub mod runtime;
pub mod search;
pub mod text;
pub mod tokenizer;
pub mod upscale;
