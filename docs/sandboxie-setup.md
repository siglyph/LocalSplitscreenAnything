# Real Steam per player with Sandboxie-Plus

Steam is a single-instance client per machine, and a second Windows user account does not get
around that. The community-proven way to run several Steam clients at once is one
[Sandboxie-Plus](https://sandboxie-plus.com) box per extra player, each holding its own Steam
install logged into that player's account. LSA automates the box configuration; the one-time
Steam install and login inside each box is interactive.

## Rules that cannot be worked around

- Every concurrent player needs a Steam account that owns the game. With Steam Families the
  family must own **one copy per concurrent player** of that game.
- Use a **Standard (yellow) box**. App Compartment (green) boxes break Windows credential
  storage, so the boxed Steam asks for password + Steam Guard on every start
  (sandboxie-plus/Sandboxie#3564). Standard boxes are free; no supporter certificate needed.
- Games with kernel anti-cheat will not run inside a box. Some games launched in a box fail to
  reach the boxed Steam; those get `provider = "steam-emu"` (M4) in their profile instead.

## What LSA does (milestone M2)

For player `n` (2..4) it runs, through `SbieIni.exe`:

```
set LSA_Pn Enabled y
set LSA_Pn Comment "LocalSplitscreenAnything LSA_Pn"
set LSA_Pn OpenPipePath C:\ProgramData\LSA\boxes\LSA_Pn\
set LSA_Pn InjectDll64  C:\ProgramData\LSA\boxes\LSA_Pn\lsa_hook64.dll
set LSA_Pn InjectDll    C:\ProgramData\LSA\boxes\LSA_Pn\lsa_hook32.dll
```

then `Start.exe /reload`. The box folder holds that player's `instance.json`; `OpenPipePath`
(unlike `OpenFilePath`) also applies to executables living inside the box, so a boxed game can
read it and write `state.json`. `InjectDll` loads the hook DLL into every process in the box;
the DLL stays dormant in anything that is not the game executable named in `instance.json`.

Launch is `Start.exe /box:LSA_Pn /silent /env:LSA_INSTANCE_DIR=... steam.exe -applaunch <appid>`.
Stopping terminates only the game process, so the boxed Steam stays logged in and warm.

## One-time setup per player (manual until the M2 wizard exists)

1. Install Sandboxie-Plus. Run `lsa play <steam-profile> -p 2 --dry-run` once; it shows the box
   name. Create the box by running the `SbieIni.exe` lines above (or let `lsa play` without
   `--dry-run` do it; that needs the hook DLLs next to `lsa.exe`).
2. In Sandboxie-Plus, run `SteamSetup.exe` inside `LSA_P2` and install Steam to its default
   location. Start the boxed Steam, log in as player 2, and tick "Remember me".
3. In the boxed Steam, Settings > Storage > Add Drive and add the host's Steam library folder.
   Reads fall through to the host install (Sandboxie's copy-on-write), so the game is not
   downloaded again; the boxed Steam verifies the files once. Writes (saves, config) stay in the
   box, which is exactly what we want.
4. Launch the game once from the boxed Steam to accept first-run dialogs and set windowed mode.

Known gap: `[[patch]]` entries in a profile currently patch host paths. For boxed players the
same path lives under the box's file root (`C:\Sandbox\<user>\LSA_Pn\user\current\...`);
patching those is part of M2.
