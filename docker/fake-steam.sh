#!/bin/sh
# Imitates the Steam client for tests: loads shortcuts.vdf into memory on start and
# writes that copy back over the file when it exits, like the real client.
F=$HOME/.local/share/Steam/userdata/12345678/config/shortcuts.vdf
PID=$HOME/.steam/steam.pid
if [ "$1" = "-shutdown" ]; then
  kill -TERM "$(cat "$PID")" 2>/dev/null
  exit 0
fi
cp "$F" /tmp/steam-memory.vdf 2>/dev/null || : > /tmp/steam-memory.vdf
echo $$ > "$PID"
trap 'sleep 1; cp /tmp/steam-memory.vdf "$F"; rm -f "$PID"; exit 0' TERM
while :; do sleep 0.2; done
