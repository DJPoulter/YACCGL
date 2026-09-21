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

/// Runs on the host: exit 0 if any Steam client process is alive, 1 if none is.
/// The web helper counts too, since Steam isn't done writing its files until it exits.
const RUNNING_CHECK: &str = r#"
pgrep -x steam >/dev/null 2>&1 && exit 0
pgrep -x steamwebhelper >/dev/null 2>&1 && exit 0
p=$(cat "$HOME/.steam/steam.pid" 2>/dev/null)
[ -n "$p" ] && kill -0 "$p" 2>/dev/null && [ "$(cat /proc/$p/comm 2>/dev/null)" = steam ] && exit 0
exit 1
"#;

/// Whether the Steam client is running on the host.
///
/// This must be checked on the host: inside a Flatpak sandbox `/proc` only shows
/// the sandbox's own processes, so a local check would always say "not running".
pub fn is_running() -> Result<bool> {
    let status = host_command("sh")
        .args(["-c", RUNNING_CHECK])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| Error::Steam(format!("couldn't check whether Steam is running: {e}")))?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        other => Err(Error::Steam(format!(
            "couldn't check whether Steam is running (exit status {other:?}). \
             Please exit Steam from its menu and try again."
        ))),
    }
}

/// Wait until Steam is running again, e.g. after [`start`].
pub fn wait_until_running(timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_running()? {
            return Ok(true);
        }
        thread::sleep(Duration::from_secs(1));
    }
    Ok(false)
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
    if !is_running()? {
        return Ok(());
    }
    let mut request = steam_command(flavor)
        .arg("-shutdown")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::Steam(format!("could not ask Steam to close: {e}")))?;
    // Reap it in the background: an unreaped `steam -shutdown` lingers as a zombie
    // named "steam", which would make Steam look like it never exits.
    thread::spawn(move || {
        let _ = request.wait();
    });
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(500));
        if !is_running()? {
            // Steam flushes its config files as the last step of exiting.
            thread::sleep(Duration::from_secs(2));
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
