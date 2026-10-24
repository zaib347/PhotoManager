//! The AI model manager: what each feature downloads, from where, and how it's
//! checked. Nothing is downloaded until the user installs a feature.
//!
//! Every file has a fixed size and SHA-256 here, so a download that's cut short,
//! changed on the server or tampered with is never used. Files pinned to exact
//! versions (Hugging Face revisions, NuGet package versions) keep the hashes valid.
//! A feature's files are checked again against their hashes before first use in
//! each session.
//!
//! The ONNX Runtime comes from Microsoft's NuGet package (only its 64-bit Windows DLL is
//! kept); it's shared by every feature and installed with the first one.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use photo_core::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A file to download.
pub struct Remote {
    /// The name it's kept under.
    pub name: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// For a package (zip): the one file kept from it, with its own size and hash.
    pub extract: Option<Extract>,
}

pub struct Extract {
    pub entry: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

impl Remote {
    /// Bytes on disk once installed.
    pub fn installed_size(&self) -> u64 {
        self.extract.as_ref().map_or(self.size, |e| e.size)
    }

    fn installed_sha(&self) -> &'static str {
        self.extract.as_ref().map_or(self.sha256, |e| e.sha256)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Pack {
    /// ONNX Runtime, installed with the first feature.
    Runtime,
    /// Search by description (CLIP).
    Search,
}

impl Pack {
    pub const FEATURES: [Pack; 1] = [Pack::Search];

    fn dir_name(self) -> &'static str {
        match self {
            Pack::Runtime => "runtime",
            Pack::Search => "search",
        }
    }

    pub fn files(self) -> &'static [Remote] {
        match self {
            Pack::Runtime => RUNTIME,
            Pack::Search => SEARCH,
        }
    }

    /// Bytes downloaded to install it (the runtime's packages count in full).
    pub fn download_size(self) -> u64 {
        self.files().iter().map(|f| f.size).sum()
    }

    pub fn installed_size(self) -> u64 {
        self.files().iter().map(Remote::installed_size).sum()
    }
}

/// Microsoft's DirectML build of ONNX Runtime, used on the CPU only: its package is a
/// tenth of the size of Microsoft's plain CPU package (12 MB against 125 MB).
const RUNTIME: &[Remote] = &[Remote {
    name: "onnxruntime.dll",
    url: "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/1.24.4/microsoft.ml.onnxruntime.directml.1.24.4.nupkg",
    size: 12_458_649,
    sha256: "57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681",
    extract: Some(Extract {
        entry: "runtimes/win-x64/native/onnxruntime.dll",
        size: 17_328_152,
        sha256: "e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40",
    }),
}];

/// OpenAI CLIP ViT-B/32 (MIT), as exported to ONNX (8-bit) by Xenova, pinned to one
/// revision.
macro_rules! clip {
    ($file:literal) => {
        concat!(
            "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/d15189d7028b43f1d3e65039190477f6af591c2a/",
            $file
        )
    };
}

const SEARCH: &[Remote] = &[
    Remote {
        name: "vision.onnx",
        url: clip!("onnx/vision_model_quantized.onnx"),
        size: 89_117_001,
        sha256: "583fd1110a514667812fee7d684952aaf82a99b959760c8d7dca7e0ab9839299",
        extract: None,
    },
    Remote {
        name: "text.onnx",
        url: clip!("onnx/text_model_quantized.onnx"),
        size: 64_504_507,
        sha256: "73baab855d406190da9faa498cfedf65f15cf309f4cc7385b7b032e6d08e5c3a",
        extract: None,
    },
    Remote {
        name: "vocab.json",
        url: clip!("vocab.json"),
        size: 862_328,
        sha256: "5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363",
        extract: None,
    },
    Remote {
        name: "merges.txt",
        url: clip!("merges.txt"),
        size: 524_619,
        sha256: "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a",
        extract: None,
    },
];

/// Where everything AI lives: `<app data>/ai/<pack>/`.
pub struct Store {
    root: PathBuf,
}

/// Written once every file of a pack is downloaded and checked.
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    files: Vec<String>,
}

const MANIFEST: &str = "installed.json";
const MANIFEST_VERSION: u32 = 1;

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    pub fn dir(&self, pack: Pack) -> PathBuf {
        self.root.join(pack.dir_name())
    }

    pub fn path(&self, pack: Pack, name: &str) -> PathBuf {
        self.dir(pack).join(name)
    }

    /// Every file is there with its expected size, and the install finished.
    pub fn is_installed(&self, pack: Pack) -> bool {
        let dir = self.dir(pack);
        let manifest: Option<Manifest> = fs::read(dir.join(MANIFEST))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        manifest.is_some_and(|m| m.version == MANIFEST_VERSION)
            && pack.files().iter().all(|f| {
                fs::metadata(dir.join(f.name)).is_ok_and(|m| m.len() == f.installed_size())
            })
    }

    /// Checks every file of an installed pack against its SHA-256.
    pub fn verify(&self, pack: Pack) -> Result<()> {
        if !self.is_installed(pack) {
            return Err(Error::Invalid("This AI feature isn't installed".into()));
        }
        for f in pack.files() {
            let path = self.path(pack, f.name);
            if sha256_file(&path)? != f.installed_sha() {
                return Err(Error::Invalid(format!(
                    "{} is damaged; remove the feature and install it again",
                    f.name
                )));
            }
        }
        Ok(())
    }

    /// Downloads, checks and installs a pack. Calls `progress(bytes done, bytes total)`;
    /// stops when `cancel` is set (a later install resumes the files cut short).
    pub fn install(
        &self,
        pack: Pack,
        cancel: &AtomicBool,
        progress: &dyn Fn(u64, u64),
    ) -> Result<()> {
        // Wanted again: a removal waiting for a restart is off.
        let _ = fs::remove_file(self.removal_marker(pack));
        if self.is_installed(pack) {
            return Ok(());
        }
        let dir = self.dir(pack);
        fs::create_dir_all(&dir)?;
        let _ = fs::remove_file(dir.join(MANIFEST));
        let total = pack.download_size();
        let mut before = 0;
        for f in pack.files() {
            let target = dir.join(f.name);
            let done_already = fs::metadata(&target).is_ok_and(|m| m.len() == f.installed_size())
                && sha256_file(&target)? == f.installed_sha();
            if !done_already {
                let download = dir.join(format!("{}.download", f.name));
                fetch(f, &download, cancel, &|n| progress(before + n, total))?;
                match &f.extract {
                    None => fs::rename(&download, &target)?,
                    Some(e) => {
                        extract(&download, e, &target)?;
                        fs::remove_file(&download)?;
                    }
                }
            }
            before += f.size;
            progress(before, total);
        }
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            files: pack.files().iter().map(|f| f.name.to_owned()).collect(),
        };
        fs::write(
            dir.join(MANIFEST),
            serde_json::to_vec(&manifest).expect("plain data"),
        )?;
        Ok(())
    }

    /// Installs a pack from files already on this computer (copied from another one, say):
    /// `folder` holds them under their installed names. Each is checked like a download.
    pub fn install_from(&self, pack: Pack, folder: &Path) -> Result<()> {
        let dir = self.dir(pack);
        fs::create_dir_all(&dir)?;
        let _ = fs::remove_file(dir.join(MANIFEST));
        for f in pack.files() {
            let source = folder.join(f.name);
            if fs::metadata(&source).map(|m| m.len()).ok() != Some(f.installed_size())
                || sha256_file(&source)? != f.installed_sha()
            {
                return Err(Error::Invalid(format!(
                    "{} in that folder isn't the right file",
                    f.name
                )));
            }
            let part = dir.join(format!("{}.part", f.name));
            fs::copy(&source, &part)?;
            fs::rename(&part, dir.join(f.name))?;
        }
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            files: pack.files().iter().map(|f| f.name.to_owned()).collect(),
        };
        fs::write(
            dir.join(MANIFEST),
            serde_json::to_vec(&manifest).expect("plain data"),
        )?;
        Ok(())
    }

    /// Removes a pack's files. Files in use (the runtime, while loaded) are removed at
    /// the next start instead; returns false then.
    pub fn remove(&self, pack: Pack, in_use: bool) -> Result<bool> {
        let dir = self.dir(pack);
        if !dir.exists() {
            return Ok(true);
        }
        // A loaded runtime is left whole (so installing again just works) and goes at
        // the next start.
        if in_use {
            fs::write(self.removal_marker(pack), b"")?;
            return Ok(false);
        }
        let _ = fs::remove_file(dir.join(MANIFEST));
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(true),
            Err(_) => {
                fs::write(self.removal_marker(pack), b"")?;
                Ok(false)
            }
        }
    }

    fn removal_marker(&self, pack: Pack) -> PathBuf {
        self.root.join(format!("{}.remove", pack.dir_name()))
    }

    /// Finishes removals that had to wait for a restart. Call before loading anything.
    pub fn finish_removals(&self) {
        for pack in [Pack::Runtime].into_iter().chain(Pack::FEATURES) {
            let marker = self.removal_marker(pack);
            if !marker.exists() {
                continue;
            }
            let dir = self.dir(pack);
            if !dir.exists() || fs::remove_dir_all(&dir).is_ok() {
                let _ = fs::remove_file(marker);
            }
        }
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .user_agent(concat!("PhotoManager/", env!("CARGO_PKG_VERSION")))
            .build(),
    )
}

/// Downloads `f` to `path`, resuming a partial file, and checks its size and hash.
fn fetch(f: &Remote, path: &Path, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<()> {
    let net = |e: ureq::Error| {
        Error::Invalid(format!(
            "Download failed: {e}. Check the internet connection and try again."
        ))
    };
    let mut have = fs::metadata(path).map_or(0, |m| m.len());
    if have > f.size {
        fs::remove_file(path)?;
        have = 0;
    }
    if have < f.size {
        let mut request = agent().get(f.url);
        if have > 0 {
            request = request.header("Range", &format!("bytes={have}-"));
        }
        let mut response = request.call().map_err(net)?;
        // A server that ignores the range sends everything again.
        let resumed = have > 0 && response.status().as_u16() == 206;
        let mut file = if resumed {
            fs::OpenOptions::new().append(true).open(path)?
        } else {
            have = 0;
            File::create(path)?
        };
        let mut reader = response.body_mut().as_reader();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Invalid("Stopped".into()));
            }
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    return Err(Error::Invalid(format!(
                        "Download failed: {e}. Try again to resume."
                    )));
                }
            };
            file.write_all(&buf[..n])?;
            have += n as u64;
            if have > f.size {
                break;
            }
            progress(have);
        }
        file.flush()?;
    }
    let size = fs::metadata(path)?.len();
    if size != f.size || sha256_file(path)? != f.sha256 {
        fs::remove_file(path)?;
        return Err(Error::Invalid(format!(
            "{} didn't download correctly (its contents don't match). Try again.",
            f.name
        )));
    }
    Ok(())
}

/// Copies `e.entry` out of the zip at `package` into `target`, checking it.
fn extract(package: &Path, e: &Extract, target: &Path) -> Result<()> {
    let bad = |what: String| {
        Error::Invalid(format!(
            "The downloaded package is damaged ({what}). Try again."
        ))
    };
    let mut zip = zip::ZipArchive::new(File::open(package)?).map_err(|err| bad(err.to_string()))?;
    let mut entry = zip.by_name(e.entry).map_err(|err| bad(err.to_string()))?;
    let part = target.with_extension("part");
    {
        let mut out = File::create(&part)?;
        io::copy(&mut entry, &mut out)?;
        out.flush()?;
    }
    if fs::metadata(&part)?.len() != e.size || sha256_file(&part)? != e.sha256 {
        fs::remove_file(&part)?;
        return Err(bad(format!("{} doesn't match", e.entry)));
    }
    fs::rename(&part, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pack_in_use_is_removed_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let runtime = store.dir(Pack::Runtime);
        fs::create_dir_all(&runtime).unwrap();
        fs::write(runtime.join("onnxruntime.dll"), b"x").unwrap();
        // In use: left whole, marked.
        assert!(!store.remove(Pack::Runtime, true).unwrap());
        assert!(runtime.join("onnxruntime.dll").exists());
        // Installing again would clear the mark; a restart removes it.
        store.finish_removals();
        assert!(!runtime.exists());
        assert!(!store.removal_marker(Pack::Runtime).exists());
        // Not in use: removed now.
        fs::create_dir_all(store.dir(Pack::Search)).unwrap();
        assert!(store.remove(Pack::Search, false).unwrap());
        assert!(!store.dir(Pack::Search).exists());
    }

    #[test]
    fn every_file_has_a_hash_and_size() {
        for pack in [Pack::Runtime].into_iter().chain(Pack::FEATURES) {
            for f in pack.files() {
                assert_eq!(f.sha256.len(), 64, "{}", f.name);
                assert!(f.size > 0 && f.url.starts_with("https://"), "{}", f.name);
                if let Some(e) = &f.extract {
                    assert_eq!(e.sha256.len(), 64);
                    assert!(e.size > 0);
                }
            }
        }
    }
}
