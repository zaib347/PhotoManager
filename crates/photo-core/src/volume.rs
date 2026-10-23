//! Volume identity: libraries are tied to a drive's identity, not its drive letter
//! or mount point, so a USB disk that comes back as F: instead of E: still matches.

use std::io;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// Stable identifier: the volume serial on Windows. Elsewhere the mount point
    /// for now; filesystem UUIDs on macOS/Linux land with platform hardening (M5).
    pub uuid: String,
    pub label: Option<String>,
    pub mount: PathBuf,
}

/// Finds the volume containing `path` and the path relative to its mount point,
/// using `/` separators.
pub fn identify(path: &Path) -> io::Result<(VolumeInfo, String)> {
    let path = std::path::absolute(path)?;
    let volume = imp::volume_of(&path)?;
    let skip = volume.mount.components().count();
    let rel = path
        .components()
        .skip(skip)
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    Ok((volume, rel))
}

/// Currently mounted volumes. Only populated where volumes can be enumerated by
/// identity (Windows); elsewhere callers fall back to checking the last mount path.
pub fn mounted() -> Vec<VolumeInfo> {
    imp::mounted()
}

#[cfg(windows)]
mod imp {
    use super::VolumeInfo;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::{
        GetLogicalDrives, GetVolumeInformationW, GetVolumePathNameW,
    };

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn from_wide(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    fn info(mount: &str) -> Option<VolumeInfo> {
        let root = wide(mount.as_ref());
        let mut label = [0u16; 261];
        let mut serial = 0u32;
        // SAFETY: buffers are valid for the lengths passed; optional outputs are null.
        let ok = unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                &mut serial,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        if ok == 0 {
            return None;
        }
        let label = from_wide(&label);
        Some(VolumeInfo {
            uuid: format!("win-serial:{serial:08X}"),
            label: (!label.is_empty()).then_some(label),
            mount: PathBuf::from(mount),
        })
    }

    pub fn volume_of(path: &Path) -> io::Result<VolumeInfo> {
        let input = wide(path.as_os_str());
        let mut buf = [0u16; 1024];
        // SAFETY: `input` is NUL-terminated and `buf` is valid for its length.
        let ok = unsafe { GetVolumePathNameW(input.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        info(&from_wide(&buf)).ok_or_else(io::Error::last_os_error)
    }

    pub fn mounted() -> Vec<VolumeInfo> {
        // SAFETY: no arguments; returns a bitmask of drive letters.
        let mask = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|i| mask & (1 << i) != 0)
            .filter_map(|i| info(&format!("{}:\\", (b'A' + i) as char)))
            .collect()
    }
}

#[cfg(unix)]
mod imp {
    use super::VolumeInfo;
    use std::io;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    pub fn volume_of(path: &Path) -> io::Result<VolumeInfo> {
        let dev = std::fs::metadata(path)?.dev();
        let mut mount = path;
        while let Some(parent) = mount.parent() {
            if std::fs::metadata(parent)?.dev() != dev {
                break;
            }
            mount = parent;
        }
        Ok(VolumeInfo {
            uuid: format!("mount:{}", mount.display()),
            label: mount.file_name().map(|n| n.to_string_lossy().into_owned()),
            mount: mount.to_path_buf(),
        })
    }

    pub fn mounted() -> Vec<VolumeInfo> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let (volume, rel) = identify(&nested).unwrap();
        assert!(rel.ends_with("a/b"), "{rel}");
        assert_eq!(
            volume.mount.join(&rel),
            std::path::absolute(&nested).unwrap()
        );
    }
}
