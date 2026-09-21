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

/// Run the game on the host and wait for it to exit.
pub fn run_game(launch: &super::GameLaunch) -> Result<std::process::ExitStatus> {
    let mut cmd = if in_flatpak() {
        // flatpak-spawn doesn't forward our environment; pass it explicitly.
        let mut c = Command::new("flatpak-spawn");
        c.arg("--host").arg(format!("--directory={}", launch.dir.display()));
        for (k, v) in &launch.env {
            c.arg(format!("--env={k}={v}"));
        }
        c.arg(&launch.program);
        c
    } else {
        let mut c = Command::new(&launch.program);
        c.envs(launch.env.iter().map(|(k, v)| (k, v))).current_dir(&launch.dir);
        c
    };
    cmd.args(&launch.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| Error::Steam(format!("couldn't start the game: {e}")))
}

/// Whether the game's executable is running on the host (it rewrites its prefix's
/// registry while running, so prefix changes must wait until it's closed).
/// Wine names each process after its exe, so this matches the process name exactly
/// rather than searching command lines, which could mention the exe for other reasons.
pub fn game_running(exe_name: &str) -> Result<bool> {
    // Linux keeps only the first 15 bytes of a process name.
    let name: String = exe_name.chars().take(15).collect();
    let status = host_command("pgrep")
        .args(["-x", "-i", &regex_escape(&name)])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| Error::Steam(format!("couldn't check whether the game is running: {e}")))?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        other => Err(Error::Steam(format!("couldn't check whether the game is running (exit status {other:?})"))),
    }
}

fn regex_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { vec![c] } else { vec!['\\', c] })
        .collect()
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
    let launch = match flavor {
        Flavor::Native => "steam".to_string(),
        Flavor::Flatpak => format!("flatpak run {FLATPAK_STEAM_ID}"),
    };
    host_command("sh")
        .args(["-c", &format!("setsid {launch} >/dev/null 2>&1 &")])
        .stdin(Stdio::null())
        .status()
        .map_err(|e| Error::Steam(format!("could not start Steam: {e}")))?;
    Ok(())
}
