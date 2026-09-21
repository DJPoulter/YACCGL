#!/bin/sh
# Screenshot the GUI against a mock Steam install. Usage: gui-smoke.sh <install_dir> <out.png> [--with-shortcut]
set -e
export HOME=/home/deck GSK_RENDERER=cairo GTK_A11Y=none NO_AT_BRIDGE=1
S=$HOME/.local/share/Steam
mkdir -p $S/config $S/userdata/12345678/config $HOME/.config/not-another-creature-launcher
[ -e $HOME/.steam/steam ] || { mkdir -p $HOME/.steam; ln -s $S $HOME/.steam/steam; }
printf '"users"\n{\n\t"76561197972611406"\n\t{\n\t\t"AccountName"\t\t"deck"\n\t\t"PersonaName"\t\t"Daniel"\n\t\t"MostRecent"\t\t"1"\n\t}\n}\n' > $S/config/loginusers.vdf
[ -f $S/config/config.vdf ] || printf '"InstallConfigStore"\n{\n}\n' > $S/config/config.vdf
printf '{"install_dir":"%s"}' "$1" > $HOME/.config/not-another-creature-launcher/settings.json
if [ "$3" = "--with-shortcut" ]; then /target/nacl-bin steam add --no-artwork >/dev/null; fi
Xvfb :9 -screen 0 1280x800x24 >/dev/null 2>&1 &
sleep 1
DISPLAY=:9 dbus-run-session -- /target/debug/not-another-creature-launcher >/tmp/gui.log 2>&1 &
sleep 8
DISPLAY=:9 import -window root "$2"
grep -viE "^$" /tmp/gui.log | head -20 || true
