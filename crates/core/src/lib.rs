//! Core logic for NotAnotherCreatureLauncher: talking to the official Aniimo
//! version API, downloading and extracting the game, and registering it with Steam.

pub mod api;
pub mod download;
pub mod error;
pub mod extract;
pub mod http;
pub mod install;
pub mod settings;
pub mod space;
pub mod steam;

pub use error::{Error, Result};

/// Display name used for the game everywhere (Steam shortcut, UI).
pub const GAME_NAME: &str = "Aniimo";

/// Executable inside the install directory, used when the API doesn't name one.
pub const DEFAULT_EXE: &str = "Aniimo.exe";
