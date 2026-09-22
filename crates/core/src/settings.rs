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
    /// Settings format; older files are migrated in [`Settings::load`]. Missing = 0.
    #[serde(default)]
    pub version: u32,
}

/// 1: single-window mode became opt-in (0.1.5 turned it on for everyone).
/// 2: single-window mode removed (it didn't fix Game Mode login).
const SETTINGS_VERSION: u32 = 2;

impl Default for Settings {
    fn default() -> Self {
        Settings {
            install_dir: default_install_dir(),
            compat_tool: crate::steam::DEFAULT_COMPAT_TOOL.into(),
            launch_options: String::new(),
            artwork: true,
            steam_user: None,
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

impl Settings {
    /// Load settings, falling back to defaults if the file is missing or unreadable.
    pub fn load() -> Settings {
        let loaded: Option<Settings> = fs::read(path()).ok().and_then(|d| serde_json::from_slice(&d).ok());
        loaded.map(Settings::migrate).unwrap_or_default()
    }

    fn migrate(mut self) -> Settings {
        // Older files may still contain `single_window` / `window_size`; serde ignores them.
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
    fn loads_defaults_and_ignores_removed_single_window_fields() {
        assert_eq!(Settings::default().version, SETTINGS_VERSION);
        let old: Settings = serde_json::from_str(
            r#"{"install_dir":"/g","single_window":true,"window_size":"1280x800"}"#,
        )
        .unwrap();
        assert_eq!(old.install_dir, PathBuf::from("/g"));
        assert_eq!(old.migrate().version, SETTINGS_VERSION);
    }
}
