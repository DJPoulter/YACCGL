#!/bin/sh
# Stand-in for Proton in tests: records how it was started, then "runs" the game.
{ echo "cwd=$(pwd)"; echo "args=$*"; env | grep -E '^(STEAM_COMPAT_|SteamAppId|SteamGameId)' | sort; } > /tmp/proton-called.txt
sleep "${FAKE_GAME_SECONDS:-12}"
