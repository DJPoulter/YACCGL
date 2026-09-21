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

1. Switch to **Desktop Mode**.
2. Download `yet-another-creature-collector-game-launcher.flatpak`, then in a terminal (Konsole):
   ```bash
   flatpak install --user ./yet-another-creature-collector-game-launcher.flatpak
   ```
3. Open **Yet Another Creature Collector Game Launcher** from the application menu.
4. Pick an install location (the default is `~/Games/Aniimo`; SD cards work too) and press **Install**.
5. When it finishes, choose **Add to Steam**. Steam restarts once so the shortcut can be written.
6. Go back to Game Mode. Aniimo is in your library, set to run with Proton 10.

The download is about 340 MB. On first launch the game downloads the rest of its data
(about 40 GB) itself, so make sure there's room.

### Tips

- If your controller isn't detected in game, open the game's controller settings in Steam and
  turn off Steam Input for it.
- Start with the Low graphics preset on the Deck.
- Only Proton 10 works for now. Newer Proton versions crash the game.

## Updates

Open the launcher. If a new version is out, the main button changes to **Update**. The Steam
shortcut stays as it is.

## Command line

The same features are available without the GUI:

```bash
yaccgl status                         # installed/latest version, Steam shortcut state
yaccgl --dir ~/Games/Aniimo install   # install or update
yaccgl install --repair               # re-download and re-extract
yaccgl steam users                    # list Steam accounts
yaccgl steam add --restart-steam      # add/update the shortcut, force Proton 10, add artwork
yaccgl steam remove --restart-steam
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
  'flatpak-builder --disable-rofiles-fuse --install-deps-from=flathub --force-clean --repo=repo build-dir flatpak/io.github.yaccgl.Launcher.yml &&
   flatpak build-bundle repo yet-another-creature-collector-game-launcher.flatpak io.github.yaccgl.Launcher --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo'
```

## License

GPL-3.0-or-later.
