# Yet Another Creature Collector Game Launcher (YACCGL)

An unofficial installer for the **standalone PC version of Aniimo** on the Steam Deck and other
Linux PCs. It downloads the game from its official servers, installs it wherever you choose, and
adds it to Steam as a non-Steam game with **Proton 10** already selected.

Not affiliated with Pawprint Interactive, FunPlus or Valve.

The official Windows launcher barely works under Wine or Proton. This tool skips it and installs
the game directly, then sets up the Steam shortcut for you.

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

The Steam account, Proton version, library artwork, launch options and
**Repair** are under **☰ → Preferences**. After changing a shortcut setting, press **Update**
next to the Steam shortcut to apply it.

The download is about 340 MB. On first launch the game downloads the rest of its data
(about 40 GB) itself, so make sure there's room.

### Logging in on the Steam Deck

The game asks for your email in a FunPlus login dialog. In Game Mode, Steam only sends
keyboard input to the game's main window, so the on-screen keyboard can't type into that
dialog.

In Desktop Mode, press **Log In** next to the Steam shortcut. It opens FunPlus's login
window with the same settings Steam uses, so the login is saved for Game Mode. Sign in —
the window may close on its own when you're done; if not, close it yourself. Then start
Aniimo from Steam.

If you skip or miss the email step, the game creates a guest account and doesn't ask again.

### Tips

- Add to Steam disables Steam Input for Aniimo (what the game needs for the controller)
  and forces Proton 10. If a controller still isn't detected, check Properties → Controller
  in Steam and try Enable Steam Input with the Gamepad template instead.
- Start with the Low graphics preset on the Deck.
- Only Proton 10 works for now. Newer Proton versions crash the game.

## Updates

Open the launcher. If a new version of the game is out, the main button changes to **Update**.
The Steam shortcut stays as it is.

To update the launcher itself: ☰ → Preferences → **Check for updates**, or install a newer
Flatpak over the current one:

```bash
flatpak install --user ~/Downloads/yet-another-creature-collector-game-launcher.flatpak
```

## Uninstalling

```bash
flatpak uninstall io.github.DJPoulter.YACCGL
```

The game folder and the Steam shortcut are left alone. Remove the shortcut first with the trash
button in the launcher if you want it gone.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
