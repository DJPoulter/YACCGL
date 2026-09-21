#!/bin/sh
# Installs a fake Proton 10 + Steam Linux Runtime into the mock Steam at $HOME.
S=$HOME/.local/share/Steam
P="$S/steamapps/common/Proton 10.0"; R="$S/steamapps/common/SteamLinuxRuntime_sniper"
mkdir -p "$P/files/share/default_pfx" "$R"
install -m755 /src/docker/fake-proton.sh "$P/proton"
install -m755 /src/docker/fake-runtime-entry.sh "$R/_v2-entry-point"
printf '"manifest"\n{\n\t"require_tool_appid"\t\t"1628350"\n}\n' > "$P/toolmanifest.vdf"
printf '"AppState"\n{\n\t"appid"\t\t"1628350"\n\t"installdir"\t\t"SteamLinuxRuntime_sniper"\n}\n' > "$S/steamapps/appmanifest_1628350.acf"
