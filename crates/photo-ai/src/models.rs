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
    /// The name it's kept under (a package's name while it downloads).
    pub name: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// For a package (zip): the files kept from it, each with its own size and hash.
    /// Empty for a plain file.
    pub extract: &'static [Extract],
}

pub struct Extract {
    pub entry: &'static str,
    /// The name it's kept under.
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// A file as installed: (name, size, SHA-256).
type Installed = (&'static str, u64, &'static str);

impl Remote {
    /// The files it leaves on disk.
    pub fn installed(&self) -> Vec<Installed> {
        if self.extract.is_empty() {
            vec![(self.name, self.size, self.sha256)]
        } else {
            self.extract
                .iter()
                .map(|e| (e.name, e.size, e.sha256))
                .collect()
        }
    }

    /// Bytes on disk once installed.
    pub fn installed_size(&self) -> u64 {
        self.installed().iter().map(|f| f.1).sum()
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Pack {
    /// ONNX Runtime, installed with the first feature that runs a model.
    Runtime,
    /// Search by description (CLIP).
    Search,
    /// Faces found and grouped into people (YuNet and SFace).
    People,
    /// People seen whole, for photos with no face found (YOLOX): part of People, a
    /// separate choice.
    Bodies,
    /// Enlarging photos on export (Real-ESRGAN).
    Upscale,
    /// Reading the text in photos, with the operating system's own OCR: nothing to
    /// download.
    Text,
}

impl Pack {
    pub const FEATURES: [Pack; 5] = [
        Pack::Search,
        Pack::People,
        Pack::Bodies,
        Pack::Upscale,
        Pack::Text,
    ];

    fn dir_name(self) -> &'static str {
        match self {
            Pack::Runtime => "runtime",
            Pack::Search => "search",
            Pack::People => "people",
            Pack::Bodies => "bodies",
            Pack::Upscale => "upscale",
            Pack::Text => "text",
        }
    }

    pub fn files(self) -> &'static [Remote] {
        match self {
            Pack::Runtime => RUNTIME,
            Pack::Search => SEARCH,
            Pack::People => PEOPLE,
            Pack::Bodies => BODIES,
            Pack::Upscale => UPSCALE,
            Pack::Text => &[],
        }
    }

    /// Runs a model, so it needs the runtime.
    pub fn needs_runtime(self) -> bool {
        !matches!(self, Pack::Runtime | Pack::Text)
    }

    /// Bytes downloaded to install it (packages count in full).
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
    name: "onnxruntime.nupkg",
    url: "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/1.24.4/microsoft.ml.onnxruntime.directml.1.24.4.nupkg",
    size: 12_458_649,
    sha256: "57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681",
    extract: &[Extract {
        entry: "runtimes/win-x64/native/onnxruntime.dll",
        name: "onnxruntime.dll",
        size: 17_328_152,
        sha256: "e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40",
    }],
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
        extract: &[],
    },
    Remote {
        name: "text.onnx",
        url: clip!("onnx/text_model_quantized.onnx"),
        size: 64_504_507,
        sha256: "73baab855d406190da9faa498cfedf65f15cf309f4cc7385b7b032e6d08e5c3a",
        extract: &[],
    },
    Remote {
        name: "vocab.json",
        url: clip!("vocab.json"),
        size: 862_328,
        sha256: "5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363",
        extract: &[],
    },
    Remote {
        name: "merges.txt",
        url: clip!("merges.txt"),
        size: 524_619,
        sha256: "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a",
        extract: &[],
    },
];

/// OpenCV's YuNet face detector (MIT, Shiqi Yu) and SFace face recogniser (Apache 2.0,
/// Yaoyao Zhong and Weihong Deng), from the OpenCV model zoo, pinned to one revision each.
const PEOPLE: &[Remote] = &[
    Remote {
        name: "detector.onnx",
        url: "https://huggingface.co/opencv/face_detection_yunet/resolve/3cc26e7f1014a5ee5d74a42acee58bafc9d0a310/face_detection_yunet_2023mar.onnx",
        size: 232_589,
        sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
        extract: &[],
    },
    Remote {
        name: "recognizer.onnx",
        url: "https://huggingface.co/opencv/face_recognition_sface/resolve/3d7082438a6e4551e840c9b2bb60b71e8da4b524/face_recognition_sface_2021dec.onnx",
        size: 38_696_353,
        sha256: "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
        extract: &[],
    },
];

/// YOLOX-S (Apache 2.0, Megvii), from the OpenCV model zoo, pinned to one revision. Its
/// 8-bit version is a quarter of the size but ran nine times slower on the CPU (2.4 s a
/// photo against 0.27 s) for the same results.
const BODIES: &[Remote] = &[Remote {
    name: crate::bodies::MODEL_FILE,
    url: "https://huggingface.co/opencv/object_detection_yolox/resolve/78c368f74ce73ee28fc7a1be418a598c71b58b52/object_detection_yolox_2022nov.onnx",
    size: 35_858_002,
    sha256: "c5c2d13e59ae883e6af3b45daea64af4833a4951c92d116ec270d9ddbe998063",
    extract: &[],
}];

/// Real-ESRGAN General x4v3 (BSD 3-Clause, Xintao Wang et al.), as exported to ONNX by
/// Qualcomm AI Hub (release 0.65.0): the model and its weights file.
const UPSCALE: &[Remote] = &[Remote {
    name: "upscale.zip",
    url: "https://qaihub-public-assets.s3.us-west-2.amazonaws.com/qai-hub-models/models/real_esrgan_general_x4v3/releases/v0.65.0/real_esrgan_general_x4v3-onnx-float.zip",
    size: 4_540_895,
    sha256: "fc094329b24a00c936ebc1cc4c51bc59633e7dc36f4a553344342171cbd3c7cf",
    extract: &[
        Extract {
            entry: "real_esrgan_general_x4v3-onnx-float/real_esrgan_general_x4v3.onnx",
            name: "real_esrgan_general_x4v3.onnx",
            size: 161_087,
            sha256: "09e6b675a0a18a97e1057c8dd7e984ef878a0a02ce104430da525b6241c858e7",
        },
        Extract {
            entry: "real_esrgan_general_x4v3-onnx-float/real_esrgan_general_x4v3.data",
            name: "real_esrgan_general_x4v3.data",
            size: 4_836_096,
            sha256: "512d0ec9940c2e9d85d27f2952f12a0b77b7841dc22df4ce9f3ea458bc98f37f",
        },
    ],
}];

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
            && installed_files(pack)
                .iter()
                .all(|&(name, size, _)| fs::metadata(dir.join(name)).is_ok_and(|m| m.len() == size))
    }

    /// Checks every file of an installed pack against its SHA-256.
    pub fn verify(&self, pack: Pack) -> Result<()> {
        if !self.is_installed(pack) {
            return Err(Error::Invalid("This AI feature isn't installed".into()));
        }
        for (name, _, sha) in installed_files(pack) {
            if sha256_file(&self.path(pack, name))? != sha {
                return Err(Error::Invalid(format!(
                    "{name} is damaged; remove the feature and install it again"
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
            let mut done_already = true;
            for (name, size, sha) in f.installed() {
                let target = dir.join(name);
                done_already &= fs::metadata(&target).is_ok_and(|m| m.len() == size)
                    && sha256_file(&target)? == sha;
            }
            if !done_already {
                let download = dir.join(format!("{}.download", f.name));
                fetch(f, &download, cancel, &|n| progress(before + n, total))?;
                if f.extract.is_empty() {
                    fs::rename(&download, dir.join(f.name))?;
                } else {
                    for e in f.extract {
                        extract(&download, e, &dir.join(e.name))?;
                    }
                    fs::remove_file(&download)?;
                }
            }
            before += f.size;
            progress(before, total);
        }
        self.write_manifest(pack)
    }

    fn write_manifest(&self, pack: Pack) -> Result<()> {
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            files: installed_files(pack)
                .iter()
                .map(|f| f.0.to_owned())
                .collect(),
        };
        let dir = self.dir(pack);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join(MANIFEST),
            serde_json::to_vec(&manifest).expect("plain data"),
        )?;
        Ok(())
    }

    /// Installs a pack from files already on this computer (copied from another one, say):
    /// `folder` holds them under their installed names. Each is checked like a download.
    pub fn install_from(&self, pack: Pack, folder: &Path) -> Result<()> {
        let _ = fs::remove_file(self.removal_marker(pack));
        let dir = self.dir(pack);
        fs::create_dir_all(&dir)?;
        let _ = fs::remove_file(dir.join(MANIFEST));
        for (name, size, sha) in installed_files(pack) {
            let source = folder.join(name);
            if fs::metadata(&source).map(|m| m.len()).ok() != Some(size)
                || sha256_file(&source)? != sha
            {
                return Err(Error::Invalid(format!(
                    "{name} in that folder isn't the right file"
                )));
            }
            let part = dir.join(format!("{name}.part"));
            fs::copy(&source, &part)?;
            fs::rename(&part, dir.join(name))?;
        }
        self.write_manifest(pack)
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

/// Every file a pack leaves on disk.
fn installed_files(pack: Pack) -> Vec<Installed> {
    pack.files().iter().flat_map(Remote::installed).collect()
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
    let part = target.with_file_name(format!("{}.part", e.name));
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
                for e in f.extract {
                    assert_eq!(e.sha256.len(), 64);
                    assert!(e.size > 0 && !e.name.is_empty());
                }
            }
        }
    }
}
