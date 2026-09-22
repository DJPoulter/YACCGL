//! FunPlus FPX login host: a tiny Windows exe that loads the game's `FPX.dll` and
//! shows the email login dialog without starting Unity. Dropped next to the plugin
//! under Proton so the session lands in the Steam shortcut's prefix.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::IoContext;
use crate::{Error, Result};

/// Relative to the game install directory.
pub const PLUGIN_DIR: &str = "Aniimo_Data/Plugins/x86_64";
pub const LOGIN_EXE_NAME: &str = "fpx_login.exe";
pub const CONFIG_NAME: &str = "config.json";

const BUNDLED_EXE: &[u8] = include_bytes!("../../../data/fpx-login/fpx_login.exe");
const BUNDLED_CONFIG: &[u8] = include_bytes!("../../../data/fpx-login/config.json");

/// `{install}/Aniimo_Data/Plugins/x86_64`.
pub fn plugin_dir(install_dir: &Path) -> PathBuf {
    install_dir.join(PLUGIN_DIR)
}

/// Path of the login host inside an install.
pub fn login_exe(install_dir: &Path) -> PathBuf {
    plugin_dir(install_dir).join(LOGIN_EXE_NAME)
}

/// True when the game's FPX plugin (and therefore a usable login host target) is present.
pub fn fpx_present(install_dir: &Path) -> bool {
    plugin_dir(install_dir).join("FPX.dll").is_file()
}

/// Copy the bundled host + config into the game plugin folder if missing or stale.
///
/// Safe to call before every Log In; only rewrites when the on-disk bytes differ.
pub fn ensure_installed(install_dir: &Path) -> Result<PathBuf> {
    let dir = plugin_dir(install_dir);
    if !dir.join("FPX.dll").is_file() {
        return Err(Error::Io {
            context: format!(
                "FPX.dll is missing under {}. Install or repair the game first.",
                dir.display()
            ),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        });
    }
    fs::create_dir_all(&dir).io_ctx(|| format!("creating {}", dir.display()))?;
    write_if_changed(&dir.join(LOGIN_EXE_NAME), BUNDLED_EXE)?;
    write_if_changed(&dir.join(CONFIG_NAME), BUNDLED_CONFIG)?;
    Ok(login_exe(install_dir))
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.is_file()
        && fs::read(path)
            .ok()
            .is_some_and(|existing| existing == bytes)
    {
        return Ok(());
    }
    fs::write(path, bytes).io_ctx(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_requires_fpx_dll() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ensure_installed(dir.path()).is_err());
    }

    #[test]
    fn ensure_writes_host_next_to_fpx() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = dir.path().join(PLUGIN_DIR);
        fs::create_dir_all(&plugins).unwrap();
        fs::write(plugins.join("FPX.dll"), b"fake").unwrap();
        let exe = ensure_installed(dir.path()).unwrap();
        assert!(exe.is_file());
        assert!(plugins.join(CONFIG_NAME).is_file());
        assert_eq!(fs::metadata(&exe).unwrap().len(), BUNDLED_EXE.len() as u64);
        // Second call is a no-op.
        ensure_installed(dir.path()).unwrap();
    }
}
