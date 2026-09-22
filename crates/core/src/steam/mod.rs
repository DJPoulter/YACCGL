//! Steam integration: locating Steam, adding the game as a non-Steam shortcut,
//! forcing a Proton version and installing library artwork.
//!
//! Steam rewrites `shortcuts.vdf` and `config.vdf` from memory on exit, so every
//! write here must happen while Steam is closed (see [`process`]).

pub mod artwork;
pub mod binary_vdf;
pub mod process;
pub mod text_vdf;
pub mod wine_reg;

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::IoContext;
use crate::{Error, Result};

/// Proton 10 is the only version Aniimo currently runs on (newer ones crash).
pub const DEFAULT_COMPAT_TOOL: &str = "proton_10";

/// Wine drive letter that points at the install directory; see [`Steam::map_game_drive`].
pub const GAME_DRIVE: &str = "g:";

/// Single-window size used unless the user picks another: the Steam Deck's screen.
pub const DEFAULT_WINDOW_SIZE: (u32, u32) = (1280, 800);

/// Name of the Wine virtual desktop YACCGL configures; see [`Steam::set_single_window`].
/// Wine only applies the configured size to the desktop named "Default" (any other
/// name fills the screen), which is also the name `winetricks vd` uses.
const WINE_DESKTOP_NAME: &str = "Default";
const WINE_EXPLORER_KEY: &str = "Software\\Wine\\Explorer";
const WINE_DESKTOPS_KEY: &str = "Software\\Wine\\Explorer\\Desktops";

/// Outcome of a change to the game's Proton prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixChange {
    Applied,
    /// The prefix doesn't exist yet and can't be prepared; retry after the first launch.
    Pending,
}

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
    /// Single-window size (see [`Steam::set_single_window`]), or `None` to turn it off.
    pub single_window: Option<(u32, u32)>,
}

/// A command that starts the game the way Steam does; see [`Steam::game_launch`].
#[derive(Debug, Clone)]
pub struct GameLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub dir: PathBuf,
}

/// Result of [`Steam::with_closed`].
#[derive(Debug, Clone)]
pub struct Applied<T> {
    pub value: T,
    /// Steam was running, so it was closed for the change and started again.
    pub restarted: bool,
    /// After the restart, whether the change was still in place once Steam had loaded.
    /// `None` if Steam wasn't restarted or didn't come back in time.
    pub survived_restart: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Registered {
    pub appid: u32,
    pub created: bool,
    pub artwork: Vec<PathBuf>,
    /// Set when artwork was requested but could not be fetched.
    pub artwork_error: Option<String>,
    /// Single-window mode couldn't be set up yet; it will be after the first launch.
    pub single_window_pending: bool,
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

    pub fn localconfig_vdf(&self, user: &User) -> PathBuf {
        self.userdata(user).join("config/localconfig.vdf")
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

    /// Apply `change` to Steam's files while Steam is closed.
    ///
    /// Steam keeps shortcuts in memory and writes them back when it exits, so an edit
    /// made while it runs is silently lost. If Steam is running it is closed first
    /// (only when `restart` is true, otherwise this fails), and started again after.
    /// `is_applied` is checked right after the change and again once Steam is back up,
    /// to catch a Steam instance that overwrote the files anyway.
    pub fn with_closed<T>(
        &self,
        restart: bool,
        change: impl FnOnce() -> Result<T>,
        is_applied: impl Fn() -> bool,
    ) -> Result<Applied<T>> {
        let running = process::is_running()?;
        if running {
            if !restart {
                return Err(Error::Steam("Steam is running. Exit Steam first, then try again.".into()));
            }
            process::shutdown(self.flavor, std::time::Duration::from_secs(60))?;
        }
        let value = change()?;
        if !is_applied() {
            return Err(Error::Steam("the change was written but could not be read back from Steam's files".into()));
        }
        let mut survived_restart = None;
        if running {
            process::start(self.flavor)?;
            if process::wait_until_running(std::time::Duration::from_secs(90))? {
                // Give Steam time to load its library (and to clobber the file if a
                // stale instance was still around).
                std::thread::sleep(std::time::Duration::from_secs(10));
                survived_restart = Some(is_applied());
            }
        }
        Ok(Applied { value, restarted: running, survived_restart })
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

    /// All shortcuts in the user's `shortcuts.vdf` as `(appid, name, exe)`.
    pub fn shortcuts(&self, user: &User) -> Result<Vec<(u32, String, String)>> {
        let path = self.shortcuts_vdf(user);
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).io_ctx(|| format!("reading {}", path.display())),
        };
        let root = parse_shortcuts(&path, &data)?;
        Ok(root
            .get_obj("shortcuts")
            .map(|list| {
                list.0
                    .iter()
                    .filter_map(|(_, v)| match v {
                        binary_vdf::Value::Obj(o) => Some((
                            o.get_int("appid").unwrap_or(0) as u32,
                            o.get_str("AppName").unwrap_or_default(),
                            o.get_str("Exe").unwrap_or_default(),
                        )),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default())
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
        // Aniimo needs the raw gamepad; Steam Input intercepts it otherwise.
        self.set_steam_input(user, appid, false)?;
        self.map_game_drive(appid, &spec.start_dir)?;
        let single_window_pending = self.set_single_window(appid, spec.single_window, spec.compat_tool.as_deref())?
            == PrefixChange::Pending;
        Ok(Registered { appid, created, artwork, artwork_error, single_window_pending })
    }

    /// The Proton prefix Steam uses for a non-Steam shortcut. It always lives in the
    /// main Steam library, and only exists once the game has been launched (or
    /// [`Self::map_game_drive`] has created it).
    pub fn prefix_dir(&self, appid: u32) -> PathBuf {
        self.root.join("steamapps/compatdata").join(appid.to_string()).join("pfx")
    }

    /// Give the install directory its own drive letter ([`GAME_DRIVE`]) in the game's prefix.
    ///
    /// Steam runs Proton inside a container whose `/` is a small RAM-backed tmpfs. Without
    /// this the game runs from `Z:\home\...`, and its free-space check on `Z:/` sees that
    /// tmpfs (about half the RAM) instead of the real disk, then refuses to download.
    /// Wine maps a path to the drive whose root is closest, so with this link the game
    /// runs from `G:\` and sees the install disk. Proton keeps extra `dosdevices` links;
    /// it only manages `c:`, `z:`, `s:` and `t:`. This doesn't touch Steam's own files,
    /// so Steam may be running.
    pub fn map_game_drive(&self, appid: u32, install_dir: &Path) -> Result<()> {
        let dosdevices = self.prefix_dir(appid).join("dosdevices");
        fs::create_dir_all(&dosdevices).io_ctx(|| format!("creating {}", dosdevices.display()))?;
        let link = dosdevices.join(GAME_DRIVE);
        match fs::read_link(&link) {
            Ok(target) if target == install_dir => return Ok(()),
            Ok(_) => fs::remove_file(&link).io_ctx(|| format!("replacing {}", link.display()))?,
            Err(_) if link.symlink_metadata().is_ok() => {
                return Err(Error::Steam(format!("{} exists and is not a drive link", link.display())));
            }
            Err(_) => {}
        }
        symlink_dir(install_dir, &link).io_ctx(|| format!("creating {}", link.display()))
    }

    /// Whether [`Self::map_game_drive`] is in place for `install_dir`.
    pub fn game_drive_mapped(&self, appid: u32, install_dir: &Path) -> bool {
        fs::read_link(self.prefix_dir(appid).join("dosdevices").join(GAME_DRIVE))
            .is_ok_and(|t| t == install_dir)
    }

    /// Steam library folders: the main one plus any listed in `libraryfolders.vdf`.
    pub fn library_folders(&self) -> Vec<PathBuf> {
        let mut out = vec![self.root.clone()];
        if let Ok(src) = fs::read_to_string(self.root.join("steamapps/libraryfolders.vdf"))
            && let Ok(root) = text_vdf::parse(&src)
            && let Some(list) = root.get_obj("libraryfolders")
        {
            for (_, v) in &list.0 {
                if let text_vdf::Value::Obj(o) = v
                    && let Some(p) = o.get_str("path")
                {
                    let p = PathBuf::from(p);
                    if !out.contains(&p) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    /// Install directory of a compat tool, by its internal name (`proton_10`, `GE-Proton10-15`, ...).
    pub fn compat_tool_dir(&self, tool: &str) -> Option<PathBuf> {
        // Custom tools declare their internal names in compatibilitytool.vdf.
        for entry in fs::read_dir(self.root.join("compatibilitytools.d")).into_iter().flatten().flatten() {
            let Ok(src) = fs::read_to_string(entry.path().join("compatibilitytool.vdf")) else { continue };
            let Ok(root) = text_vdf::parse(&src) else { continue };
            let tools = root.get_obj("compatibilitytools").and_then(|o| o.get_obj("compat_tools"));
            if let Some(t) = tools.and_then(|t| t.get_obj(tool)) {
                return Some(entry.path().join(t.get_str("install_path").unwrap_or(".")));
            }
        }
        // Valve's Proton builds are Steam apps named "Proton 10.0", "Proton - Experimental", ...
        let matches = |name: &str| match tool {
            "proton_experimental" => name == "Proton - Experimental",
            "proton_hotfix" => name == "Proton Hotfix",
            t => t.strip_prefix("proton_").filter(|v| v.chars().all(|c| c.is_ascii_digit())).is_some_and(|v| {
                name.strip_prefix("Proton ")
                    .and_then(|rest| rest.strip_prefix(v))
                    .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_digit()))
            }),
        };
        self.library_folders().into_iter().find_map(|lib| {
            fs::read_dir(lib.join("steamapps/common"))
                .into_iter()
                .flatten()
                .flatten()
                .find(|e| e.file_name().to_str().is_some_and(matches))
                .map(|e| e.path())
        })
    }

    /// Install directory of an installed Steam app, from its `appmanifest_<id>.acf`.
    pub fn app_install_dir(&self, appid: u32) -> Option<PathBuf> {
        self.library_folders().into_iter().find_map(|lib| {
            let src = fs::read_to_string(lib.join(format!("steamapps/appmanifest_{appid}.acf"))).ok()?;
            let root = text_vdf::parse(&src).ok()?;
            let dir = lib.join("steamapps/common").join(root.get_obj("AppState")?.get_str("installdir")?);
            dir.is_dir().then_some(dir)
        })
    }

    /// How Steam would start `exe` for this shortcut: the compat tool inside the Steam
    /// Linux Runtime container it asks for, with the same `STEAM_COMPAT_*` environment.
    /// Using the shortcut's prefix means anything the game saves (like its login) is
    /// there the next time it's started from Steam.
    pub fn game_launch(&self, appid: u32, exe: &Path, tool: &str) -> Result<GameLaunch> {
        let tool_dir = self
            .compat_tool_dir(tool)
            .ok_or_else(|| Error::Steam(format!("{tool} isn't installed. Start the game from Steam once so Steam downloads it.")))?;
        let proton = tool_dir.join("proton");
        if !proton.is_file() {
            return Err(Error::Steam(format!("{} doesn't look like a Proton install", tool_dir.display())));
        }
        let compat_data = self.prefix_dir(appid).parent().map(Path::to_path_buf).unwrap_or_default();
        fs::create_dir_all(&compat_data).io_ctx(|| format!("creating {}", compat_data.display()))?;

        // Proton declares the runtime it needs, e.g. "require_tool_appid" "1628350" (sniper).
        let runtime = fs::read_to_string(tool_dir.join("toolmanifest.vdf"))
            .ok()
            .and_then(|src| text_vdf::parse(&src).ok())
            .and_then(|m| m.get_obj("manifest")?.get_str("require_tool_appid")?.parse::<u32>().ok())
            .and_then(|id| self.app_install_dir(id));
        let mut tool_paths = vec![tool_dir.display().to_string()];
        let mut args = Vec::new();
        let program = match runtime.as_ref().map(|r| r.join("_v2-entry-point")).filter(|p| p.is_file()) {
            Some(entry) => {
                tool_paths.push(runtime.unwrap().display().to_string());
                args.extend(["--verb=waitforexitandrun".to_string(), "--".to_string(), proton.display().to_string()]);
                entry
            }
            None => proton,
        };
        args.extend(["waitforexitandrun".to_string(), exe.display().to_string()]);

        let id = appid.to_string();
        let install = exe.parent().map_or_else(String::new, |p| p.display().to_string());
        let env = [
            ("STEAM_COMPAT_DATA_PATH", compat_data.display().to_string()),
            ("STEAM_COMPAT_CLIENT_INSTALL_PATH", self.root.display().to_string()),
            ("STEAM_COMPAT_INSTALL_PATH", install.clone()),
            ("STEAM_COMPAT_TOOL_PATHS", tool_paths.join(":")),
            ("STEAM_COMPAT_LIBRARY_PATHS", self.library_folders().iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(":")),
            ("STEAM_COMPAT_APP_ID", id.clone()),
            ("SteamAppId", id.clone()),
            ("SteamGameId", id),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        Ok(GameLaunch { program, args, env, dir: PathBuf::from(install) })
    }

    /// Whether Proton has set up the game's prefix yet (it does so on first launch).
    pub fn prefix_ready(&self, appid: u32) -> bool {
        self.prefix_dir(appid).join("user.reg").is_file()
    }

    /// The single-window size YACCGL configured in the game's prefix, if any.
    pub fn single_window(&self, appid: u32) -> Option<(u32, u32)> {
        let reg = wine_reg::RegFile::parse(&fs::read_to_string(self.prefix_dir(appid).join("user.reg")).ok()?);
        if reg.get(WINE_EXPLORER_KEY, "Desktop").as_deref() != Some(WINE_DESKTOP_NAME) {
            return None;
        }
        let size = reg.get(WINE_DESKTOPS_KEY, WINE_DESKTOP_NAME)?;
        let (w, h) = size.split_once('x')?;
        Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
    }

    /// Turn single-window mode on (`Some(size)`) or off for the game's prefix.
    ///
    /// Wine's virtual desktop puts every window the game opens, including its login
    /// window, inside one window. In Game Mode, gamescope only gives keyboard focus to
    /// the game's main window, so without this the on-screen keyboard can't type into
    /// the login window.
    ///
    /// Before the first launch there's no prefix yet. Proton builds a new prefix by
    /// copying its template and skipping files that already exist, so we seed
    /// `user.reg` from the compat tool's template with the setting added. If that
    /// template doesn't exist yet either, this returns [`PrefixChange::Pending`].
    /// Wine rewrites `user.reg` while the game runs, so the game must be closed.
    pub fn set_single_window(&self, appid: u32, size: Option<(u32, u32)>, tool: Option<&str>) -> Result<PrefixChange> {
        let pfx = self.prefix_dir(appid);
        let path = pfx.join("user.reg");
        if !path.is_file() {
            if size.is_none() {
                return Ok(PrefixChange::Applied);
            }
            let template = tool
                .and_then(|t| self.compat_tool_dir(t))
                .map(|d| d.join("files/share/default_pfx/user.reg"))
                .filter(|p| p.is_file());
            let Some(template) = template else { return Ok(PrefixChange::Pending) };
            fs::create_dir_all(&pfx).io_ctx(|| format!("creating {}", pfx.display()))?;
            fs::copy(&template, &path).io_ctx(|| format!("creating {}", path.display()))?;
        }
        let src = fs::read_to_string(&path).io_ctx(|| format!("reading {}", path.display()))?;
        let mut reg = wine_reg::RegFile::parse(&src);
        match size {
            Some((w, h)) => {
                reg.set(WINE_EXPLORER_KEY, "Desktop", WINE_DESKTOP_NAME);
                reg.set(WINE_DESKTOPS_KEY, WINE_DESKTOP_NAME, &format!("{w}x{h}"));
            }
            // Leave any other virtual desktop the user set up themselves.
            None if reg.get(WINE_EXPLORER_KEY, "Desktop").as_deref() == Some(WINE_DESKTOP_NAME) => {
                reg.remove(WINE_EXPLORER_KEY, "Desktop");
            }
            None => return Ok(PrefixChange::Applied),
        }
        write_backed_up(&path, reg.serialize().as_bytes())?;
        Ok(PrefixChange::Applied)
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

    /// Enable or disable Steam Input for a shortcut in `localconfig.vdf`.
    ///
    /// `enabled == false` is "Disable Steam Input" in the Steam UI (`UseSteamControllerConfig` `0`),
    /// which is what Aniimo needs so the raw gamepad reaches the game.
    pub fn set_steam_input(&self, user: &User, appid: u32, enabled: bool) -> Result<()> {
        let path = self.localconfig_vdf(user);
        let src = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).io_ctx(|| format!("reading {}", path.display())),
        };
        let mut root = text_vdf::parse(&src).map_err(|message| Error::Vdf { file: path.display().to_string(), message })?;
        let entry = root.obj_mut("UserLocalConfigStore").obj_mut("apps").obj_mut(&signed_appid_key(appid));
        // Values match what Steam writes when you flip the Controller override in Properties.
        entry.set_str("UseSteamControllerConfig", if enabled { "1" } else { "0" });
        entry.set_str("SteamControllerRumble", "-1");
        entry.set_str("SteamControllerRumbleIntensity", "320");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).io_ctx(|| format!("creating {}", parent.display()))?;
        }
        write_backed_up(&path, text_vdf::serialize(&root).as_bytes())
    }

    /// Whether Steam Input is forced off for `appid` (`UseSteamControllerConfig` `0`).
    pub fn steam_input_disabled(&self, user: &User, appid: u32) -> bool {
        let Ok(src) = fs::read_to_string(self.localconfig_vdf(user)) else { return false };
        let Ok(root) = text_vdf::parse(&src) else { return false };
        root.get_obj("UserLocalConfigStore")
            .and_then(|o| o.get_obj("apps"))
            .and_then(|o| o.get_obj(&signed_appid_key(appid)))
            .and_then(|o| o.get_str("UseSteamControllerConfig"))
            == Some("0")
    }
}

/// `localconfig.vdf` keys non-Steam shortcuts under the signed 32-bit form of the app id.
fn signed_appid_key(appid: u32) -> String {
    (appid as i32).to_string()
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

/// Keep a `.yaccgl-bak` copy of the previous contents, then replace the file atomically.
fn write_backed_up(path: &Path, data: &[u8]) -> Result<()> {
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".yaccgl-bak");
        fs::copy(path, &bak).io_ctx(|| format!("backing up {}", path.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".yaccgl-tmp");
    fs::write(&tmp, data).io_ctx(|| format!("writing {}", path.display()))?;
    fs::rename(&tmp, path).io_ctx(|| format!("replacing {}", path.display()))
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
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
            single_window: Some(DEFAULT_WINDOW_SIZE),
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
        assert!(steam.steam_input_disabled(&user, first.appid));
        let local = fs::read_to_string(steam.localconfig_vdf(&user)).unwrap();
        assert!(local.contains(&format!("\"{}\"", first.appid as i32)));
        assert!(local.contains("\"UseSteamControllerConfig\"\t\t\"0\""));
        // Existing lowercase "valve" key was reused rather than duplicated.
        let cfg = fs::read_to_string(steam.config_vdf()).unwrap();
        assert!(!cfg.contains("\"Valve\""));
        assert!(cfg.contains("AutoUpdateWindowEnabled"));
        assert!(steam.config_vdf().with_extension("vdf.yaccgl-bak").exists());
    }

    #[test]
    fn set_steam_input_uses_signed_appid_and_preserves_other_apps() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        let user = steam.users().remove(0);
        fs::write(
            steam.localconfig_vdf(&user),
            "\"UserLocalConfigStore\"\n{\n\t\"apps\"\n\t{\n\t\t\"570\"\n\t\t{\n\t\t\t\"LastPlayed\"\t\t\"1\"\n\t\t}\n\t}\n\t\"streaming_v2\"\n\t{\n\t\t\"EnableStreaming\"\t\t\"0\"\n\t}\n}\n",
        )
        .unwrap();

        steam.set_steam_input(&user, 3_039_192_195, false).unwrap();
        assert!(steam.steam_input_disabled(&user, 3_039_192_195));
        let src = fs::read_to_string(steam.localconfig_vdf(&user)).unwrap();
        assert!(src.contains("\"-1255775101\""));
        assert!(src.contains("\"570\""));
        assert!(src.contains("\"LastPlayed\""));
        assert!(src.contains("\"EnableStreaming\""));

        steam.set_steam_input(&user, 3_039_192_195, true).unwrap();
        assert!(!steam.steam_input_disabled(&user, 3_039_192_195));
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

    #[cfg(unix)]
    #[test]
    fn maps_game_drive_and_follows_moves() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        let (a, b) = (home.path().join("Games/Aniimo"), home.path().join("Other/Aniimo"));
        let link = steam.prefix_dir(42).join("dosdevices/g:");

        // Works before Proton has created the prefix.
        steam.map_game_drive(42, &a).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), a);
        assert!(steam.game_drive_mapped(42, &a));
        steam.map_game_drive(42, &a).unwrap();

        steam.map_game_drive(42, &b).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), b);
        assert!(!steam.game_drive_mapped(42, &a));

        // Proton's own drives are left alone.
        std::os::unix::fs::symlink("/", link.with_file_name("z:")).unwrap();
        steam.map_game_drive(42, &a).unwrap();
        assert_eq!(fs::read_link(link.with_file_name("z:")).unwrap(), PathBuf::from("/"));
    }

    const TEMPLATE_REG: &str = "WINE REGISTRY Version 2\n;; All keys relative to \\\\User\\\\S-1-5-21-0-0-0-1000\n\n#arch=win64\n\n[Control Panel\\\\Desktop] 1700000000\n\"FontSmoothing\"=\"2\"\n";

    /// Installs a fake "Proton 10.0" in a second Steam library with a template prefix.
    fn fake_proton(home: &Path, steam: &Steam) -> PathBuf {
        let lib = home.join("sdcard/SteamLibrary");
        let proton = lib.join("steamapps/common/Proton 10.0");
        fs::create_dir_all(proton.join("files/share/default_pfx")).unwrap();
        fs::write(proton.join("files/share/default_pfx/user.reg"), TEMPLATE_REG).unwrap();
        fs::create_dir_all(steam.root.join("steamapps/common/Proton 100.0")).unwrap();
        fs::write(
            steam.root.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                steam.root.display(),
                lib.display()
            ),
        )
        .unwrap();
        proton
    }

    #[test]
    fn finds_compat_tool_dirs() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        let proton = fake_proton(home.path(), &steam);
        assert_eq!(steam.compat_tool_dir("proton_10"), Some(proton));
        // "Proton 100.0" must not match proton_10, and missing tools aren't invented.
        assert_eq!(steam.compat_tool_dir("proton_9"), None);
        assert!(steam.compat_tool_dir("proton_100").unwrap().ends_with("Proton 100.0"));
    }

    #[test]
    fn game_launch_matches_steams_command() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        let proton = fake_proton(home.path(), &steam);
        let exe = home.path().join("Games/Aniimo/Aniimo.exe");

        // Proton present but not its runtime: run Proton directly.
        fs::write(proton.join("proton"), "#!/bin/sh\n").unwrap();
        fs::write(proton.join("toolmanifest.vdf"), "\"manifest\"\n{\n\t\"require_tool_appid\"\t\t\"1628350\"\n}\n").unwrap();
        let l = steam.game_launch(99, &exe, "proton_10").unwrap();
        assert_eq!(l.program, proton.join("proton"));
        assert_eq!(l.args, vec!["waitforexitandrun".to_string(), exe.display().to_string()]);

        // With the runtime installed (in the main library), go through its entry point.
        let runtime = steam.root.join("steamapps/common/SteamLinuxRuntime_sniper");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(runtime.join("_v2-entry-point"), "#!/bin/sh\n").unwrap();
        fs::write(
            steam.root.join("steamapps/appmanifest_1628350.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"1628350\"\n\t\"installdir\"\t\t\"SteamLinuxRuntime_sniper\"\n}\n",
        )
        .unwrap();
        let l = steam.game_launch(99, &exe, "proton_10").unwrap();
        assert_eq!(l.program, runtime.join("_v2-entry-point"));
        assert_eq!(l.args[..3], ["--verb=waitforexitandrun".to_string(), "--".into(), proton.join("proton").display().to_string()]);
        let env: std::collections::HashMap<_, _> = l.env.into_iter().collect();
        assert_eq!(env["STEAM_COMPAT_DATA_PATH"], steam.root.join("steamapps/compatdata/99").display().to_string());
        assert_eq!(env["STEAM_COMPAT_CLIENT_INSTALL_PATH"], steam.root.display().to_string());
        assert_eq!(env["SteamGameId"], "99");
        assert!(env["STEAM_COMPAT_TOOL_PATHS"].contains("SteamLinuxRuntime_sniper"));
        assert!(steam.root.join("steamapps/compatdata/99").is_dir());

        assert!(steam.game_launch(99, &exe, "proton_9").is_err());
    }

    #[test]
    fn single_window_before_first_launch_seeds_from_template() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        assert_eq!(steam.set_single_window(7, Some((1280, 800)), Some("proton_10")).unwrap(), PrefixChange::Pending);
        assert!(!steam.prefix_ready(7));

        fake_proton(home.path(), &steam);
        assert_eq!(steam.set_single_window(7, Some((1280, 800)), Some("proton_10")).unwrap(), PrefixChange::Applied);
        assert!(steam.prefix_ready(7));
        assert_eq!(steam.single_window(7), Some((1280, 800)));
        let reg = fs::read_to_string(steam.prefix_dir(7).join("user.reg")).unwrap();
        assert!(reg.starts_with(TEMPLATE_REG), "template content must be kept as-is");
        assert!(reg.contains("[Software\\\\Wine\\\\Explorer] "));
        assert!(reg.contains("\"Desktop\"=\"Default\""));
        assert!(reg.contains("\"Default\"=\"1280x800\""));
    }

    #[test]
    fn single_window_toggles_on_existing_prefix_and_respects_user_desktops() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        fs::create_dir_all(steam.prefix_dir(7)).unwrap();
        fs::write(steam.prefix_dir(7).join("user.reg"), TEMPLATE_REG).unwrap();

        steam.set_single_window(7, Some((1920, 1080)), None).unwrap();
        assert_eq!(steam.single_window(7), Some((1920, 1080)));
        steam.set_single_window(7, None, None).unwrap();
        assert_eq!(steam.single_window(7), None);

        // A differently named virtual desktop the user set up themselves stays.
        let path = steam.prefix_dir(7).join("user.reg");
        let mut reg = wine_reg::RegFile::parse(&fs::read_to_string(&path).unwrap());
        reg.set(WINE_EXPLORER_KEY, "Desktop", "Mine");
        fs::write(&path, reg.serialize()).unwrap();
        steam.set_single_window(7, None, None).unwrap();
        assert_eq!(steam.single_window(7), None);
        let reg = wine_reg::RegFile::parse(&fs::read_to_string(&path).unwrap());
        assert_eq!(reg.get(WINE_EXPLORER_KEY, "Desktop").as_deref(), Some("Mine"));
    }

    #[test]
    fn register_maps_game_drive() {
        let home = tempfile::tempdir().unwrap();
        let steam = fake_steam(home.path());
        let user = steam.users().remove(0);
        let spec = spec(home.path());
        let reg = steam.register(&crate::http::agent(), &user, &spec).unwrap();
        assert!(steam.game_drive_mapped(reg.appid, &spec.start_dir));
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
