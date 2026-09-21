//! Installing, updating and inspecting a game installation.
//!
//! State lives inside the install directory (`.nacl/state.json`), so pointing the
//! launcher at an existing folder picks up what is already there.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::api::GamePackage;
use crate::download::{self, Transfer};
use crate::error::IoContext;
use crate::{Error, Result, extract, space};

const STATE_DIR: &str = ".nacl";
const STATE_FILE: &str = "state.json";
const CACHE_DIR: &str = "cache";

/// Unpacked client size is about 2.8x the archive; keep extra headroom.
const UNPACK_FACTOR: u64 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallState {
    pub package: GamePackage,
    /// Unix timestamp of the last successful install/update.
    pub installed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    UpToDate(InstallState),
    UpdateAvailable { installed: InstallState, latest: GamePackage },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Downloading { done: u64, total: Option<u64> },
    Verifying { done: u64, total: u64 },
    Extracting { done: u64, total: u64 },
}

pub fn read_state(dir: &Path) -> Option<InstallState> {
    let data = fs::read(dir.join(STATE_DIR).join(STATE_FILE)).ok()?;
    let state: InstallState = serde_json::from_slice(&data).ok()?;
    // A state file without the game next to it (e.g. files deleted by hand) doesn't count.
    dir.join(state.package.exe_name()).is_file().then_some(state)
}

pub fn status(dir: &Path, latest: &GamePackage) -> Status {
    match read_state(dir) {
        None => Status::NotInstalled,
        Some(s) if s.package.id == latest.id && s.package.md5 == latest.md5 => Status::UpToDate(s),
        Some(installed) => Status::UpdateAvailable { installed, latest: latest.clone() },
    }
}

pub fn exe_path(dir: &Path, package: &GamePackage) -> PathBuf {
    dir.join(package.exe_name())
}

/// Bytes needed on disk to download and unpack `package`.
pub fn required_space(package: &GamePackage) -> u64 {
    package.ext.archive_size.unwrap_or(400_000_000) * (1 + UNPACK_FACTOR)
}

/// Download and unpack `package` into `dir`. Used for fresh installs, updates and repairs.
/// A partial download is resumed on the next call.
pub fn install(
    agent: &ureq::Agent,
    dir: &Path,
    package: &GamePackage,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Progress),
) -> Result<InstallState> {
    fs::create_dir_all(dir).io_ctx(|| format!("creating {}", dir.display()))?;
    let cache = dir.join(STATE_DIR).join(CACHE_DIR);
    fs::create_dir_all(&cache).io_ctx(|| format!("creating {}", cache.display()))?;
    let archive = cache.join(format!("{}.7z", package.md5.to_ascii_lowercase()));
    remove_stale_downloads(&cache, &archive);

    let already = fs::metadata(download::part_path(&archive)).map_or(0, |m| m.len())
        + fs::metadata(&archive).map_or(0, |m| m.len());
    let needed = required_space(package).saturating_sub(already);
    if let Some(free) = space::available(dir)
        && free < needed
    {
        return Err(Error::Io {
            context: format!(
                "not enough free space in {}: {} needed, {} available",
                dir.display(),
                space::human(needed),
                space::human(free)
            ),
            source: std::io::Error::from(std::io::ErrorKind::StorageFull),
        });
    }

    let urls = package.download_urls();
    let req = download::Request {
        urls: &urls,
        dest: &archive,
        md5: &package.md5,
        size: package.ext.archive_size,
    };
    download::fetch(agent, &req, cancel, &mut |t| {
        on(match t {
            Transfer::Downloading { done, total } => Progress::Downloading { done, total },
            Transfer::Verifying { done, total } => Progress::Verifying { done, total },
        })
    })?;

    extract::extract_7z(&archive, dir, cancel, &mut |done, total| on(Progress::Extracting { done, total }))?;

    let exe = exe_path(dir, package);
    if !exe.is_file() {
        return Err(Error::Extract(format!("{} is missing after extraction", exe.display())));
    }

    let state = InstallState {
        package: package.clone(),
        installed_at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
    };
    let state_path = dir.join(STATE_DIR).join(STATE_FILE);
    fs::write(&state_path, serde_json::to_vec_pretty(&state)?)
        .io_ctx(|| format!("writing {}", state_path.display()))?;
    let _ = fs::remove_file(&archive);
    Ok(state)
}

/// Delete downloads for other versions so an abandoned update doesn't waste space.
fn remove_stale_downloads(cache: &Path, keep: &Path) {
    let keep_part = download::part_path(keep);
    for entry in fs::read_dir(cache).into_iter().flatten().flatten() {
        let p = entry.path();
        if p != keep && p != keep_part {
            let _ = fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::PackageExt;

    fn pkg(id: u64, md5: &str) -> GamePackage {
        GamePackage {
            id,
            version_number: "1.0.0.3".into(),
            md5: md5.into(),
            url: String::new(),
            back_url: String::new(),
            exe_start_name: "Aniimo.exe".into(),
            package_file_size: None,
            ext: PackageExt { archive_size: Some(100) },
        }
    }

    fn write_state(dir: &Path, p: &GamePackage) {
        fs::create_dir_all(dir.join(STATE_DIR)).unwrap();
        let s = InstallState { package: p.clone(), installed_at: 1 };
        fs::write(dir.join(STATE_DIR).join(STATE_FILE), serde_json::to_vec(&s).unwrap()).unwrap();
    }

    #[test]
    fn status_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = pkg(1, "aa");
        let v2 = pkg(2, "bb");
        assert_eq!(status(dir.path(), &v1), Status::NotInstalled);

        write_state(dir.path(), &v1);
        assert_eq!(status(dir.path(), &v1), Status::NotInstalled, "exe missing");

        fs::write(dir.path().join("Aniimo.exe"), b"").unwrap();
        assert!(matches!(status(dir.path(), &v1), Status::UpToDate(_)));
        assert!(matches!(status(dir.path(), &v2), Status::UpdateAvailable { .. }));
    }

    #[test]
    fn stale_downloads_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let keep = dir.path().join("new.7z");
        fs::write(dir.path().join("old.7z.part"), b"x").unwrap();
        fs::write(download::part_path(&keep), b"x").unwrap();
        remove_stale_downloads(dir.path(), &keep);
        assert!(!dir.path().join("old.7z.part").exists());
        assert!(download::part_path(&keep).exists());
    }
}
