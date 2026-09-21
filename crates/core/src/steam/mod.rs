//! Steam integration: locating Steam, adding the game as a non-Steam shortcut,
//! forcing a Proton version and installing library artwork.
//!
//! Steam rewrites `shortcuts.vdf` and `config.vdf` from memory on exit, so every
//! write here must happen while Steam is closed (see [`process`]).

pub mod artwork;
pub mod binary_vdf;
pub mod process;
pub mod text_vdf;

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::IoContext;
use crate::{Error, Result};

/// Proton 10 is the only version Aniimo currently runs on (newer ones crash).
pub const DEFAULT_COMPAT_TOOL: &str = "proton_10";

const STEAMID64_BASE: u64 = 76_561_197_960_265_728;
const FLATPAK_STEAM_ID: &str = "com.valvesoftware.Steam";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Native,
    Flatpak,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Steam {
    pub root: PathBuf,
    pub flavor: Flavor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// 32-bit account id, which is also the `userdata/<id>` directory name.
    pub account_id: u32,
    pub persona: Option<String>,
    pub most_recent: bool,
}

#[derive(Debug, Clone)]
pub struct ShortcutSpec {
    pub name: String,
    pub exe: PathBuf,
    pub start_dir: PathBuf,
    pub launch_options: String,
    /// Internal compat tool name such as `proton_10`, or `None` to leave Steam's default.
    pub compat_tool: Option<String>,
    pub artwork: bool,
}

#[derive(Debug, Clone)]
pub struct Registered {
    pub appid: u32,
    pub created: bool,
    pub artwork: Vec<PathBuf>,
    /// Set when artwork was requested but could not be fetched.
    pub artwork_error: Option<String>,
}

impl Steam {
    /// All Steam installations found for the current user, native first.
    pub fn detect() -> Vec<Steam> {
        let Some(home) = dirs::home_dir() else { return Vec::new() };
        Self::detect_in(&home)
    }

    pub fn detect_in(home: &Path) -> Vec<Steam> {
        let candidates = [
            (home.join(".steam/steam"), Flavor::Native),
            (home.join(".local/share/Steam"), Flavor::Native),
            (home.join(".steam/debian-installation"), Flavor::Native),
            (home.join(".var/app").join(FLATPAK_STEAM_ID).join(".local/share/Steam"), Flavor::Flatpak),
        ];
        let mut found: Vec<Steam> = Vec::new();
        for (path, flavor) in candidates {
            let Ok(root) = path.canonicalize() else { continue };
            let valid = root.join("userdata").is_dir() || root.join("config/config.vdf").is_file();
            if valid && !found.iter().any(|s| s.root == root) {
                found.push(Steam { root, flavor });
            }
        }
        found
    }

    pub fn config_vdf(&self) -> PathBuf {
        self.root.join("config/config.vdf")
    }

    pub fn userdata(&self, user: &User) -> PathBuf {
        self.root.join("userdata").join(user.account_id.to_string())
    }

    pub fn shortcuts_vdf(&self, user: &User) -> PathBuf {
        self.userdata(user).join("config/shortcuts.vdf")
    }

    pub fn grid_dir(&self, user: &User) -> PathBuf {
        self.userdata(user).join("config/grid")
    }

    /// Users that have a `userdata` directory, most recently logged in first.
    pub fn users(&self) -> Vec<User> {
        let mut logins = std::collections::HashMap::new();
        if let Ok(src) = fs::read_to_string(self.root.join("config/loginusers.vdf"))
            && let Some(users) = text_vdf::parse(&src).ok().and_then(|r| r.get_obj("users").cloned())
        {
            for (id64, v) in users.0 {
                let (Ok(id64), text_vdf::Value::Obj(o)) = (id64.parse::<u64>(), v) else { continue };
                let Some(account) = id64.checked_sub(STEAMID64_BASE) else { continue };
                let persona = o.get_str("PersonaName").map(str::to_owned);
                let recent = o.get_str("MostRecent") == Some("1");
                let stamp = o.get_str("Timestamp").and_then(|t| t.parse::<u64>().ok()).unwrap_or(0);
                logins.insert(account as u32, (persona, recent, stamp));
            }
        }

        let mut users: Vec<(User, u64)> = fs::read_dir(self.root.join("userdata"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let id: u32 = e.file_name().to_str()?.parse().ok()?;
                (id != 0 && e.path().is_dir()).then_some((id, e.path()))
            })
            .map(|(id, path)| {
                let (persona, most_recent, stamp) = logins.get(&id).cloned().unwrap_or_default();
                // Fall back to how recently Steam touched this user's config.
                let stamp = if stamp > 0 { stamp } else { mtime_secs(&path.join("config/localconfig.vdf")) };
                (User { account_id: id, persona, most_recent }, stamp)
            })
            .collect();
        users.sort_by(|(a, sa), (b, sb)| b.most_recent.cmp(&a.most_recent).then(sb.cmp(sa)));
        users.into_iter().map(|(u, _)| u).collect()
    }

    /// Custom compat tools (e.g. GE-Proton) installed in `compatibilitytools.d`,
    /// as `(internal name, display name)`.
    pub fn custom_compat_tools(&self) -> Vec<(String, String)> {
        let mut tools = Vec::new();
        for entry in fs::read_dir(self.root.join("compatibilitytools.d")).into_iter().flatten().flatten() {
            let Ok(src) = fs::read_to_string(entry.path().join("compatibilitytool.vdf")) else { continue };
            let Ok(root) = text_vdf::parse(&src) else { continue };
            let Some(list) = root.get_obj("compatibilitytools").and_then(|o| o.get_obj("compat_tools")) else {
                continue;
            };
            for (internal, v) in &list.0 {
                let display = match v {
                    text_vdf::Value::Obj(o) => o.get_str("display_name").unwrap_or(internal),
                    text_vdf::Value::Str(_) => internal,
                };
                tools.push((internal.clone(), display.to_owned()));
            }
        }
        tools.sort_by(|a, b| b.1.cmp(&a.1));
        tools
    }

    /// Look up an existing shortcut for this game by its app id.
    pub fn shortcut_exists(&self, user: &User, appid: u32) -> bool {
        let Ok(data) = fs::read(self.shortcuts_vdf(user)) else { return false };
        let Ok(root) = binary_vdf::parse(&data) else { return false };
        root.get_obj("shortcuts")
            .is_some_and(|s| s.0.iter().any(|(_, v)| entry_appid(v) == Some(appid)))
    }

    /// Current compat tool forced for `appid`, if any.
    pub fn compat_tool(&self, appid: u32) -> Option<String> {
        let src = fs::read_to_string(self.config_vdf()).ok()?;
        let root = text_vdf::parse(&src).ok()?;
        compat_mapping(&root)?.get_obj(&appid.to_string())?.get_str("name").map(str::to_owned)
    }

    /// Add or update the shortcut, set its compat tool and artwork.
    /// Steam must not be running.
    pub fn register(&self, agent: &ureq::Agent, user: &User, spec: &ShortcutSpec) -> Result<Registered> {
        let exe = quote(&spec.exe);
        let appid = shortcut_appid(&exe, &spec.name);

        let mut artwork = Vec::new();
        let mut artwork_error = None;
        let mut icon = String::new();
        if spec.artwork {
            match artwork::install(agent, &self.grid_dir(user), appid) {
                Ok(w) => {
                    icon = w.icon.map(|p| p.display().to_string()).unwrap_or_default();
                    artwork = w.files;
                }
                Err(e) => artwork_error = Some(e.to_string()),
            }
        }

        let created = self.write_shortcut(user, appid, &exe, spec, &icon)?;
        if let Some(tool) = &spec.compat_tool {
            self.set_compat_tool(appid, Some(tool))?;
        }
        Ok(Registered { appid, created, artwork, artwork_error })
    }

    /// Remove the shortcut, its compat tool mapping and artwork. Steam must not be running.
    pub fn unregister(&self, user: &User, appid: u32) -> Result<()> {
        let path = self.shortcuts_vdf(user);
        if let Ok(data) = fs::read(&path) {
            let mut root = parse_shortcuts(&path, &data)?;
            let list = root.obj_mut("shortcuts");
            list.0.retain(|(_, v)| entry_appid(v) != Some(appid));
            renumber(list);
            write_backed_up(&path, &binary_vdf::serialize(&root))?;
        }
        self.set_compat_tool(appid, None)?;
        if let Ok(entries) = fs::read_dir(self.grid_dir(user)) {
            let ours = ["", "p", "_hero", "_logo", "_icon"].map(|suffix| format!("{appid}{suffix}"));
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let stem = name.split('.').next().unwrap_or("");
                if ours.iter().any(|o| o == stem) {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
        Ok(())
    }

    fn write_shortcut(&self, user: &User, appid: u32, exe: &str, spec: &ShortcutSpec, icon: &str) -> Result<bool> {
        let path = self.shortcuts_vdf(user);
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).io_ctx(|| format!("reading {}", path.display())),
        };
        let mut root = parse_shortcuts(&path, &data)?;
        let list = root.obj_mut("shortcuts");

        let existing = list.0.iter().position(|(_, v)| entry_appid(v) == Some(appid));
        let created = existing.is_none();
        let idx = existing.unwrap_or_else(|| {
            list.0.push((Vec::new(), binary_vdf::Value::Obj(new_entry())));
            renumber(list);
            list.0.len() - 1
        });
        let binary_vdf::Value::Obj(entry) = &mut list.0[idx].1 else { unreachable!() };
        entry.set_int("appid", appid as i32);
        entry.set_str("AppName", &spec.name);
        entry.set_str("Exe", exe);
        entry.set_str("StartDir", &quote_dir(&spec.start_dir));
        entry.set_str("LaunchOptions", &spec.launch_options);
        if !icon.is_empty() || created {
            entry.set_str("icon", icon);
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).io_ctx(|| format!("creating {}", parent.display()))?;
        }
        write_backed_up(&path, &binary_vdf::serialize(&root))?;
        Ok(created)
    }

    /// Force (`Some`) or clear (`None`) the compat tool for `appid` in config.vdf.
    pub fn set_compat_tool(&self, appid: u32, tool: Option<&str>) -> Result<()> {
        let path = self.config_vdf();
        let src = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && tool.is_none() => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).io_ctx(|| format!("reading {}", path.display())),
        };
        let mut root = text_vdf::parse(&src).map_err(|message| Error::Vdf { file: path.display().to_string(), message })?;
        let mapping = root
            .obj_mut("InstallConfigStore")
            .obj_mut("Software")
            .obj_mut("Valve")
            .obj_mut("Steam")
            .obj_mut("CompatToolMapping");
        let key = appid.to_string();
        match tool {
            Some(tool) => {
                let entry = mapping.obj_mut(&key);
                entry.set_str("name", tool);
                entry.set_str("config", "");
                entry.set_str("priority", "250");
            }
            None => {
                if mapping.remove(&key).is_none() {
                    return Ok(());
                }
            }
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).io_ctx(|| format!("creating {}", parent.display()))?;
        }
        write_backed_up(&path, text_vdf::serialize(&root).as_bytes())
    }
}

/// Steam's id for a non-Steam shortcut: CRC32 of the quoted exe followed by the name,
/// with the high bit set. It names the Proton prefix and keys the compat mapping.
pub fn shortcut_appid(quoted_exe: &str, name: &str) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(quoted_exe.as_bytes());
    h.update(name.as_bytes());
    h.finalize() | 0x8000_0000
}

/// App id the shortcut for `exe` will get.
pub fn appid_for(exe: &Path, name: &str) -> u32 {
    shortcut_appid(&quote(exe), name)
}

fn quote(p: &Path) -> String {
    format!("\"{}\"", p.display())
}

fn quote_dir(p: &Path) -> String {
    let s = p.display().to_string();
    if s.ends_with('/') { format!("\"{s}\"") } else { format!("\"{s}/\"") }
}

fn compat_mapping(root: &text_vdf::Obj) -> Option<&text_vdf::Obj> {
    root.get_obj("InstallConfigStore")?
        .get_obj("Software")?
        .get_obj("Valve")?
        .get_obj("Steam")?
        .get_obj("CompatToolMapping")
}

fn parse_shortcuts(path: &Path, data: &[u8]) -> Result<binary_vdf::Obj> {
    binary_vdf::parse(data).map_err(|message| Error::Vdf { file: path.display().to_string(), message })
}

fn entry_appid(v: &binary_vdf::Value) -> Option<u32> {
    match v {
        binary_vdf::Value::Obj(o) => o.get_int("appid").map(|n| n as u32),
        _ => None,
    }
}

/// Shortcut entries are keyed "0", "1", ... in order.
fn renumber(list: &mut binary_vdf::Obj) {
    for (i, (k, _)) in list.0.iter_mut().enumerate() {
        *k = i.to_string().into_bytes();
    }
}

fn new_entry() -> binary_vdf::Obj {
    let mut e = binary_vdf::Obj::default();
    e.set_int("appid", 0);
    e.set_str("AppName", "");
    e.set_str("Exe", "");
    e.set_str("StartDir", "");
    e.set_str("icon", "");
    e.set_str("ShortcutPath", "");
    e.set_str("LaunchOptions", "");
    e.set_int("IsHidden", 0);
    e.set_int("AllowDesktopConfig", 1);
    e.set_int("AllowOverlay", 1);
    e.set_int("OpenVR", 0);
    e.set_int("Devkit", 0);
    e.set_str("DevkitGameID", "");
    e.set_int("DevkitOverrideAppID", 0);
    e.set_int("LastPlayTime", 0);
    e.set_str("FlatpakAppID", "");
    e.obj_mut("tags");
    e
}

/// Keep a `.nacl-bak` copy of the previous contents, then replace the file atomically.
fn write_backed_up(path: &Path, data: &[u8]) -> Result<()> {
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".nacl-bak");
        fs::copy(path, &bak).io_ctx(|| format!("backing up {}", path.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".nacl-tmp");
    fs::write(&tmp, data).io_ctx(|| format!("writing {}", path.display()))?;
    fs::rename(&tmp, path).io_ctx(|| format!("replacing {}", path.display()))
}

fn mtime_secs(p: &Path) -> u64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_steam(home: &Path) -> Steam {
        let root = home.join(".local/share/Steam");
        fs::create_dir_all(root.join("userdata/12345/config")).unwrap();
        fs::create_dir_all(root.join("userdata/0")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/loginusers.vdf"),
            "\"users\"\n{\n\t\"76561197960278073\"\n\t{\n\t\t\"AccountName\"\t\t\"deck\"\n\t\t\"PersonaName\"\t\t\"Deck User\"\n\t\t\"MostRecent\"\t\t\"1\"\n\t}\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("config/config.vdf"),
            "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"AutoUpdateWindowEnabled\"\t\t\"0\"\n\t\t\t}\n\t\t}\n\t}\n}\n",
        )
        .unwrap();
        Steam { root: root.canonicalize().unwrap(), flavor: Flavor::Native }
    }

    fn spec(home: &Path) -> ShortcutSpec {
        ShortcutSpec {
            name: "Aniimo".into(),
            exe: home.join("Games/Aniimo/Aniimo.exe"),
            start_dir: home.join("Games/Aniimo"),
            launch_options: String::new(),
            compat_tool: Some(DEFAULT_COMPAT_TOOL.into()),
            artwork: false,
        }
    }

    #[test]
    fn appid_matches_known_value() {
        // Cross-checked with Python: zlib.crc32(b'"/home/deck/Games/Aniimo/Aniimo.exe"Aniimo') | 0x80000000
        let id = shortcut_appid("\"/home/deck/Games/Aniimo/Aniimo.exe\"", "Aniimo");
        assert_eq!(id, 3_039_192_195);
    }

    #[test]
    fn detects_install_and_users() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        let found = Steam::detect_in(home.path());
        assert_eq!(found, vec![steam.clone()]);
        let users = steam.users();
        assert_eq!(users.len(), 1, "userdata/0 must be ignored");
        assert_eq!(users[0].account_id, 12345);
        assert_eq!(users[0].persona.as_deref(), Some("Deck User"));
        assert!(users[0].most_recent);
    }

    #[test]
    fn register_is_idempotent_and_preserves_other_entries() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        let user = steam.users().remove(0);

        // A pre-existing shortcut from the user that must survive untouched.
        let mut root = binary_vdf::Obj::default();
        let mut other = new_entry();
        other.set_int("appid", 0x9000_0001u32 as i32);
        other.set_str("AppName", "Other Game");
        root.obj_mut("shortcuts").0.push((b"0".to_vec(), binary_vdf::Value::Obj(other.clone())));
        fs::write(steam.shortcuts_vdf(&user), binary_vdf::serialize(&root)).unwrap();

        let agent = crate::http::agent();
        let first = steam.register(&agent, &user, &spec(home.path())).unwrap();
        assert!(first.created);
        let second = steam.register(&agent, &user, &spec(home.path())).unwrap();
        assert!(!second.created);
        assert_eq!(first.appid, second.appid);

        let data = fs::read(steam.shortcuts_vdf(&user)).unwrap();
        let parsed = binary_vdf::parse(&data).unwrap();
        let list = parsed.get_obj("shortcuts").unwrap();
        assert_eq!(list.0.len(), 2);
        assert_eq!(list.get_obj("0").unwrap(), &other);
        let ours = list.get_obj("1").unwrap();
        assert_eq!(ours.get_str("AppName").as_deref(), Some("Aniimo"));
        let exe = format!("\"{}\"", home.path().join("Games/Aniimo/Aniimo.exe").display());
        assert_eq!(ours.get_str("Exe"), Some(exe));
        assert!(ours.get_str("StartDir").unwrap().ends_with("/Aniimo/\""));

        assert!(steam.shortcut_exists(&user, first.appid));
        assert_eq!(steam.compat_tool(first.appid).as_deref(), Some("proton_10"));
        // Existing lowercase "valve" key was reused rather than duplicated.
        let cfg = fs::read_to_string(steam.config_vdf()).unwrap();
        assert!(!cfg.contains("\"Valve\""));
        assert!(cfg.contains("AutoUpdateWindowEnabled"));
        assert!(steam.config_vdf().with_extension("vdf.nacl-bak").exists());
    }

    #[test]
    fn unregister_removes_everything() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        let user = steam.users().remove(0);
        let reg = steam.register(&crate::http::agent(), &user, &spec(home.path())).unwrap();
        fs::create_dir_all(steam.grid_dir(&user)).unwrap();
        fs::write(steam.grid_dir(&user).join(format!("{}p.jpg", reg.appid)), b"x").unwrap();
        fs::write(steam.grid_dir(&user).join("999p.jpg"), b"x").unwrap();

        steam.unregister(&user, reg.appid).unwrap();
        assert!(!steam.shortcut_exists(&user, reg.appid));
        assert_eq!(steam.compat_tool(reg.appid), None);
        assert!(!steam.grid_dir(&user).join(format!("{}p.jpg", reg.appid)).exists());
        assert!(steam.grid_dir(&user).join("999p.jpg").exists());
    }

    #[test]
    fn finds_custom_compat_tools() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        let dir = steam.root.join("compatibilitytools.d/GE-Proton10-15");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("compatibilitytool.vdf"),
            "\"compatibilitytools\"\n{\n  \"compat_tools\"\n  {\n    \"GE-Proton10-15\"\n    {\n      \"install_path\" \".\"\n      \"display_name\" \"GE-Proton10-15\"\n      \"from_oslist\" \"windows\"\n      \"to_oslist\" \"linux\"\n    }\n  }\n}\n",
        )
        .unwrap();
        assert_eq!(steam.custom_compat_tools(), vec![("GE-Proton10-15".into(), "GE-Proton10-15".into())]);
    }

    #[test]
    fn creates_config_when_missing() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        fs::remove_file(steam.config_vdf()).unwrap();
        steam.set_compat_tool(0x8000_1234, Some("proton_10")).unwrap();
        assert_eq!(steam.compat_tool(0x8000_1234).as_deref(), Some("proton_10"));
    }
}
