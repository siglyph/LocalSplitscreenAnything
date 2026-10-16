# LocalSplitscreenAnything

Run 2–4 instances of a PC game on one Windows machine and tile them on your screens, so any
game with LAN or online multiplayer becomes couch co-op. Each player gets their own instance,
their own controller, and (for Steam games) their own Steam account.

**Status: milestone M0 scaffold.** The core library, command line and profile format exist and
are unit-tested; the hook DLL is a scaffold with no hooks yet; nothing has been run on a Windows
PC with real games yet. See [the roadmap](#roadmap).

## How it works

No virtual machines. The orchestrator launches N ordinary game processes and isolates only the
things that collide:

| Concern | Mechanism |
|---|---|
| Window placement | Finds each instance's main window, strips its borders, pins it to its viewport (`lsa-core/src/window.rs`). |
| Controllers | A DLL injected into each instance remaps XInput so the game sees only its assigned pad (`lsa-hook`, milestone M1). |
| Focus | The same DLL makes every instance believe it is the foreground window (M1). |
| Single-instance checks | Named mutexes/events are renamed per instance (M1). |
| Saves / config / Steam login | `env` isolation (per-instance `APPDATA` etc.) or one **Sandboxie-Plus** box per player with its own Steam client (M2). |
| Launchers | Providers: plain `exe`, `steam` (real client per player), `prism` (Minecraft Java through Prism Launcher), `steam-emu` (Goldberg fallback, M4). |

Each game has a small TOML **profile** describing which provider, arguments, hooks and config
patches it needs; see [docs/profile-format.md](docs/profile-format.md).

## Requirements

- Windows 10/11 x64 for playing. (Layout previews and `--dry-run` also work on macOS/Linux.)
- Rust stable with the `x86_64-pc-windows-msvc` and `i686-pc-windows-msvc` targets
  (`rust-toolchain.toml` adds them) and Visual Studio 2022 Build Tools (C++ workload).
- For Minecraft: [Prism Launcher](https://prismlauncher.org), one instance per player, the
  Controlify mod for gamepads.
- For real-Steam-per-player: [Sandboxie-Plus](https://sandboxie-plus.com) and one copy of the
  game per concurrent player (Steam Families counts). See [docs/sandboxie-setup.md](docs/sandboxie-setup.md).

## Build

On the Windows PC:

```powershell
.\build.ps1
```

This builds `lsa.exe` plus `lsa_hook64.dll` and `lsa_hook32.dll` and collects them with the
profiles in `dist\`. Or by hand:

```bash
cargo build --release -p lsa-cli
cargo build --profile hook -p lsa-hook --target x86_64-pc-windows-msvc
cargo build --profile hook -p lsa-hook --target i686-pc-windows-msvc
```

Everywhere: `cargo test` runs the unit tests, `cargo check --workspace --target x86_64-pc-windows-msvc`
type-checks the Windows code from another OS.

## Quick start

```bash
lsa profiles list                                  # shipped profiles
lsa monitors                                       # what the layout engine sees
lsa layouts list -p 3                              # presets for 3 players
lsa layouts preview big-left -p 4                  # ASCII preview on your monitors
lsa controllers assign -p 2                        # each player presses A; prints -c flags
lsa play minecraft-java -p 2 --layout left-right -c 1=0 -c 2=1
lsa play terraria -p 2 --dry-run                   # show what would happen, launch nothing
lsa stop                                           # clean up after a crashed session
```

`lsa play` keeps running while the games run; Ctrl+C stops every instance and restores any
config file it patched.

## Layouts

Presets, chosen per player count: `left-right`, `top-bottom`, `grid` (3 players leave one cell
empty, pick which with `--empty-cell tl|tr|bl|br`), `columns`, `rows`, `big-left`,
`big-right`, `big-top`, `big-bottom`, `one-per-monitor`, `two-per-monitor`,
`two-per-monitor-stacked`. The big+small presets keep the small viewports at the monitor's
aspect ratio.

Custom layouts are TOML files with normalised rectangles; `lsa layouts save grid -p 4 -o my.toml`
writes one to edit by hand, and `--layout my.toml` uses it. Files in `%APPDATA%\LSA\layouts\`
can be referenced by name. The Tauri UI (M3) adds a drag-and-drop editor on top of the same model.

## Repository layout

```
crates/lsa-proto   types shared with the hook DLL (instance.json / state.json)
crates/lsa-core    profiles, layouts, providers, isolation, window management, sessions
crates/lsa-cli     the `lsa` command line
crates/lsa-hook    DLL injected into game instances (scaffold; hooks arrive in M1)
profiles/          shipped game profiles (_default.toml is merged under all of them)
docs/              profile format, Sandboxie setup, manual test checklist
build.ps1          Windows build script
```

## Roadmap

- **M0** (this): workspace, profiles, layout engine, window manager, exe/Prism/Steam providers,
  env isolation, CLI, hook DLL scaffold. Validate on Windows with Minecraft (no hooks needed).
- **M1**: hook DLL v1 (XInput remap, fake focus, handle rename, window pin/screen-size lies,
  force windowed) and the injector for unsandboxed games.
- **M2**: Sandboxie boxes with a real Steam client per player; player setup wizard; patching of
  config files that live inside a box.
- **M3**: Tauri UI with library, controller assignment, layout picker and editor.
- **M4**: Goldberg fallback provider, DirectInput/HID filtering, Windows.Gaming.Input shim.
- **M5**: per-player audio routing, installer, more profiles.

## Limits

The game must have some way for instances to connect (LAN, direct IP, Steam lobbies, a mod);
single-player games do not become co-op. Games with kernel anti-cheat generally refuse injected
DLLs and sandboxes. Minecraft Bedrock has no multi-instance support on Windows; use Java.
