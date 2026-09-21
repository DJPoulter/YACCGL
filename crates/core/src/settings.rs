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
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            install_dir: default_install_dir(),
            compat_tool: crate::steam::DEFAULT_COMPAT_TOOL.into(),
            launch_options: String::new(),
            artwork: true,
            steam_user: None,
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
        fs::read(path())
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let p = path();
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).io_ctx(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&p, serde_json::to_vec_pretty(self)?).io_ctx(|| format!("writing {}", p.display()))
    }
}
