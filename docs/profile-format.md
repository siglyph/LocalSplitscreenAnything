# Profile format

A profile is one TOML file per game in a `profiles/` directory (next to `lsa.exe`, in the
current directory, or in `%APPDATA%\LSA\profiles`, the last one winning). `_default.toml` is
merged underneath every profile: tables merge recursively, arrays and scalars in the game
profile replace the defaults. Unknown keys are errors, so typos surface immediately
(`lsa profiles validate path.toml`).

Placeholders usable in `launch.args`, `launch.env`, patch files and values, and `[prism]`
account/server strings: `${player}` (1-based), `${nickname}`, `${width}`, `${height}`, `${x}`,
`${y}` (viewport in virtual-desktop pixels), `${monitor}`, `${instance_dir}`, `${session_id}`.
Paths may also use `%VAR%` environment references.

```toml
default_layout = "grid"        # optional preset id used when --layout is not given

[game]
id = "terraria"                # file name without .toml; letters, digits, - and _
name = "Terraria"
provider = "steam"             # exe | steam | prism | steam-emu
steam_appid = 105600           # steam providers
executable = "Terraria.exe"    # process to manage/hook (required for steam providers)
path = "C:/Games/X/X.exe"      # exe provider: the executable
working_dir = "C:/Games/X"     # optional
max_players = 4                # 1..=4
notes = "free text"

[isolation]
mode = "none"                  # none | env | sandboxie
instance_dir = "none"          # none | symlink | copy  (per-instance game folder; M4)
env_redirect = ["APPDATA", "LOCALAPPDATA", "USERPROFILE", "TEMP", "TMP"]   # env mode

[launch]
args = ["-windowed", "-w", "${width}", "-h", "${height}"]
stagger_ms = 3000              # delay between instance starts
env = { SOME_VAR = "value" }

[launch.window]
title_regex = "^Terraria"      # how to recognise the main window
class = "UnityWndClass"        # optional exact window class
wait_ms = 60000                # how long to wait for the window

[hooks]
inject = true                  # false: never inject the DLL (Minecraft)
xinput = true                  # game sees only its assigned controller
fake_focus = true              # game believes it is always focused
force_windowed = true          # refuse exclusive fullscreen
lie_screen_size = true         # report the viewport as the screen size
pin_window = true              # keep the window at its viewport
clip_cursor = false            # confine the cursor to the viewport
rename_handles = ["Global\\SomeMutex"]   # single-instance objects to rename per instance
rename_all_handles = false     # rename every named kernel object (aggressive)

[[patch]]                      # per-instance config edits before launch, restored on stop
file = "%USERPROFILE%/Documents/My Games/Terraria/config.json"
format = "json"                # json | ini | key-value-colon | key-value-equals | regex
section = "Video"              # ini only
set = { Fullscreen = false, DisplayWidth = "${width}" }   # json keys may be dotted paths
pattern = "Width=\\d+"         # regex only
replace = "Width=${width}"     # regex only
create_if_missing = false

[prism]                        # provider = "prism"
launcher = "C:/.../prismlauncher.exe"   # auto-detected when omitted
data_dir = "C:/.../PrismLauncher"       # auto-detected when omitted
instances = ["Player1", "Player2"]      # Prism instance folder names, one per player
accounts = ["jude", "sam"]              # Prism account profile names, one per player
server = "127.0.0.1:25565"              # optional: every instance joins this server
patch_options = true                    # windowed, keep running unfocused, size
patch_instance_cfg = true               # window size in Prism's instance settings

[steam]                        # provider = "steam"
host_slot_unsandboxed = true   # player 1 uses the host's Steam; others get Sandboxie boxes
steam_exe = "C:/Program Files (x86)/Steam/steam.exe"   # auto-detected from the registry
```

## Providers

- **exe**: runs `game.path` with `launch.args`. The game process is the spawned process (or a
  process with the same exe name if it relaunches itself).
- **steam**: runs `steam.exe -applaunch <appid> <args>`. Steam starts the game, so the game is
  found by `game.executable`. With `isolation.mode = "sandboxie"`, players other than the host
  slot run inside their box's Steam (`docs/sandboxie-setup.md`).
- **prism**: runs `prismlauncher --launch <instance> --profile <account> [--server ...]`,
  after patching `options.txt` (`fullscreen:false`, `pauseOnLostFocus:false`, override size)
  and `instance.cfg` (window size). Minecraft needs no hooks (`[hooks] inject = false`).
- **steam-emu**: not implemented yet (M4).

## Finding handle names

Run the game once, open Process Explorer, select the process, View > Lower Pane View > Handles,
and look for `Mutant`/`Event`/`Section` objects with a game-specific name. Put the exact name in
`rename_handles`. If the game still refuses a second instance, try `rename_all_handles = true`.
