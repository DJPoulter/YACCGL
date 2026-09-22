Aniimo FPX login host
=====================

Development / manual Deck testing for the FunPlus login dialog.

The launcher embeds `data/fpx-login/fpx_login.exe` + `config.json` and copies them
into `Aniimo_Data/Plugins/x86_64/` when you install or press Log In.

Rebuild the exe (from this folder's host.c) with wine10 + mingw, then copy to
`data/fpx-login/` so `include_bytes!` picks it up. See the scratchpad
`rebuild_exe.sh` or:

  x86_64-w64-mingw32-gcc -O2 -mwindows host.c -o fpx_login.exe -lgdi32 -luser32

Auto-close: best-effort. The host watches FPX callbacks and child windows; if
the login panel closes after appearing it exits. If that doesn't fire, close the
window yourself — the session is still saved in the Proton prefix.
