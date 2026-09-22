//! Check GitHub Releases for a newer YACCGL Flatpak and install it in place.
//!
//! Uses `releases.atom` + `github.com/.../releases/download/...` instead of the
//! REST API, so unauthenticated clients aren't blocked by the 60 req/hour rate limit.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

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

/// Newest published release that ships the Flatpak bundle and is newer than `current`
/// (e.g. `env!("CARGO_PKG_VERSION")`).
pub fn check(agent: &ureq::Agent, current: &str) -> Result<Option<AvailableUpdate>> {
    let url = format!("https://github.com/{REPO}/releases.atom");
    let mut res = agent
        .get(&url)
        .header("Accept", "application/atom+xml, application/xml, text/xml;q=0.9, */*;q=0.8")
        .call()
        .map_err(|e| Error::Http(e.to_string()))?;
    let status = res.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(Error::Http(format!("GitHub releases feed returned HTTP {status}")));
    }
    let body = res.body_mut().read_to_string().map_err(|e| Error::Http(e.to_string()))?;

    for entry in atom_entries(&body) {
        let version = entry.tag.trim_start_matches('v').to_owned();
        if !is_newer(&version, current) {
            continue;
        }
        let tag = entry.tag;
        let name = entry.title.unwrap_or_else(|| tag.clone());
        let prerelease = version.contains('-');
        return Ok(Some(AvailableUpdate {
            download_url: format!("https://github.com/{REPO}/releases/download/{tag}/{BUNDLE_NAME}"),
            tag,
            version,
            name,
            size: 0, // unknown without the REST API; download skips the size check
            prerelease,
        }));
    }
    Ok(None)
}

struct AtomEntry {
    tag: String,
    title: Option<String>,
}

/// Pull release tags from a GitHub releases Atom feed (newest first).
fn atom_entries(xml: &str) -> Vec<AtomEntry> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<entry>") {
        let after = &rest[start + 7..];
        let Some(end) = after.find("</entry>") else { break };
        let entry = &after[..end];
        rest = &after[end + 8..];

        let tag = entry_tag(entry);
        let Some(tag) = tag else { continue };
        let title = xml_tag_text(entry, "title");
        out.push(AtomEntry { tag, title });
    }
    out
}

fn entry_tag(entry: &str) -> Option<String> {
    // <id>tag:github.com,2008:Repository/123/v0.1.10</id>
    if let Some(id) = xml_tag_text(entry, "id") {
        if let Some(tag) = id.rsplit('/').next() {
            if tag.starts_with('v') || tag.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return Some(tag.to_owned());
            }
        }
    }
    // <link ... href="https://github.com/DJPoulter/YACCGL/releases/tag/v0.1.10"/>
    for part in entry.split("href=\"").skip(1) {
        let href = part.split('"').next().unwrap_or("");
        if let Some(tag) = href.rsplit("/tag/").nth(1) {
            let tag = tag.trim_end_matches('/');
            if !tag.is_empty() {
                return Some(tag.to_owned());
            }
        }
    }
    None
}

fn xml_tag_text(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = block.find(&open)?;
    let after_open = &block[start + open.len()..];
    let text_start = after_open.find('>')? + 1;
    let text = &after_open[text_start..];
    let end = text.find(&close)?;
    Some(text[..end].trim().to_owned())
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
    if done == 0 {
        let _ = fs::remove_file(&part);
        return Err(Error::Http("download was empty".into()));
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

    #[test]
    fn parses_releases_atom() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>tag:github.com,2008:Repository/1380416239/v0.1.10</id>
    <link rel="alternate" href="https://github.com/DJPoulter/YACCGL/releases/tag/v0.1.10"/>
    <title>YACCGL 0.1.10</title>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/1380416239/v0.1.9</id>
    <title>YACCGL 0.1.9</title>
  </entry>
</feed>"#;
        let entries = atom_entries(xml);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].tag, "v0.1.10");
        assert_eq!(entries[0].title.as_deref(), Some("YACCGL 0.1.10"));
        assert_eq!(entries[1].tag, "v0.1.9");
    }
}
