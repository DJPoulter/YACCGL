//! Check GitHub Releases for a newer YACCGL Flatpak and install it in place.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;

use crate::error::IoContext;
use crate::steam::process;
use crate::{Error, Result};

pub const FLATPAK_ID: &str = "io.github.DJPoulter.YACCGL";
pub const BUNDLE_NAME: &str = "yet-another-creature-collector-game-launcher.flatpak";
const REPO: &str = "DJPoulter/YACCGL";

#[derive(Debug, Clone)]
pub struct AvailableUpdate {
    pub tag: String,
    pub version: String,
    pub name: String,
    pub download_url: String,
    pub size: u64,
    pub prerelease: bool,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    name: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Newest published release (including betas) that ships the Flatpak bundle and is
/// newer than `current` (e.g. `env!("CARGO_PKG_VERSION")`).
pub fn check(agent: &ureq::Agent, current: &str) -> Result<Option<AvailableUpdate>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=15");
    let mut res = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| Error::Http(e.to_string()))?;
    if !(200..300).contains(&res.status().as_u16()) {
        return Err(Error::Http(format!("GitHub returned HTTP {}", res.status())));
    }
    let releases: Vec<GhRelease> = res.body_mut().read_json().map_err(|e| Error::Http(e.to_string()))?;

    for rel in releases {
        if rel.draft {
            continue;
        }
        let Some(asset) = rel.assets.iter().find(|a| a.name == BUNDLE_NAME) else {
            continue;
        };
        let version = rel.tag_name.trim_start_matches('v').to_owned();
        if !is_newer(&version, current) {
            continue;
        }
        let tag = rel.tag_name;
        let name = rel.name.unwrap_or_else(|| tag.clone());
        return Ok(Some(AvailableUpdate {
            tag,
            version,
            name,
            download_url: asset.browser_download_url.clone(),
            size: asset.size,
            prerelease: rel.prerelease,
        }));
    }
    Ok(None)
}

/// Download the Flatpak bundle to a temp file.
pub fn download(agent: &ureq::Agent, update: &AvailableUpdate, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).io_ctx(|| format!("creating {}", parent.display()))?;
    }
    let part = dest.with_extension("flatpak.part");
    let _ = fs::remove_file(&part);

    let mut res = agent
        .get(&update.download_url)
        .call()
        .map_err(|e| Error::Http(e.to_string()))?;
    if !(200..300).contains(&res.status().as_u16()) {
        return Err(Error::Http(format!("download returned HTTP {}", res.status())));
    }
    let mut body = res.body_mut().as_reader();
    let mut file = File::create(&part).io_ctx(|| format!("creating {}", part.display()))?;
    let mut buf = [0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| Error::Http(e.to_string()))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).io_ctx(|| format!("writing {}", part.display()))?;
        done += n as u64;
    }
    drop(file);
    if update.size > 0 && done != update.size {
        let _ = fs::remove_file(&part);
        return Err(Error::Http(format!(
            "download size mismatch: got {done} bytes, expected {}",
            update.size
        )));
    }
    fs::rename(&part, dest).io_ctx(|| format!("moving {}", dest.display()))
}

/// Install/upgrade the user Flatpak from a local `.flatpak` bundle (no uninstall needed).
pub fn install_bundle(bundle: &Path) -> Result<()> {
    if !bundle.is_file() {
        return Err(Error::Io {
            context: format!("{} not found", bundle.display()),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        });
    }
    let status = process::host_command("flatpak")
        .args(["install", "--user", "-y", &bundle.to_string_lossy()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .map_err(|e| Error::Io {
            context: "couldn't run flatpak install".into(),
            source: e,
        })?;
    if !status.success() {
        return Err(Error::Http(format!(
            "flatpak install failed (exit {:?}). Try: flatpak install --user {}",
            status.code(),
            bundle.display()
        )));
    }
    Ok(())
}

/// Cache path for a downloaded update bundle.
pub fn bundle_cache_path(version: &str) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("yet-another-creature-collector-game-launcher")
        .join(format!("yaccgl-{version}.flatpak"))
}

/// True when `candidate` is a newer version than `current` (e.g. `0.1.7-beta.2` > `0.1.7-beta`).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (c_num, c_pre) = split_ver(candidate);
    let (u_num, u_pre) = split_ver(current);
    match cmp_num(&c_num, &u_num) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (c_pre.as_deref(), u_pre.as_deref()) {
            // Same base: a release beats any prerelease.
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (None, None) => false,
            (Some(a), Some(b)) => a > b,
        },
    }
}

fn split_ver(s: &str) -> (Vec<u64>, Option<String>) {
    let s = s.trim().trim_start_matches('v');
    let (num, pre) = match s.split_once('-') {
        Some((n, p)) => (n, Some(p.to_ascii_lowercase())),
        None => (s, None),
    };
    let parts = num.split('.').filter_map(|p| p.parse().ok()).collect();
    (parts, pre)
}

fn cmp_num(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => {}
            o => return o,
        }
    }
    std::cmp::Ordering::Equal
}

/// Convenience: check → download → install. Caller should quit afterwards.
pub fn apply(agent: &ureq::Agent, update: &AvailableUpdate) -> Result<()> {
    let dest = bundle_cache_path(&update.version);
    download(agent, update, &dest)?;
    install_bundle(&dest)?;
    let _ = fs::remove_file(&dest);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.1.7", "0.1.6"));
        assert!(is_newer("0.1.7-beta.2", "0.1.7-beta"));
        assert!(is_newer("0.1.7-beta.2", "0.1.6"));
        assert!(is_newer("0.1.7", "0.1.7-beta.2"));
        assert!(!is_newer("0.1.6", "0.1.7"));
        assert!(!is_newer("0.1.7-beta", "0.1.7"));
        assert!(!is_newer("0.1.7", "0.1.7"));
    }
}
