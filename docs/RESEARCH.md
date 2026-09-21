# Aniimo standalone launcher — reverse-engineering notes

## Launcher download
- Official URL (embedded in aniimo.com homepage Nuxt state):
  `https://worldx-global-pc-cdn.kingsgroupgames.com/official-website/pc-launcher/aniimo-global.exe`
- Scrape it from `https://www.aniimo.com/` at runtime (regex `aniimo-global\.exe`, unescape `/`) rather than hardcoding.
- Checked 2026-09-21: 84,755,936 bytes, Last-Modified 2026-09-13, installer version 1.0.0.281.

## Installer (`aniimo-global.exe`, original name `Installer.exe`)
- 32-bit custom installer by Pawprint Interactive / FunPlus (Kingsgroup). Not NSIS/Inno.
- Can be unpacked **without running it**: `7z x aniimo-global.exe` gives
  `Launcher.exe`, `UacLauncher.exe`, `uninstall.exe`, `config/version.ini`, `1.0.0.281/…`
  (the payload is resource `.rsrc/1033/SETUP/IDR_SETUP1`, a 7z archive).
- `1.0.0.281/PC-Launcher.exe` is the real Qt5 launcher. `GameUpdater.dll` handles download and unzip (libcurl/cpr + 7z).
  `FPX*.dll` / `fpxcore.dll` is the FunPlus PC SDK (login and remote config).
- Default install layout: `<parent_dir=PawPrint>/<product_dir=Aniimo>/`, main exe `Launcher.exe`.
- Embedded installer config (plain JSON near the end of the exe):
  ```
  appId            worldx.global.prod
  projectId        worldx_global
  package_region   global
  channel_id       Funplus-PC
  package_id       Funplus-PC.Funplus-PC.Apfozx
  sub_channel_code Funplus-PC.official.00000000
  tag              funplus.global.prod.pc_core
  ```
  (a `secret` is also present; used to sign `?tag=…&signature=…&num=1&timestamp=` config requests)

## Launcher update API — SOLVED (2026-09-21)
Found by running the launcher in a Wine + tcpdump Docker sandbox. Hosts contacted:
`pc-client-api.funplus.com` (version API), `worldx-ame.kingsgroupgames.com`,
`fp-logagent-worldx.kingsgroupgames.com` + `upload-s3.funplus.com` + `events.appsflyer.com` (telemetry).
`fpcs-web.*` is the customer-service web app, not the API.

- `GET https://pc-client-api.funplus.com/api/version/list?game_project=worldx_global&primitive_game_id=<installed id>`
  - No auth or signature needed. Plain JSON.
  - `data.primitive_game` = the game: `id`, `version_number`, `md5` (of the .7z), `url`, `back_url` (mirror),
    `exe_start_name` (`Aniimo.exe`), `ext.7z_file_size` (download size). `package_file_size` (~40.9 GB) is
    presumably the total on-disk size after in-game asset download.
  - `data.platfrom` [sic] = the launcher itself (we ignore this).
  - `primitive_game_force_update` is true when `primitive_game_id` != latest `id` → use this as the update check
    (or just compare ids ourselves).
- `GET .../api/version/conf?game_project=worldx_global` → `conf_content` = base64(base64(ciphertext)), 352 bytes,
  encrypted cloud config. Not needed for downloading.
- CDN URLs contain a literal `|` → encode as `%7C`. CDN returns **403 for Python-urllib's default
  User-Agent**; send a browser-like UA. Supports HTTP Range (resumable).

Snapshot 2026-09-21: game id 2402, version 1.0.0.3, md5 `76b8cdf4b45418ea6ef3c8d6404e656e`,
`.../worldx_global%7Cprimitive/prod/package_new/76b8cdf4b45418ea6ef3c8d6404e656e.7z`, 340,386,629 bytes.

## Game package contents
- 7z (LZMA2 + BCJ, non-solid, 120 files, ~938 MB unpacked). Unity IL2CPP client at the archive root:
  `Aniimo.exe`, `GameAssembly.dll`, `UnityPlayer.dll`, `repair.exe`, `NEP2.dll` (NetEase anti-cheat),
  DLSS dlls, `D3D12/`, `Aniimo_Data/`.
- Assets use YooAsset `DefaultPackage` (manifests included, `BuildinFileManifest.txt` empty), so the remaining
  ~40 GB is **downloaded by the game itself at first launch** (still to be confirmed on a real run).
- Install = download .7z → verify md5 → `7z x` into install dir. Nothing from the Qt launcher is needed.

## Old notes (static analysis)
- `GET {funplusApiServer}/api/version/conf?game_project=%1`
- `GET {funplusApiServer}/api/version/list?game_project=%1&primitive_game_id=%2`
- Relevant JSON keys seen: `game_version_code`, `game_version_name`, `engine_version_*`, `data_version`,
  `start_version`/`end_version` (diff patches), `browser_kernel` (CEF package), `default_url`, `md5`.
- Updater behaviour: download package → verify md5 → 7z extract → delete package. Supports pre-download,
  optional updates, and `HashFile.txt`/`fileList.txt`/`chunkList.txt`/`brokenFileList.txt` for repair.
- Launcher logs lines like `[Updater] GameUpdater flow: url=` → **running it once and reading its log
  should reveal the API host and package URLs.**

## Linux/Deck facts
- Only Proton 10.0 works (newer crashes). Use DX11 (default), not DX12.
- Anti-cheat: NetEase Yidun, enabled for Linux.
- Game exe: `Aniimo.exe` in a game subfolder of the install dir.
