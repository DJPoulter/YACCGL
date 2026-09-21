//! Detecting, closing and restarting the Steam client, from inside or outside Flatpak.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::{FLATPAK_STEAM_ID, Flavor};
use crate::{Error, Result};

/// True when this launcher itself runs inside a Flatpak sandbox.
pub fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// True in the Steam Deck's Game Mode (gamescope session), where Steam is the
/// session itself and must not be closed.
pub fn in_game_mode() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_lowercase().contains("gamescope"))
        || std::env::var_os("SteamGamepadUI").is_some()
}

/// A command that runs on the host, escaping the Flatpak sandbox when needed.
pub fn host_command(program: &str) -> Command {
    if in_flatpak() {
        let mut c = Command::new("flatpak-spawn");
        c.arg("--host").arg(program);
        c
    } else {
        Command::new(program)
    }
}

pub fn is_running() -> bool {
    let status = host_command("pgrep")
        .args(["-x", "steam"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status.map(|s| s.code()) {
        Ok(Some(0)) => true,
        Ok(Some(1)) => false,
        // pgrep unavailable or the host couldn't be reached: fall back to Steam's pid file.
        _ => pid_file_alive(),
    }
}

fn pid_file_alive() -> bool {
    let Some(home) = dirs::home_dir() else { return false };
    let Ok(pid) = std::fs::read_to_string(home.join(".steam/steam.pid")) else { return false };
    let pid = pid.trim();
    !pid.is_empty() && Path::new("/proc").join(pid).join("comm").exists()
}

fn steam_command(flavor: Flavor) -> Command {
    match flavor {
        Flavor::Native => host_command("steam"),
        Flavor::Flatpak => {
            let mut c = host_command("flatpak");
            c.args(["run", FLATPAK_STEAM_ID]);
            c
        }
    }
}

/// Ask Steam to exit and wait until it has.
pub fn shutdown(flavor: Flavor, timeout: Duration) -> Result<()> {
    if in_game_mode() {
        return Err(Error::Steam(
            "Steam can't be closed from Game Mode. Switch to Desktop Mode to add the game to Steam.".into(),
        ));
    }
    if !is_running() {
        return Ok(());
    }
    steam_command(flavor)
        .arg("-shutdown")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::Steam(format!("could not ask Steam to close: {e}")))?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(500));
        if !is_running() {
            // Steam flushes its config files as the last step of exiting.
            thread::sleep(Duration::from_secs(1));
            return Ok(());
        }
    }
    Err(Error::Steam("Steam did not close in time. Please exit Steam manually and try again.".into()))
}

/// Start Steam detached from this process.
pub fn start(flavor: Flavor) -> Result<()> {
    spawn_detached(flavor, "")
}

/// Launch a non-Steam shortcut through Steam (starting Steam if needed).
pub fn launch_shortcut(flavor: Flavor, appid: u32) -> Result<()> {
    // Shortcut game ids are the 32-bit app id in the high half, with type 0x02000000.
    let game_id = (u64::from(appid) << 32) | 0x0200_0000;
    spawn_detached(flavor, &format!("steam://rungameid/{game_id}"))
}

fn spawn_detached(flavor: Flavor, arg: &str) -> Result<()> {
    let launch = match flavor {
        Flavor::Native => "steam".to_string(),
        Flavor::Flatpak => format!("flatpak run {FLATPAK_STEAM_ID}"),
    };
    host_command("sh")
        .args(["-c", &format!("setsid {launch} {arg} >/dev/null 2>&1 &")])
        .stdin(Stdio::null())
        .status()
        .map_err(|e| Error::Steam(format!("could not start Steam: {e}")))?;
    Ok(())
}
