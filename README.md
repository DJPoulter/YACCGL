# Yet Another Creature Collector Game Launcher (YACCGL)

An unofficial installer for the **standalone PC version of Aniimo** on the Steam Deck and other
Linux PCs. It downloads the game from its official servers, installs it wherever you choose, and
adds it to Steam as a non-Steam game with **Proton 10** already selected.

Not affiliated with Pawprint Interactive, FunPlus or Valve.

## Why

The official Windows launcher barely renders under Wine or Proton, so the usual route is to install it in a VM
and copy files across, then set up the shortcut and Proton version by hand. This tool skips the
official launcher entirely: it talks to the same version API and CDN the launcher uses.

## Installing on a Steam Deck

1. Switch to **Desktop Mode** (Steam button → Power → Switch to Desktop).
2. Download `yet-another-creature-collector-game-launcher.flatpak` from the
   [latest release](https://github.com/DJPoulter/YACCGL/releases/latest).
3. Install it, either way works:
   - **Discover:** open your Downloads folder in Dolphin and double-click the file, then press
     **Install**.
   - **Terminal (Konsole):**
     ```bash
     flatpak install --user ~/Downloads/yet-another-creature-collector-game-launcher.flatpak
     ```
     Answer `y` when asked to install the GNOME runtime from Flathub (a few hundred MB, only needed
     the first time).
4. Open **Yet Another Creature Collector Game Launcher** from the application menu (under
   Games).
5. Pick an install location (the default is `~/Games/Aniimo`; SD cards work too) and press
   **Install**.
6. When it finishes, choose **Add to Steam**. If Steam is running it is closed and restarted,
   because Steam overwrites its shortcut list when it exits.
7. Go back to Game Mode. Aniimo is in your library, set to run with Proton 10.

The Steam account, Proton version, library artwork, launch options, single-window mode and
**Repair** are under **☰ → Preferences**. After changing a shortcut setting, press **Update**
next to the Steam shortcut to apply it.

### Logging in on the Steam Deck

The game asks for your email in a FunPlus login dialog. In Game Mode, Steam only sends
keyboard input to the game's main window, so the on-screen keyboard can't type into that
dialog. What's built in:

- **Log In button:** in Desktop Mode, press **Log In** next to the Steam shortcut. It opens
  FunPlus's login window (via `FPX.dll`) with the same Proton prefix Steam uses, so the login
  is saved for Game Mode. Sign in — the window may close on its own when you're done; if not,
  close it yourself. Then start Aniimo from Steam.
- **Launcher updates:** ☰ → Preferences → **Check for updates** installs a newer Flatpak over
  the current one (no uninstall). Or from a terminal:
  `flatpak install --user ~/Downloads/yet-another-creature-collector-game-launcher.flatpak`
- **Single window** (off by default, in Preferences): runs the game inside one fixed-size window
  using Wine's virtual desktop. It didn't fix the login window in Game Mode in testing, so it's
  opt-in only.

If you skip or miss the email step, the game creates a guest account and doesn't ask again.

To uninstall the launcher: `flatpak uninstall io.github.DJPoulter.YACCGL`. The game folder and the
Steam shortcut are left alone; remove the shortcut first with the trash button if you want it gone.

The download is about 340 MB. On first launch the game downloads the rest of its data
(about 40 GB) itself, so make sure there's room.

### Tips

- Add to Steam disables Steam Input for Aniimo (what the game needs for the controller)
  and forces Proton 10. If a controller still isn't detected, check Properties → Controller
  in Steam and try Enable Steam Input with the Gamepad template instead.
- Start with the Low graphics preset on the Deck.
- Only Proton 10 works for now. Newer Proton versions crash the game.

## Updates

Open the launcher. If a new version is out, the main button changes to **Update**. The Steam
shortcut stays as it is.

## Command line

The same features are available without the GUI. The Flatpak includes the `yaccgl` command:

```bash
alias yaccgl='flatpak run --command=yaccgl io.github.DJPoulter.YACCGL'

yaccgl status                         # installed/latest version, Steam shortcut state
yaccgl --dir ~/Games/Aniimo install   # install or update
yaccgl install --repair               # re-download and re-extract
yaccgl steam users                    # list Steam accounts
yaccgl steam add --restart-steam      # add/update the shortcut, force Proton 10, add artwork
yaccgl steam remove --restart-steam
yaccgl steam doctor                   # troubleshooting: Steam detection and every shortcut
yaccgl launch                         # start the game like Steam does, e.g. to log in
```

## How it works

- `GET https://pc-client-api.funplus.com/api/version/list?game_project=worldx_global` returns the
  current build, its MD5 and CDN URLs (see [docs/RESEARCH.md](docs/RESEARCH.md)).
- The `.7z` package is downloaded with resume support, checked against the MD5, and extracted into
  the install directory. The installed build is recorded in `<install dir>/.yaccgl/state.json`.
- Steam integration edits `userdata/<id>/config/shortcuts.vdf` (binary KeyValues) and the
  `CompatToolMapping` section of `config/config.vdf`, keeping a `.yaccgl-bak` backup of each.
  Steam overwrites these files when it exits, which is why it has to be closed while they're
  edited.
- Artwork comes from Aniimo's official Steam store assets.
- The install folder is linked as drive `G:` in the game's Proton prefix
  (`steamapps/compatdata/<id>/pfx/dosdevices/g:`). Steam runs Proton in a container whose root
  is a RAM disk, so without this the game runs from `Z:\` and its free-space check sees only
  a few GB. With it, the game runs from `G:\` and sees the real disk.
- Single-window mode turns on Wine's virtual desktop in the prefix's `user.reg`
  (`HKCU\Software\Wine\Explorer`). Before the game's first launch, that file is created from
  Proton's own template with the setting added; Proton copies the rest of the prefix around it.
- **Log In** runs the game the way Steam does: Proton inside the Steam Linux Runtime, with the
  shortcut's `STEAM_COMPAT_*` environment, so it uses the same prefix.

## Development

The code is a Cargo workspace:

| Crate         | Purpose                                                        |
|---------------|----------------------------------------------------------------|
| `crates/core` | API client, downloader, 7z extraction, Steam integration        |
| `crates/cli`  | `yaccgl` command-line tool                                        |
| `crates/gui`  | GTK4 + libadwaita app                                           |

On Linux, install Rust plus the GTK4 and libadwaita development packages, then run `cargo run -p yaccgl-gui`.

Everything also builds in Docker, which is how it's developed on Windows:

```bash
docker build -t yaccgl-dev -f docker/dev.Dockerfile docker
docker run --rm -v "$PWD:/src" yaccgl-dev cargo test --workspace
```

Build the Flatpak bundle:

```bash
docker build -t yaccgl-flatpak -f docker/flatpak.Dockerfile docker
docker run --rm --privileged -v "$PWD:/src" yaccgl-flatpak sh -c \
  'flatpak-builder --disable-rofiles-fuse --install-deps-from=flathub --force-clean --repo=repo build-dir flatpak/io.github.DJPoulter.YACCGL.yml &&
   flatpak build-bundle repo yet-another-creature-collector-game-launcher.flatpak io.github.DJPoulter.YACCGL --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo'
```

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
