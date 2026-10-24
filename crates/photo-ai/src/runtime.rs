//! ONNX Runtime, loaded once per process from the downloaded runtime pack. Models run
//! on the CPU.
//!
//! The GPU (DirectML) was tried and left out: with the 8-bit search model it was four
//! times slower than the CPU on an integrated GPU, and the runtime crashed while
//! optimising the model for DirectML. A CPU-only setup also keeps the download small.

use std::path::Path;
use std::sync::OnceLock;

use ort::environment::Environment;
use ort::session::Session;
use photo_core::{Error, Result};

use crate::models::{Pack, Store};

pub struct Runtime {
    env: Environment,
}

static RUNTIME: OnceLock<std::result::Result<Runtime, String>> = OnceLock::new();

pub(crate) fn ai_error(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("AI: {e}"))
}

impl Runtime {
    /// The runtime, loading it on first use (after checking its files).
    pub fn get(store: &Store) -> Result<&'static Runtime> {
        RUNTIME
            .get_or_init(|| Runtime::load(store).map_err(|e| e.to_string()))
            .as_ref()
            .map_err(|e| Error::Invalid(e.clone()))
    }

    /// True once the runtime is loaded in this process (its files can't be removed).
    pub fn is_loaded() -> bool {
        RUNTIME.get().is_some_and(|r| r.is_ok())
    }

    fn load(store: &Store) -> Result<Runtime> {
        store.verify(Pack::Runtime)?;
        check_cpp_runtime()?;
        let env = ort::init_from(store.path(Pack::Runtime, "onnxruntime.dll"))
            .map_err(ai_error)?
            .with_name("photomanager")
            .build()
            .map_err(ai_error)?;
        Ok(Runtime { env })
    }

    /// A session for `model` on `threads` CPU threads.
    pub fn session(&self, model: &Path, threads: usize) -> Result<Session> {
        let build = || -> ort::Result<Session> {
            Session::builder(&self.env)?
                .with_intra_threads(threads.max(1))?
                .commit_from_file(model)
        };
        build().map_err(ai_error)
    }
}

/// ONNX Runtime needs Microsoft's Visual C++ runtime, which not every Windows has: say
/// so plainly rather than failing to load. Checked before anything is downloaded, too.
pub fn check_cpp_runtime() -> Result<()> {
    #[cfg(windows)]
    for dll in [
        "MSVCP140.dll",
        "MSVCP140_1.dll",
        "VCRUNTIME140.dll",
        "VCRUNTIME140_1.dll",
    ] {
        // SAFETY: Microsoft's own runtime libraries, loaded by name from the system.
        if unsafe { libloading::Library::new(dll) }.is_err() {
            return Err(Error::Invalid(
                "The AI features need Microsoft's Visual C++ runtime, which isn't on this \
                 computer. Install it from https://aka.ms/vs/17/release/vc_redist.x64.exe \
                 and try again."
                    .into(),
            ));
        }
    }
    Ok(())
}
