//! Launcher preferences, stored as JSON in the user's config directory.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::IoContext;
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub install_dir: PathBuf,
    /// Internal Steam compat tool name forced on the shortcut.
    pub compat_tool: String,
    pub launch_options: String,
    pub artwork: bool,
    /// Steam account the shortcut goes to; `None` = most recently logged in.
    pub steam_user: Option<u32>,
    /// Keep all the game's windows in one (Wine virtual desktop). Off by default.
    pub single_window: bool,
    /// Size of that window, as "WIDTHxHEIGHT".
    pub window_size: String,
    /// Settings format; older files are migrated in [`Settings::load`]. Missing = 0.
    #[serde(default)]
    pub version: u32,
}

/// 1: single-window mode became opt-in (0.1.5 turned it on for everyone).
const SETTINGS_VERSION: u32 = 1;

impl Default for Settings {
    fn default() -> Self {
        Settings {
            install_dir: default_install_dir(),
            compat_tool: crate::steam::DEFAULT_COMPAT_TOOL.into(),
            launch_options: String::new(),
            artwork: true,
            steam_user: None,
            single_window: false,
            window_size: format_size(crate::steam::DEFAULT_WINDOW_SIZE),
            version: SETTINGS_VERSION,
        }
    }
}

pub fn default_install_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Games").join(crate::GAME_NAME)
}

fn path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("yet-another-creature-collector-game-launcher")
        .join("settings.json")
}

pub fn format_size((w, h): (u32, u32)) -> String {
    format!("{w}x{h}")
}

pub fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    let size = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (size.0 > 0 && size.1 > 0).then_some(size)
}

impl Settings {
    /// The single-window size to apply, or `None` when the option is off.
    pub fn single_window_size(&self) -> Option<(u32, u32)> {
        self.single_window
            .then(|| parse_size(&self.window_size).unwrap_or(crate::steam::DEFAULT_WINDOW_SIZE))
    }

    /// Load settings, falling back to defaults if the file is missing or unreadable.
    pub fn load() -> Settings {
        let loaded: Option<Settings> = fs::read(path()).ok().and_then(|d| serde_json::from_slice(&d).ok());
        loaded.map(Settings::migrate).unwrap_or_default()
    }

    fn migrate(mut self) -> Settings {
        if self.version < 1 {
            self.single_window = false;
        }
        self.version = SETTINGS_VERSION;
        self
    }

    pub fn save(&self) -> Result<()> {
        let p = path();
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).io_ctx(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&p, serde_json::to_vec_pretty(self)?).io_ctx(|| format!("writing {}", p.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_window_is_opt_in_and_0_1_5_settings_are_migrated() {
        assert!(!Settings::default().single_window);
        // Saved by 0.1.5, which turned it on for everyone.
        let old: Settings = serde_json::from_str(r#"{"install_dir":"/g","single_window":true}"#).unwrap();
        assert_eq!(old.version, 0);
        let s = old.migrate();
        assert!(!s.single_window);
        assert_eq!(s.single_window_size(), None);
        // Chosen after the migration: kept.
        let s: Settings = serde_json::from_str(r#"{"install_dir":"/g","single_window":true,"version":1}"#).unwrap();
        assert!(s.migrate().single_window);
    }

    #[test]
    fn window_sizes() {
        assert_eq!(parse_size("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_size(" 2560 X 1440 "), Some((2560, 1440)));
        assert_eq!(parse_size("0x10"), None);
        assert_eq!(parse_size("big"), None);
        let s = Settings { single_window: false, ..Settings::default() };
        assert_eq!(s.single_window_size(), None);
    }
}
