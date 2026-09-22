#!/bin/bash
# Run fpx_login.exe under Proton using Aniimo's Steam shortcut prefix.
# Usage: ./run-on-deck.sh [path-to-Aniimo-install]
set -euo pipefail

GAME_DIR="${1:-$HOME/Games/Aniimo}"
PLUGIN="$GAME_DIR/Aniimo_Data/Plugins/x86_64"
EXE="$PLUGIN/fpx_login.exe"
CFG="$PLUGIN/config.json"

if [[ ! -f "$EXE" || ! -f "$CFG" ]]; then
  echo "Put fpx_login.exe and config.json in:" >&2
  echo "  $PLUGIN" >&2
  exit 1
fi

STEAM_ROOT="${STEAM_ROOT:-$HOME/.steam/steam}"
[[ -d "$STEAM_ROOT/steamapps" ]] || STEAM_ROOT="$HOME/.local/share/Steam"

APPID="${APPID:-}"
if [[ -z "$APPID" ]]; then
  for pfx in "$STEAM_ROOT"/steamapps/compatdata/*/pfx; do
    [[ -e "$pfx/dosdevices/g:" ]] || continue
    target=$(readlink -f "$pfx/dosdevices/g:" 2>/dev/null || true)
    if [[ "$target" == *"Aniimo"* ]] || [[ "$target" == "$GAME_DIR" ]]; then
      APPID=$(basename "$(dirname "$pfx")")
      break
    fi
  done
fi

if [[ -z "$APPID" ]]; then
  echo "Could not find Aniimo's Proton prefix automatically." >&2
  echo "Set APPID yourself (from YACCGL / compatdata folder name), e.g.:" >&2
  echo "  APPID=4017823374 $0" >&2
  exit 1
fi

PROTON=$(ls -d "$STEAM_ROOT"/steamapps/common/Proton\ 10* 2>/dev/null | head -1 || true)
if [[ -z "$PROTON" || ! -x "$PROTON/proton" ]]; then
  echo "Proton 10 not found under $STEAM_ROOT/steamapps/common" >&2
  exit 1
fi

echo "prefix appid=$APPID"
echo "proton=$PROTON"
export STEAM_COMPAT_DATA_PATH="$STEAM_ROOT/steamapps/compatdata/$APPID"
export STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_ROOT"
cd "$PLUGIN"
exec "$PROTON/proton" run ./fpx_login.exe
