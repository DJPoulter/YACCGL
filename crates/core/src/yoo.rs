//! YooAsset cache verification for Aniimo's `DefaultPackage`.
//!
//! The game downloads ~20–40 GB of asset bundles. Stock YooAsset keeps them under
//! `Aniimo_Data/Sandbox/CacheFiles/DefaultPackage/BundleFiles/{hash[0:2]}/{hash}/__data`.
//! Under Proton the same tree can also appear in the Wine prefix
//! (`…/compatdata/<appid>/pfx/drive_c/users/…/AppData/LocalLow/Aniimo/Aniimo/Sandbox/…`).
//! FunPlus ships a PackageManifest that still advertises YooAsset 1.4.17 but uses a compact
//! bundle table (u8 names, raw MD5 + CRC32).

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::IoContext;
use crate::{Error, Result, GAME_NAME};

const MAGIC: u32 = 0x594F4F; // "YOO\0"
const DATA_FILE: &str = "__data";

/// Built-in manifest under StreamingAssets (always present after install).
const STREAMING_PACKAGE_DIR: &str =
    "Aniimo_Data/StreamingAssets/cvs/res/uab/win/DefaultPackage";
/// Runtime cache + optional refreshed manifests (created on first game launch).
const SANDBOX_MANIFEST_DIR: &str = "Aniimo_Data/Sandbox/ManifestFiles";
const SANDBOX_BUNDLE_FILES: &str =
    "Aniimo_Data/Sandbox/CacheFiles/DefaultPackage/BundleFiles";
const LOCALLOW_BUNDLE_FILES: &str =
    "drive_c/users/{user}/AppData/LocalLow/Aniimo/Aniimo/Sandbox/CacheFiles/DefaultPackage/BundleFiles";

#[derive(Debug, Clone)]
pub struct Bundle {
    pub name: String,
    /// 32 lowercase hex chars (MD5). Also the cache folder name (CacheGUID).
    pub file_hash: String,
    /// 8 lowercase hex chars (CRC32 as little-endian bytes, YooAsset style).
    pub file_crc: String,
    pub file_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleStatus {
    Ok,
    Missing,
    SizeMismatch { actual: u64 },
    CrcMismatch { actual: String },
    Unreadable,
}

#[derive(Debug, Clone)]
pub struct CheckedBundle {
    pub bundle: Bundle,
    pub status: BundleStatus,
    /// Directory containing `__data` / `__info`, when it exists.
    pub cache_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub package_name: String,
    pub package_version: String,
    pub checked: Vec<CheckedBundle>,
    /// Where cache entries were read from, if any tree was found.
    pub cache_root: Option<PathBuf>,
    /// Number of `__data` folders present under `cache_root` (any GUID).
    pub cache_entries_on_disk: usize,
}

impl VerifyReport {
    pub fn bad(&self) -> impl Iterator<Item = &CheckedBundle> {
        self.checked.iter().filter(|c| c.status != BundleStatus::Ok)
    }

    pub fn ok_count(&self) -> usize {
        self.checked.iter().filter(|c| c.status == BundleStatus::Ok).count()
    }

    pub fn missing_count(&self) -> usize {
        self.checked.iter().filter(|c| c.status == BundleStatus::Missing).count()
    }

    /// Short user-facing summary for toasts / CLI.
    pub fn summary(&self) -> String {
        let total = self.checked.len();
        let bad: Vec<_> = self.bad().collect();
        if bad.is_empty() {
            return format!("All {total} asset bundles look good");
        }

        let missing = bad.iter().filter(|b| b.status == BundleStatus::Missing).count();
        let corrupt = bad.len() - missing;

        // No cache tree at all — game has never finished downloading world data.
        if self.cache_root.is_none() || (self.cache_entries_on_disk == 0 && missing == total) {
            return format!(
                "No downloaded game cache found yet ({total} bundles). Launch Aniimo once to download world data, then Verify again."
            );
        }

        // Cache exists but nothing matches the manifest GUIDs — wrong tree or hash layout.
        if self.cache_entries_on_disk > 0 && self.ok_count() == 0 && missing == total {
            return format!(
                "Found {} cache file(s) under {}, but none match the manifest. The cache path or hash layout may be wrong.",
                self.cache_entries_on_disk,
                self.cache_root
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(unknown)".into())
            );
        }

        let mut msg = format!("{} of {total} bundles need attention", bad.len());
        if corrupt > 0 {
            msg.push_str(&format!(" ({corrupt} corrupt removed)"));
        }
        if missing > 0 {
            msg.push_str(&format!(" ({missing} not downloaded yet)"));
        }
        msg.push_str(". Launch the game to re-download.");
        msg
    }
}

/// Locate and parse the DefaultPackage manifest, then check every cached bundle.
///
/// `on(done_bytes, total_bytes)` is called as files are hashed. When `repair` is true,
/// cache folders that fail size/CRC checks are deleted so the game can re-download them.
pub fn verify(
    install_dir: &Path,
    repair: bool,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(u64, u64),
) -> Result<VerifyReport> {
    let (package_name, package_version, bundles) = load_bundles(install_dir)?;
    let (cache_root, index) = resolve_cache(install_dir);
    let cache_entries_on_disk = index.len();
    let total: u64 = bundles.iter().map(|b| b.file_size.max(1)).sum();
    let mut done = 0u64;
    on(0, total);

    let mut checked = Vec::with_capacity(bundles.len());
    for bundle in bundles {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let cache_dir = index
            .get(&bundle.file_hash)
            .cloned()
            .unwrap_or_else(|| expected_cache_dir(cache_root.as_deref(), &bundle.file_hash));
        let data_path = cache_dir.join(DATA_FILE);
        let status = check_bundle(&bundle, &data_path, cancel)?;
        done = done.saturating_add(bundle.file_size.max(1));
        on(done.min(total), total);

        if repair
            && !matches!(status, BundleStatus::Ok | BundleStatus::Missing)
            && cache_dir.is_dir()
        {
            let _ = fs::remove_dir_all(&cache_dir);
        }

        checked.push(CheckedBundle { bundle, status, cache_dir });
    }

    Ok(VerifyReport {
        package_name,
        package_version,
        checked,
        cache_root,
        cache_entries_on_disk,
    })
}

fn expected_cache_dir(root: Option<&Path>, file_hash: &str) -> PathBuf {
    let folder = file_hash.get(..2).unwrap_or(file_hash);
    match root {
        Some(root) => root.join(folder).join(file_hash),
        None => PathBuf::from(folder).join(file_hash),
    }
}

fn check_bundle(bundle: &Bundle, data_path: &Path, cancel: &AtomicBool) -> Result<BundleStatus> {
    let meta = match fs::metadata(data_path) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return Ok(BundleStatus::Missing),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BundleStatus::Missing),
        Err(_) => return Ok(BundleStatus::Unreadable),
    };
    let actual_size = meta.len();
    if actual_size != bundle.file_size {
        return Ok(BundleStatus::SizeMismatch { actual: actual_size });
    }

    let actual_crc = match file_crc32_hex(data_path, cancel) {
        Ok(c) => c,
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(_) => return Ok(BundleStatus::Unreadable),
    };
    if actual_crc != bundle.file_crc {
        return Ok(BundleStatus::CrcMismatch { actual: actual_crc });
    }
    Ok(BundleStatus::Ok)
}

/// CRC32 as YooAsset `HashUtility.ToString`: lowercase hex of the little-endian CRC bytes.
fn file_crc32_hex(path: &Path, cancel: &AtomicBool) -> Result<String> {
    let file = File::open(path).io_ctx(|| format!("opening {}", path.display()))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut hasher = crc32fast::Hasher::new();
    let mut buf = [0u8; 256 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = reader.read(&mut buf).io_ctx(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize().to_le_bytes()))
}

/// Pick the fullest cache tree we can see and index GUID → folder.
fn resolve_cache(install_dir: &Path) -> (Option<PathBuf>, HashMap<String, PathBuf>) {
    let mut best: Option<(PathBuf, HashMap<String, PathBuf>)> = None;
    for root in cache_root_candidates(install_dir) {
        if !root.is_dir() {
            continue;
        }
        let index = index_cache_root(&root);
        let better = best.as_ref().is_none_or(|(_, prev)| index.len() > prev.len());
        if better {
            best = Some((root, index));
        }
    }
    match best {
        Some((root, index)) => (Some(root), index),
        None => (None, HashMap::new()),
    }
}

fn cache_root_candidates(install_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    out.push(install_dir.join(SANDBOX_BUNDLE_FILES));

    // Proton/Wine: Unity persistentDataPath under LocalLow (company/product = Aniimo).
    let exe = install_dir.join(crate::DEFAULT_EXE);
    let appid = crate::steam::appid_for(&exe, GAME_NAME);
    for steam in crate::steam::Steam::detect() {
        let pfx = steam.prefix_dir(appid);
        for user in ["steamuser", "deck", "user"] {
            out.push(pfx.join(LOCALLOW_BUNDLE_FILES.replace("{user}", user)));
        }
        // Any other Windows user profile under the prefix.
        let users = pfx.join("drive_c/users");
        if let Ok(rd) = fs::read_dir(users) {
            for ent in rd.flatten() {
                let name = ent.file_name();
                let name = name.to_string_lossy();
                if name.eq_ignore_ascii_case("Public") || name.eq_ignore_ascii_case("Default") {
                    continue;
                }
                out.push(ent.path().join(
                    "AppData/LocalLow/Aniimo/Aniimo/Sandbox/CacheFiles/DefaultPackage/BundleFiles",
                ));
            }
        }
    }

    // Last resort: find BundleFiles dirs under the install tree (cheap dir walk).
    out.extend(find_bundle_files_dirs(&install_dir.join("Aniimo_Data"), 6));
    out
}

fn find_bundle_files_dirs(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for ent in rd.flatten() {
            let Ok(ft) = ent.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let path = ent.path();
            if ent.file_name().eq_ignore_ascii_case("BundleFiles") {
                found.push(path.clone());
            }
            if depth < max_depth {
                // Skip known huge / irrelevant trees.
                let name = ent.file_name();
                let name = name.to_string_lossy();
                if name.eq_ignore_ascii_case("il2cpp_data")
                    || name.eq_ignore_ascii_case("Plugins")
                    || name.eq_ignore_ascii_case("Resources")
                {
                    continue;
                }
                stack.push((path, depth + 1));
            }
        }
    }
    found
}

/// Map lowercase 32-char GUID → cache folder (the directory that contains `__data`).
fn index_cache_root(root: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    let Ok(shards) = fs::read_dir(root) else {
        return map;
    };
    for shard in shards.flatten() {
        let Ok(ft) = shard.file_type() else { continue };
        let path = shard.path();
        if ft.is_dir() {
            // Sharded layout: BundleFiles/ab/abcd…/__data
            if let Ok(children) = fs::read_dir(&path) {
                for child in children.flatten() {
                    maybe_insert_cache_entry(&mut map, &child.path());
                }
            }
            // Unsharded: BundleFiles/<guid>/__data
            maybe_insert_cache_entry(&mut map, &path);
        }
    }
    map
}

fn maybe_insert_cache_entry(map: &mut HashMap<String, PathBuf>, dir: &Path) {
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let key = name.to_ascii_lowercase();
    if key.len() != 32 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return;
    }
    if dir.join(DATA_FILE).is_file() {
        map.entry(key).or_insert_with(|| dir.to_path_buf());
    }
}

fn load_bundles(install_dir: &Path) -> Result<(String, String, Vec<Bundle>)> {
    let path = find_manifest(install_dir)?;
    let data = fs::read(&path).io_ctx(|| format!("reading {}", path.display()))?;
    parse_manifest(&data)
}

fn find_manifest(install_dir: &Path) -> Result<PathBuf> {
    // Prefer a Sandbox copy (updated by the game) when present.
    let mut dirs = vec![
        install_dir.join(SANDBOX_MANIFEST_DIR),
        install_dir.join(STREAMING_PACKAGE_DIR),
    ];
    // Proton LocalLow manifests
    let exe = install_dir.join(crate::DEFAULT_EXE);
    let appid = crate::steam::appid_for(&exe, GAME_NAME);
    for steam in crate::steam::Steam::detect() {
        let pfx = steam.prefix_dir(appid);
        for user in ["steamuser", "deck", "user"] {
            dirs.push(pfx.join(format!(
                "drive_c/users/{user}/AppData/LocalLow/Aniimo/Aniimo/Sandbox/ManifestFiles"
            )));
        }
    }

    for dir in dirs {
        if let Some(p) = newest_manifest_in(&dir) {
            return Ok(p);
        }
    }
    Err(Error::Yoo(format!(
        "no PackageManifest_DefaultPackage_*.bytes under {} or {}",
        install_dir.join(STREAMING_PACKAGE_DIR).display(),
        install_dir.join(SANDBOX_MANIFEST_DIR).display()
    )))
}

fn newest_manifest_in(dir: &Path) -> Option<PathBuf> {
    let rd = fs::read_dir(dir).ok()?;
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("PackageManifest_DefaultPackage_") || !name.ends_with(".bytes") {
            continue;
        }
        if name.contains("assetxbt") {
            continue;
        }
        let meta = entry.metadata().ok()?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if best.as_ref().is_none_or(|(t, _)| mtime >= *t) {
            best = Some((mtime, entry.path()));
        }
    }
    best.map(|(_, p)| p)
}

fn parse_manifest(data: &[u8]) -> Result<(String, String, Vec<Bundle>)> {
    let mut r = Reader::new(data);
    let magic = r.u32().map_err(|_| Error::Yoo("manifest too short".into()))?;
    if magic != MAGIC {
        return Err(Error::Yoo(format!("bad manifest magic {magic:#x}, expected {MAGIC:#x}")));
    }
    let file_version = r.utf8().map_err(|e| Error::Yoo(e))?;
    if !file_version.starts_with("1.4.") {
        return Err(Error::Yoo(format!("unsupported manifest version {file_version}")));
    }
    // EnableAddressable, LocationToLower, IncludeAssetGUID
    r.skip(3).map_err(|e| Error::Yoo(e))?;
    let _output_name_style = r.i32().map_err(|e| Error::Yoo(e))?;
    let package_name = r.utf8().map_err(|e| Error::Yoo(e))?;
    let package_version = r.utf8().map_err(|e| Error::Yoo(e))?;

    let bundles = find_and_parse_bundles(data).ok_or_else(|| {
        Error::Yoo("could not locate compact bundle table in PackageManifest".into())
    })?;

    Ok((package_name, package_version, bundles))
}

/// FunPlus compact bundle table: i32 count, then per bundle
/// `u8 nameLen | name | [16] md5 | [4] crc | i32 size | u16 refCount | i32 refs…`
/// Sitting after the (large) asset list; discover via `.uab` name markers.
fn find_and_parse_bundles(data: &[u8]) -> Option<Vec<Bundle>> {
    let mut from = 0usize;
    while let Some(rel) = data[from..].windows(4).position(|w| w == b".uab") {
        let end = from + rel + 4;
        // Walk back a plausible ASCII name to a u8 length prefix, then to the table count.
        for nlen in (5usize..=200).rev() {
            if end < nlen + 5 {
                continue;
            }
            let name_start = end - nlen;
            if data[name_start - 1] as usize != nlen {
                continue;
            }
            if !data[name_start..end].iter().all(|b| (32..127).contains(b)) {
                continue;
            }
            let count_at = name_start - 1 - 4;
            if let Some(bundles) = parse_bundles_at(data, count_at) {
                return Some(bundles);
            }
        }
        from = end;
    }
    None
}

fn parse_bundles_at(data: &[u8], start: usize) -> Option<Vec<Bundle>> {
    let count = i32::from_le_bytes(data[start..start + 4].try_into().ok()?) as usize;
    let mut i = start + 4;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if i >= data.len() {
            return None;
        }
        let nlen = data[i] as usize;
        i += 1;
        if nlen == 0 || i + nlen + 16 + 4 + 4 + 2 > data.len() {
            return None;
        }
        let name = data[i..i + nlen].to_vec();
        i += nlen;
        if !name.iter().all(|b| (32..127).contains(b)) {
            return None;
        }
        let hash = &data[i..i + 16];
        i += 16;
        let crc = &data[i..i + 4];
        i += 4;
        let size = i32::from_le_bytes(data[i..i + 4].try_into().ok()?);
        i += 4;
        if !(0..500_000_000).contains(&size) {
            return None;
        }
        let nref = u16::from_le_bytes(data[i..i + 2].try_into().ok()?) as usize;
        i += 2;
        if nref > 10_000 || i + 4 * nref > data.len() {
            return None;
        }
        i += 4 * nref;
        out.push(Bundle {
            name: String::from_utf8(name).ok()?,
            file_hash: hex::encode(hash),
            file_crc: hex::encode(crc),
            file_size: size as u64,
        });
    }
    if i != data.len() {
        return None;
    }
    Some(out)
}

struct Reader<'a> {
    data: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, i: 0 }
    }

    fn need(&self, n: usize) -> std::result::Result<(), String> {
        if self.i + n > self.data.len() {
            Err("unexpected end of manifest".into())
        } else {
            Ok(())
        }
    }

    fn skip(&mut self, n: usize) -> std::result::Result<(), String> {
        self.need(n)?;
        self.i += n;
        Ok(())
    }

    fn u32(&mut self) -> std::result::Result<u32, String> {
        self.need(4)?;
        let v = u32::from_le_bytes(self.data[self.i..self.i + 4].try_into().unwrap());
        self.i += 4;
        Ok(v)
    }

    fn i32(&mut self) -> std::result::Result<i32, String> {
        self.need(4)?;
        let v = i32::from_le_bytes(self.data[self.i..self.i + 4].try_into().unwrap());
        self.i += 4;
        Ok(v)
    }

    fn u16(&mut self) -> std::result::Result<u16, String> {
        self.need(2)?;
        let v = u16::from_le_bytes(self.data[self.i..self.i + 2].try_into().unwrap());
        self.i += 2;
        Ok(v)
    }

    fn utf8(&mut self) -> std::result::Result<String, String> {
        let n = self.u16()? as usize;
        self.need(n)?;
        let s = std::str::from_utf8(&self.data[self.i..self.i + n])
            .map_err(|_| "invalid UTF-8 in manifest".to_string())?
            .to_string();
        self.i += n;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_aniimo_manifest_fixture() {
        let path = std::env::temp_dir()
            .join("yaccgl-re")
            .join("yoo")
            .join("PackageManifest_DefaultPackage_3544783.bytes");
        if !path.is_file() {
            eprintln!("skip: fixture missing at {}", path.display());
            return;
        }
        let data = fs::read(&path).unwrap();
        let (name, ver, bundles) = parse_manifest(&data).unwrap();
        assert_eq!(name, "DefaultPackage");
        assert_eq!(ver, "3544783");
        assert_eq!(bundles.len(), 2073);
        assert!(bundles.iter().all(|b| b.file_hash.len() == 32 && b.file_crc.len() == 8));
        let total: u64 = bundles.iter().map(|b| b.file_size).sum();
        assert!(total > 20 * 1024 * 1024 * 1024);
    }

    #[test]
    fn crc_hex_is_le_bytes() {
        // crc32("hello") = 0x3610a686 → LE bytes hex "86a61036"
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t");
        fs::write(&p, b"hello").unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(file_crc32_hex(&p, &cancel).unwrap(), "86a61036");
    }

    #[test]
    fn indexes_sharded_cache_layout() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("BundleFiles");
        let guid = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
        let folder = root.join(&guid[..2]).join(guid);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(DATA_FILE), b"x").unwrap();
        let idx = index_cache_root(&root);
        assert_eq!(idx.get(guid), Some(&folder));
    }

    #[test]
    fn resolve_prefers_populated_root() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path();
        fs::create_dir_all(install.join("Aniimo_Data")).unwrap();
        // Empty default sandbox root.
        fs::create_dir_all(install.join(SANDBOX_BUNDLE_FILES)).unwrap();
        // Populated alternate BundleFiles under Aniimo_Data.
        let alt = install.join("Aniimo_Data/Somewhere/BundleFiles");
        let guid = "0123456789abcdef0123456789abcdef";
        let folder = alt.join(&guid[..2]).join(guid);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(DATA_FILE), b"x").unwrap();

        let (root, index) = resolve_cache(install);
        assert_eq!(root.as_deref(), Some(alt.as_path()));
        assert_eq!(index.len(), 1);
    }
}
