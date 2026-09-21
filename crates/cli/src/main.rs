//! Command-line front end, mainly for scripting and testing without the GUI.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use yaccgl_core::install::{self, Progress, Status};
use yaccgl_core::settings::Settings;
use yaccgl_core::space::human;
use yaccgl_core::steam::{self, ShortcutSpec, Steam, process};
use yaccgl_core::{GAME_NAME, api, http};

#[derive(Parser)]
#[command(name = "yaccgl", version, about = "Install Aniimo and add it to Steam")]
struct Cli {
    /// Install directory (defaults to the saved setting, then ~/Games/Aniimo).
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show installed and latest versions and Steam status.
    Status,
    /// Install the game, or update it if a new version is out.
    Install {
        /// Re-download and re-extract even if up to date.
        #[arg(long)]
        repair: bool,
    },
    /// Manage the Steam shortcut.
    #[command(subcommand)]
    Steam(SteamCmd),
}

#[derive(Subcommand)]
enum SteamCmd {
    /// List Steam accounts found on this machine.
    Users,
    /// Add (or update) the non-Steam shortcut and force a Proton version.
    Add {
        #[arg(long)]
        user: Option<u32>,
        /// Steam compat tool internal name.
        #[arg(long, default_value = steam::DEFAULT_COMPAT_TOOL)]
        tool: String,
        #[arg(long, default_value = "")]
        launch_options: String,
        #[arg(long)]
        no_artwork: bool,
        /// Close Steam first if it is running, and start it again afterwards.
        #[arg(long)]
        restart_steam: bool,
    },
    /// Remove the shortcut, its Proton setting and artwork.
    Remove {
        #[arg(long)]
        user: Option<u32>,
        #[arg(long)]
        restart_steam: bool,
    },
    /// Show Steam detection details and every shortcut, for troubleshooting.
    Doctor,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut settings = Settings::load();
    if let Some(dir) = &cli.dir {
        settings.install_dir = absolute(dir)?;
    }
    match cli.cmd {
        Cmd::Status => status(&settings),
        Cmd::Install { repair } => install(&mut settings, repair),
        Cmd::Steam(SteamCmd::Users) => users(),
        Cmd::Steam(SteamCmd::Add { user, tool, launch_options, no_artwork, restart_steam }) => {
            settings.compat_tool = tool;
            settings.launch_options = launch_options;
            settings.artwork = !no_artwork;
            settings.steam_user = user.or(settings.steam_user);
            steam_add(&settings, restart_steam)?;
            settings.save()?;
            Ok(())
        }
        Cmd::Steam(SteamCmd::Remove { user, restart_steam }) => {
            steam_remove(&settings, user.or(settings.steam_user), restart_steam)
        }
        Cmd::Steam(SteamCmd::Doctor) => steam_doctor(&settings),
    }
}

fn absolute(p: &PathBuf) -> Result<PathBuf> {
    Ok(if p.is_absolute() { p.clone() } else { std::env::current_dir()?.join(p) })
}

fn status(settings: &Settings) -> Result<()> {
    let agent = http::agent();
    let dir = &settings.install_dir;
    println!("Install directory: {}", dir.display());
    let latest = api::latest_game(&agent).context("checking for the latest version")?;
    println!("Latest version:    {} (build {})", latest.version_number, latest.id);
    match install::status(dir, &latest) {
        Status::NotInstalled => println!("Installed:         no"),
        Status::UpToDate(s) => println!("Installed:         {} (up to date)", s.package.version_number),
        Status::UpdateAvailable { installed, .. } => {
            println!("Installed:         {} (update available)", installed.package.version_number)
        }
    }
    let appid = steam::appid_for(&install::exe_path(dir, &latest), GAME_NAME);
    match pick_steam(settings.steam_user) {
        Ok((steam, user)) => {
            println!("Steam:             {} (user {})", steam.root.display(), user.account_id);
            println!("Shortcut:          {}", if steam.shortcut_exists(&user, appid) { "added" } else { "not added" });
            println!("Compat tool:       {}", steam.compat_tool(appid).unwrap_or_else(|| "Steam default".into()));
        }
        Err(e) => println!("Steam:             {e}"),
    }
    Ok(())
}

fn install(settings: &mut Settings, repair: bool) -> Result<()> {
    let agent = http::agent();
    let dir = settings.install_dir.clone();
    let latest = api::latest_game(&agent).context("checking for the latest version")?;
    match install::status(&dir, &latest) {
        Status::UpToDate(s) if !repair => {
            println!("Aniimo {} is already installed and up to date.", s.package.version_number);
            return Ok(());
        }
        Status::UpdateAvailable { installed, .. } => println!(
            "Updating {} -> {} in {}",
            installed.package.version_number,
            latest.version_number,
            dir.display()
        ),
        _ => println!("Installing Aniimo {} into {}", latest.version_number, dir.display()),
    }

    let cancel = AtomicBool::new(false);
    let mut bar = Bar::default();
    install::install(&agent, &dir, &latest, &cancel, &mut |p| bar.update(p))?;
    bar.finish();
    settings.save()?;
    println!("Done. Run `yaccgl steam add` to add it to Steam.");
    Ok(())
}

fn users() -> Result<()> {
    let installs = Steam::detect();
    if installs.is_empty() {
        bail!("no Steam installation found");
    }
    for steam in installs {
        println!("{} ({:?})", steam.root.display(), steam.flavor);
        for u in steam.users() {
            let name = u.persona.as_deref().unwrap_or("?");
            let recent = if u.most_recent { "  [most recent]" } else { "" };
            println!("  {:>12}  {name}{recent}", u.account_id);
        }
    }
    Ok(())
}

fn pick_steam(user: Option<u32>) -> Result<(Steam, steam::User)> {
    for steam in Steam::detect() {
        let users = steam.users();
        let found = match user {
            Some(id) => users.into_iter().find(|u| u.account_id == id),
            None => users.into_iter().next(),
        };
        if let Some(u) = found {
            return Ok((steam, u));
        }
    }
    match user {
        Some(id) => bail!("Steam user {id} not found (see `yaccgl steam users`)"),
        None => bail!("no Steam installation with a logged-in user found"),
    }
}

fn exe_path(settings: &Settings) -> PathBuf {
    let dir = &settings.install_dir;
    dir.join(
        install::read_state(dir)
            .map(|s| s.package.exe_name().to_owned())
            .unwrap_or_else(|| yaccgl_core::DEFAULT_EXE.into()),
    )
}

/// Report what happened around a Steam restart, failing if Steam discarded the change.
fn check_restart<T>(applied: &steam::Applied<T>) -> Result<()> {
    if applied.restarted {
        println!("Steam was closed for the change and started again.");
    }
    if applied.survived_restart == Some(false) {
        bail!(
            "Steam overwrote the change when it restarted, which means another Steam process was still running. \
             Exit Steam completely (Steam menu > Exit), then run this command again."
        );
    }
    Ok(())
}

fn steam_add(settings: &Settings, restart: bool) -> Result<()> {
    let dir = &settings.install_dir;
    let exe = exe_path(settings);
    if !exe.is_file() {
        bail!("{} not found. Install the game first.", exe.display());
    }
    let (steam, user) = pick_steam(settings.steam_user)?;
    let spec = ShortcutSpec {
        name: GAME_NAME.into(),
        exe,
        start_dir: dir.clone(),
        launch_options: settings.launch_options.clone(),
        compat_tool: Some(settings.compat_tool.clone()),
        artwork: settings.artwork,
    };
    let appid = steam::appid_for(&spec.exe, &spec.name);
    if restart && process::is_running()? {
        println!("Closing Steam...");
    }
    let applied = steam.with_closed(
        restart,
        || steam.register(&http::agent(), &user, &spec),
        || steam.shortcut_exists(&user, appid),
    )?;
    check_restart(&applied)?;
    let reg = applied.value;
    println!(
        "{} shortcut {} for user {} with {}.",
        if reg.created { "Added" } else { "Updated" },
        reg.appid,
        user.account_id,
        settings.compat_tool
    );
    if let Some(e) = reg.artwork_error {
        println!("Artwork could not be downloaded: {e}");
    } else if settings.artwork {
        println!("Installed {} artwork files.", reg.artwork.len());
    }
    Ok(())
}

fn steam_remove(settings: &Settings, user: Option<u32>, restart: bool) -> Result<()> {
    let appid = steam::appid_for(&exe_path(settings), GAME_NAME);
    let (steam, user) = pick_steam(user)?;
    let applied = steam.with_closed(
        restart,
        || steam.unregister(&user, appid),
        || !steam.shortcut_exists(&user, appid),
    )?;
    check_restart(&applied)?;
    println!("Removed shortcut {appid}.");
    Ok(())
}

/// Print everything needed to work out why a shortcut isn't showing up.
fn steam_doctor(settings: &Settings) -> Result<()> {
    let exe = exe_path(settings);
    let appid = steam::appid_for(&exe, GAME_NAME);
    println!("Running in Flatpak:  {}", process::in_flatpak());
    println!("Game Mode:           {}", process::in_game_mode());
    match process::is_running() {
        Ok(r) => println!("Steam running:       {r}"),
        Err(e) => println!("Steam running:       unknown ({e})"),
    }
    println!("Game exe:            {} ({})", exe.display(), if exe.is_file() { "found" } else { "MISSING" });
    println!("Expected app id:     {appid}");

    let installs = Steam::detect();
    if installs.is_empty() {
        println!("\nNo Steam installation found.");
    }
    for steam in installs {
        println!("\nSteam: {} ({:?})", steam.root.display(), steam.flavor);
        println!("  Compat tool for {appid}: {}", steam.compat_tool(appid).unwrap_or_else(|| "none".into()));
        for user in steam.users() {
            let path = steam.shortcuts_vdf(&user);
            let modified = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map_or("missing".into(), |d| format!("modified {}s ago", d.as_secs()));
            println!(
                "  User {} {}{}: {} ({modified})",
                user.account_id,
                user.persona.as_deref().unwrap_or("?"),
                if user.most_recent { " [most recent]" } else { "" },
                path.display(),
            );
            match steam.shortcuts(&user) {
                Ok(list) if list.is_empty() => println!("    (no shortcuts)"),
                Ok(list) => {
                    for (id, name, exe) in list {
                        let mark = if id == appid { "  <-- YACCGL" } else { "" };
                        println!("    {id:>10}  {name}  {exe}{mark}");
                    }
                }
                Err(e) => println!("    could not read: {e}"),
            }
        }
    }
    Ok(())
}

/// Single-line progress output that works in plain terminals and logs.
#[derive(Default)]
struct Bar {
    last: Option<Instant>,
    stage: &'static str,
    speed_from: Option<(Instant, u64)>,
}

impl Bar {
    fn update(&mut self, p: Progress) {
        let (stage, done, total) = match p {
            Progress::Downloading { done, total } => ("Downloading", done, total),
            Progress::Verifying { done, total } => ("Verifying", done, Some(total)),
            Progress::Extracting { done, total } => ("Extracting", done, Some(total)),
        };
        let now = Instant::now();
        if stage != self.stage {
            if !self.stage.is_empty() {
                println!();
            }
            self.stage = stage;
            self.speed_from = Some((now, done));
        } else if self.last.is_some_and(|l| now - l < Duration::from_millis(250)) && total != Some(done) {
            return;
        }
        self.last = Some(now);
        let pct = total.filter(|&t| t > 0).map_or(String::new(), |t| format!("{:5.1}% ", done as f64 * 100.0 / t as f64));
        let speed = self
            .speed_from
            .map(|(t0, d0)| (now - t0).as_secs_f64().max(0.001).recip() * done.saturating_sub(d0) as f64)
            .map_or(String::new(), |s| format!("  {}/s", human(s as u64)));
        let of = total.map_or(String::new(), |t| format!(" / {}", human(t)));
        print!("\r{stage}: {pct}{}{of}{speed}        ", human(done));
        let _ = std::io::stdout().flush();
    }

    fn finish(&self) {
        println!();
    }
}
