//! Unzipping Google Takeout and iCloud Photos exports into a folder, so they can be
//! added to the library and their metadata imported (see [`crate::imports`]).
//!
//! Both services split a big export into several zips that share one folder layout
//! (`Takeout/Google Photos/…`, `iCloud Photos/…`); unzipping them all into one folder
//! puts each photo next to its metadata again, even when they came in different parts.
//!
//! Files are written as `<name>.partial` and renamed when complete, so a file that
//! exists is whole: unzipping again (after a cancel or a crash) skips the files already
//! there, and never overwrites one. Entries whose names would land outside the folder
//! are left out.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use zip::ZipArchive;

use crate::{Error, Result};

/// How often progress is reported.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);
/// Free space kept on the drive beyond what the export needs.
const SPARE_BYTES: u64 = 512 << 20;

#[derive(Serialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// The zip being unzipped (1-based) and how many there are.
    pub zip: usize,
    pub zips: usize,
    /// Bytes unzipped so far, of all the zips' files.
    pub bytes: u64,
    pub total_bytes: u64,
}

#[derive(Serialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Files written.
    pub files: usize,
    /// Files already in the folder, left as they were.
    pub skipped: usize,
    /// Entries with names that would land outside the folder, left out.
    pub unsafe_names: usize,
    /// False when cancelled.
    pub completed: bool,
}

fn open(path: &Path) -> Result<ZipArchive<File>> {
    ZipArchive::new(File::open(path)?).map_err(|e| {
        Error::Invalid(format!(
            "{} could not be read as a zip file ({e})",
            path.display()
        ))
    })
}

fn zip_error(path: &Path, e: zip::result::ZipError) -> Error {
    match e {
        zip::result::ZipError::Io(e) => Error::Io(e),
        e => Error::Invalid(format!("{}: {e}", path.display())),
    }
}

/// Bytes of all files in `zips`, and those not yet in `dest`.
fn sizes(zips: &[PathBuf], dest: &Path) -> Result<(u64, u64)> {
    let (mut total, mut needed) = (0u64, 0u64);
    for path in zips {
        let mut archive = open(path)?;
        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i).map_err(|e| zip_error(path, e))?;
            if entry.is_dir() {
                continue;
            }
            total += entry.size();
            let there = entry
                .enclosed_name()
                .and_then(|name| fs::metadata(dest.join(name)).ok())
                .is_some();
            if !there {
                needed += entry.size();
            }
        }
    }
    Ok((total, needed))
}

/// Unzips every file in `zips` into `dest` (which must exist). Refuses to start when
/// the drive hasn't room for the files not there yet.
pub fn extract(
    zips: &[PathBuf],
    dest: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(&Progress),
) -> Result<Summary> {
    if !dest.is_dir() {
        return Err(Error::Invalid(format!(
            "{} is not a folder",
            dest.display()
        )));
    }
    let (total, needed) = sizes(zips, dest)?;
    if let Some(free) = free_space(dest)
        && needed + SPARE_BYTES > free
    {
        return Err(Error::Invalid(format!(
            "Not enough space on the drive: the export needs {}, and {} is free",
            gigabytes(needed),
            gigabytes(free)
        )));
    }

    let mut report = Progress {
        zip: 0,
        zips: zips.len(),
        bytes: 0,
        total_bytes: total,
    };
    let mut summary = Summary::default();
    let mut last = Instant::now();
    let mut buf = vec![0u8; 1 << 20];
    progress(&report);

    for (z, path) in zips.iter().enumerate() {
        report.zip = z + 1;
        progress(&report);
        let mut archive = open(path)?;
        for i in 0..archive.len() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(summary);
            }
            let mut entry = archive.by_index(i).map_err(|e| zip_error(path, e))?;
            let Some(name) = entry.enclosed_name() else {
                summary.unsafe_names += 1;
                continue;
            };
            let target = dest.join(&name);
            if entry.is_dir() {
                fs::create_dir_all(&target)?;
                continue;
            }
            let size = entry.size();
            if fs::symlink_metadata(&target).is_ok() {
                summary.skipped += 1;
                report.bytes += size;
                continue;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut partial = target.clone().into_os_string();
            partial.push(".partial");
            let partial = PathBuf::from(partial);
            let written = (|| -> io::Result<bool> {
                let mut out = File::create(&partial)?;
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(false);
                    }
                    let n = entry.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n])?;
                    report.bytes += n as u64;
                    if last.elapsed() >= PROGRESS_EVERY {
                        last = Instant::now();
                        progress(&report);
                    }
                }
                if let Some(time) = entry.last_modified().and_then(zip_time) {
                    let _ = out.set_modified(time);
                }
                Ok(true)
            })();
            match written {
                Ok(true) => fs::rename(&partial, &target)?,
                Ok(false) => {
                    let _ = fs::remove_file(&partial);
                    return Ok(summary);
                }
                Err(e) => {
                    let _ = fs::remove_file(&partial);
                    return Err(Error::Invalid(format!(
                        "Could not unzip {} from {}: {e}",
                        name.display(),
                        path.display()
                    )));
                }
            }
            summary.files += 1;
        }
    }
    report.bytes = total;
    progress(&report);
    summary.completed = true;
    Ok(summary)
}

/// A zip entry's modified time (local time, as zips store it).
fn zip_time(t: zip::DateTime) -> Option<SystemTime> {
    use chrono::{Local, NaiveDate, TimeZone};
    let naive = NaiveDate::from_ymd_opt(t.year().into(), t.month().into(), t.day().into())?
        .and_hms_opt(t.hour().into(), t.minute().into(), t.second().into())?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(SystemTime::from)
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Free bytes on the drive holding `path`, when the system says.
#[cfg(windows)]
fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is a NUL-terminated path; the null pointers are optional outputs.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

#[cfg(not(windows))]
fn free_space(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, data) in files {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    fn run(zips: &[PathBuf], dest: &Path) -> Summary {
        extract(zips, dest, &AtomicBool::new(false), &|_| {}).unwrap()
    }

    #[test]
    fn parts_merge_into_one_folder_and_rerun_skips() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.zip"), dir.path().join("b.zip"));
        make_zip(&a, &[("Takeout/Google Photos/Trip/IMG_1.jpg", b"photo")]);
        make_zip(
            &b,
            &[
                ("Takeout/Google Photos/Trip/IMG_1.jpg.json", b"{}"),
                ("../evil.txt", b"x"),
            ],
        );
        let dest = dir.path().join("out");
        fs::create_dir(&dest).unwrap();
        let zips = [a, b];

        let s = run(&zips, &dest);
        assert_eq!(
            (s.files, s.skipped, s.unsafe_names, s.completed),
            (2, 0, 1, true)
        );
        let trip = dest.join("Takeout/Google Photos/Trip");
        assert_eq!(fs::read(trip.join("IMG_1.jpg")).unwrap(), b"photo");
        assert!(trip.join("IMG_1.jpg.json").is_file());
        assert!(!dir.path().join("evil.txt").exists());

        // A file already there is never overwritten.
        fs::write(trip.join("IMG_1.jpg"), b"mine").unwrap();
        let s = run(&zips, &dest);
        assert_eq!((s.files, s.skipped), (0, 2));
        assert_eq!(fs::read(trip.join("IMG_1.jpg")).unwrap(), b"mine");
    }

    #[test]
    fn cancel_leaves_no_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.zip");
        make_zip(&a, &[("x.jpg", &[7u8; 3 << 20])]);
        let dest = dir.path().join("out");
        fs::create_dir(&dest).unwrap();
        let cancel = AtomicBool::new(true);
        let s = extract(&[a], &dest, &cancel, &|_| {}).unwrap();
        assert!(!s.completed);
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    }

    #[test]
    fn not_a_zip_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.zip");
        fs::write(&bad, b"not a zip").unwrap();
        let err = extract(&[bad], dir.path(), &AtomicBool::new(false), &|_| {}).unwrap_err();
        assert!(err.to_string().contains("zip file"));
    }
}
