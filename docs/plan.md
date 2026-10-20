# LocalSplitscreenAnything — architecture and build plan

## Context

Goal: a Windows tool that turns any multiplayer-capable PC game into local splitscreen for 2–4 players by running one game instance per player on a single machine. Requirements gathered so far:

- Target OS for play: **Windows gaming PC** (dev + test happens there; the Mac is only for editing/git).
- Launchers: **not Steam-only**. Steam games, Minecraft Java (Prism Launcher), and plain executables must all work. Steam is one "launch provider" among several.
- Steam accounts: **real Steam per player** (each player logged into their own account), with an emulator fallback for games where that fails.
- Input: **controllers only** (each instance sees exactly one gamepad). Keyboard/mouse goes to whichever window is focused, as normal.
- Layout: **user-chosen arrangement** per player count: top/bottom, left/right, grid, big+small, multi-monitor, and fully custom.
- Stack: **Rust everywhere** (Rust core, Tauri 2 UI, Rust hook DLL).
- Build strategy: new codebase that borrows proven techniques from Nucleus Co-op / Proto Input (both documented below), not a fork.

### Why not VMs
Four VMs would need four GPUs (or enterprise vGPU licensing), break anti-cheat, and add latency. Every working tool in this space (Nucleus Co-op, Universal Split Screen) instead runs N ordinary processes on one OS and isolates only what collides: input devices, window placement/focus, single-instance mutexes, save/config paths, and the Steam client. That is the design here.

### What "any game" realistically means
The game must have **some way for instances to connect**: LAN, direct IP, Steam/online lobbies, or a mod. Single-player-only games cannot be made co-op. Games with kernel anti-cheat (EAC/BattlEye/Vanguard) will generally refuse injected DLLs and sandboxes; they are out of scope. Each game gets a small declarative **profile** (TOML); a generic default profile covers well-behaved games, and awkward ones get per-game tweaks (Nucleus has ~hundreds of these "handlers" for a reason).

---

## Research findings the design relies on

| Topic | Finding | Consequence |
|---|---|---|
| Multiple Steam clients | Steam is a single-instance client per machine; a second Windows user account does **not** work. **Sandboxie-Plus** is the community-proven way: a second Steam inside a box, logged into another account, runs alongside the host's Steam. Must use a **Standard (yellow) box**: the "App Compartment" box breaks credential storage so Steam asks for password+2FA every launch (sandboxie-plus/Sandboxie#3564). Some games launched in the box fail to reach the boxed Steam (anecdotal), and anti-cheat games refuse to run boxed. | Real-Steam-per-player = Sandboxie backend. Keep Goldberg emulator as a fallback provider. Treat as the highest-risk component; validate in M2 early. |
| Sandboxie automation | `Start.exe /box:NAME [/env:K=V] [/wait] prog.exe`, `Start.exe /box:NAME /terminate`, `/listpids`; `SbieIni.exe set|append|delete BOX Setting Value`; `InjectDll=` / `InjectDll64=` inject a DLL into **every** process in the box (loaded before the app's own imports, optional `InjectDllMain` export); `OpenPipePath=` exposes a host folder read-write even when the exe lives inside the box (`OpenFilePath` does not). Standard boxes are free; privacy/compartment boxes need a supporter cert. | Orchestrator provisions one box per player slot via SbieIni, injects our hook DLL via InjectDll, exchanges per-instance config/state through an `OpenPipePath` folder. |
| Steam Families | Members can play the **same** game simultaneously only if the family owns **one copy per concurrent player**. | 4 real-Steam players = 4 copies (family counts). Document this up front in the UI. |
| Nucleus Co-op | C#/.NET Framework + C++ hook DLLs; JS "handlers"; techniques: symlinked per-instance game folder, mutex kill/rename, per-instance XInput DLL that remaps one slot, fake focus (periodic WM_ACTIVATE etc.), strip borders + SetWindowPos, Goldberg built in, temporary Windows users for save isolation. Repo has no license file; last commit 2021 (successor repo exists). | Borrow techniques, not code. |
| Proto Input | MIT, by Ilyaki, unmaintained since Sept 2021, EasyHook+BlackBone deps. Hook catalogue worth porting: `FocusHooks` + focus message loop, `RenameHandles` (mutex/named pipe rename), `XinputHook` (`SetControllerIndex`), `DinputOrderHook`, `SetWindowPos`, `WindowStyle`, `FindWindow`, `CreateSingleHID`, `ClipCursor`. Startup injection = create suspended, inject, configure, `WakeUpProcess`. | Our Rust hook DLL reimplements this subset (controllers + focus + window + handles). MIT lets us port specific logic. |
| Prism Launcher CLI | `prismlauncher --launch <instanceFolder> --profile <accountName> [--server host:port]`. | Minecraft provider is nearly free: one Prism instance per player, each with its own account. |
| Minecraft Bedrock | No local splitscreen on Windows; UWP single-instance. | Out of scope. Java only. |
| Windows.Gaming.Input / GameInput | WGI only delivers gamepad input to the **foreground** window; GameInput has an opt-in background focus policy. XInput and raw HID work in background. | Fake focus alone isn't enough for WGI games; needs a WGI shim (Phase 3). XInput games are the v1 target. |
| gbe_fork (Goldberg) | LGPL-3.0, active (2026 releases), drop-in `steam_api(64).dll`, LAN lobbies via broadcast, per-instance `steam_settings/` (steam id, account name), optional overlay. | Fallback "steam-emu" provider, Nucleus-style. |

---

## Architecture

```
┌──────────────────────────── lsa-app (Tauri 2, Rust) ────────────────────────────┐
│  UI (Svelte+TS): Library · Players & controllers · Layout editor · Play/Stop     │
└───────────────┬──────────────────────────────────────────────────────────────────┘
                │ Tauri commands
┌───────────────▼──────────────── lsa-core (Rust lib) ─────────────────────────────┐
│ Profiles (TOML)   Session orchestrator   Layout engine   Controller registry     │
│ Providers: exe | steam | prism | steam-emu     Isolation: none | env | sandboxie │
│ Window manager (find/strip/pin windows)  Injector (startup/runtime, x86+x64)     │
│ Instance IPC: per-instance folder with instance.json (config) + state.json       │
└───────────────┬──────────────────────────────────────────────────────────────────┘
                │ spawns / injects                                 (per instance)
┌───────────────▼───────────────┐  ┌──────────────────────────┐  ┌────────────────┐
│ Game process (host)           │  │ Sandboxie box LSA_P2     │  │ Prism → javaw  │
│  + lsa-hook{32,64}.dll        │  │  Steam (acct 2)          │  │  (no hooks)    │
│    XInput remap · fake focus  │  │  Game + lsa-hook via     │  │                │
│    handle rename · window pin │  │  InjectDll               │  │                │
└───────────────────────────────┘  └──────────────────────────┘  └────────────────┘
```

### Repository layout (Cargo workspace, this folder)

```
Cargo.toml                 workspace: resolver 2, shared profile settings
crates/
  lsa-core/                domain + Win32 glue (windows crate). No UI.
    src/profile.rs         TOML schema (serde) + validation + placeholder expansion
    src/layout.rs          viewports, presets, custom layouts, monitor model
    src/controllers.rs     XInput slot + HID enumeration, "press A to assign" detection
    src/providers/{exe,steam,prism,steam_emu}.rs
    src/isolation/{none,env,sandboxie}.rs   (sandboxie.rs wraps Start.exe / SbieIni.exe)
    src/window.rs          find main window of a process tree, strip styles, SetWindowPos, backdrop
    src/inject.rs          CreateProcess(SUSPENDED) + LoadLibrary remote thread; 32-bit via helper
    src/session.rs         start/stop a 2–4 player session, per-instance state machine
    src/ipc.rs             instance.json / state.json read-write + file watching (notify)
  lsa-hook/                cdylib, built for x86_64 AND i686 → lsa-hook64.dll / lsa-hook32.dll
    src/lib.rs             DllMain / InjectDllMain → spawn init thread (never hook inside loader lock)
    src/config.rs          locate instance.json (env LSA_INSTANCE_DIR, else next to the DLL), hot-reload
    src/hooks/xinput.rs    XInputGetState/GetStateEx/GetCapabilities/SetState across xinput1_4/1_3/9_1_0
    src/hooks/focus.rs     GetForegroundWindow/GetActiveWindow/GetFocus + WndProc message filter + focus loop
    src/hooks/handles.rs   CreateMutex/CreateEvent/CreateSemaphore/CreateFileMapping/CreateNamedPipe name rename
    src/hooks/window.rs    CreateWindowExW size override, SetWindowPos pin, GetSystemMetrics/MonitorInfo lies,
                           IDXGISwapChain::SetFullscreenState → no-op
    src/hooks/dinput.rs    (Phase 3) IDirectInput8::EnumDevices filter by instance GUID
    src/hooks/wgi.rs       (Phase 3) Windows.Gaming.Input shim backed by XInput
  lsa-inject32/            tiny i686 exe: inject lsa-hook32.dll into a WOW64 pid (non-sandboxed games only)
  lsa-cli/                 headless `lsa play <profile> --players 2 --layout lr` for testing before UI exists
  lsa-app/                 Tauri 2 shell: commands → lsa-core; global hotkeys (kill-all, spotlight)
ui/                        Svelte + TypeScript frontend (Vite); layout editor lives here
profiles/                  shipped TOML profiles: _default.toml, minecraft-java.toml, terraria.toml, ...
docs/                      profile-format.md, sandboxie-setup.md, troubleshooting.md
```

Key crates: `windows` (Win32/COM/XInput/DXGI), `minhook-sys` (MinHook bindings; `retour` as fallback if MinHook misbehaves on a target), `serde`/`toml`, `notify`, `tracing`, `tauri` 2, `global-hotkey`, `thiserror`/`anyhow`. Hook DLL profile: `panic = "abort"`, `opt-level = "s"`, no unwinding across FFI, static CRT (`-C target-feature=+crt-static`).

---

## Components in detail

### 1. Game profiles (TOML)
Declarative, shareable, one file per game, plus `_default.toml` used for unknown games. Placeholders `${player}`, `${width}`, `${height}`, `${x}`, `${y}`, `${instance_dir}`, `${nickname}` expand per instance.

```toml
[game]
id = "terraria"            # also the profile filename
name = "Terraria"
provider = "steam"         # exe | steam | prism | steam-emu
steam_appid = 105600
executable = "Terraria.exe"   # process to hook/manage (may differ from the launcher)
max_players = 4

[isolation]
mode = "sandboxie"         # none | env | sandboxie   (steam provider forces sandboxie for slots != host)
instance_dir = "none"      # none | symlink | copy    (Nucleus-style per-instance game folder)

[launch]
args = ["-windowed", "-w", "${width}", "-h", "${height}"]
stagger_ms = 4000          # delay between instance starts
window = { title_regex = "^Terraria", wait_ms = 60000 }

[hooks]
xinput = true
fake_focus = true
force_windowed = true
lie_screen_size = true     # GetSystemMetrics / EnumDisplayMonitors report viewport size
pin_window = true
rename_handles = []        # e.g. ["Global\\SomeGameMutex"]

[[patch]]                  # per-instance config edits before launch
file = "%USERPROFILE%/Documents/My Games/Terraria/config.json"
format = "json"
set = { "Fullscreen" = false, "DisplayWidth" = "${width}", "DisplayHeight" = "${height}" }
```

`lsa-core/src/profile.rs` validates, expands placeholders, and merges `_default.toml` ← game profile ← per-session overrides.

### 2. Providers (how an instance is launched)
- **exe**: run a path with args. Isolation `none` or `env` (sets `APPDATA`, `LOCALAPPDATA`, `USERPROFILE`, `TEMP` to `instances/<n>/` so games that honour env vars get separate saves) or `sandboxie`.
- **steam** (real Steam per player): slot 1 may run on the host's un-sandboxed Steam; every other slot runs inside its own Sandboxie box (`LSA_P2`…`LSA_P4`). Launch = `Start.exe /box:LSA_Pn /env:LSA_INSTANCE_DIR=... "<box steam>\steam.exe" -applaunch <appid> <args>`. If the boxed Steam is already running, `-applaunch` hands off via IPC and env vars don't propagate, so the hook DLL falls back to the `instance.json` sitting **next to the DLL copy for that box** (each box's `InjectDll64=` points at `C:\ProgramData\LSA\boxes\LSA_Pn\lsa-hook64.dll`). This avoids needing any Sandboxie API inside the DLL.
- **prism** (Minecraft Java): `prismlauncher.exe --launch <instance> --profile <account> [--server 127.0.0.1:25565]`. The provider writes per-instance `options.txt` (`fullscreen:false`, `pauseOnLostFocus:false`, window size), Prism `instance.cfg` (`OverrideWindow=true`, `MinecraftWinWidth/Height`), and pre-selects the player's controller in the **Controlify** mod config. Optional: provider runs a local dedicated server so all four instances auto-join (avoids the manual "Open to LAN"). No DLL injection needed: Minecraft keeps rendering unfocused and Controlify's SDL backend polls XInput in the background.
- **steam-emu** (fallback, Phase 4): symlink the game folder per instance, drop in gbe_fork `steam_api(64).dll`, write `steam_settings/` with distinct `user_steam_id.txt`/`account_name.txt`, `custom_broadcasts.txt = 127.0.0.1`. Used when a game won't talk to a boxed Steam, or when players lack extra copies. Only ever applied to games the user owns.

### 3. Isolation backend: Sandboxie-Plus
`lsa-core/src/isolation/sandboxie.rs` wraps `SbieIni.exe` and `Start.exe` (located via registry/`Program Files\Sandboxie-Plus`). Per player slot, the one-time **Player setup wizard** creates a Standard box:

```
SbieIni.exe set LSA_P2 Enabled y
SbieIni.exe set LSA_P2 Comment "LocalSplitscreenAnything player 2"
SbieIni.exe append LSA_P2 OpenPipePath "C:\ProgramData\LSA\boxes\LSA_P2\"     # config/state exchange, always applied
SbieIni.exe set LSA_P2 InjectDll64 "C:\ProgramData\LSA\boxes\LSA_P2\lsa-hook64.dll"
SbieIni.exe set LSA_P2 InjectDll   "C:\ProgramData\LSA\boxes\LSA_P2\lsa-hook32.dll"
Start.exe /reload
```
Then it runs `SteamSetup.exe` inside the box, the player logs in once (Standard box persists the session), and adds the host's Steam library folder in the boxed Steam so reads fall through to the host install (Sandboxie's default copy-on-write: reads from host, writes stay in the box; no `OpenFilePath` on the game folders, so saves/config stay per player automatically). The hook DLL is a no-op in every boxed process whose exe name doesn't match `instance.json`, so Steam itself is never hooked.

Session teardown: `Start.exe /box:LSA_Pn /terminate` for the game process only (not the Steam client, which stays warm), via `/listpids` + targeted kill.

### 4. Hook DLL (`lsa-hook`)
Injected into each game instance (via Sandboxie `InjectDll`, or our injector for un-sandboxed games). Reads `instance.json` (controller assignment, viewport rect, enabled hooks, handle rename list), watches it for changes, and installs:

- **XInput remap**: `XInputGetState`, `XInputGetStateEx` (ordinal 100), `XInputGetCapabilities`, `XInputSetState`, `XInputGetBatteryInformation` in `xinput1_4`, `xinput1_3`, `xinput9_1_0`, `xinputuap`. Virtual slot 0 ← assigned physical slot; other virtual slots → `ERROR_DEVICE_NOT_CONNECTED`. Hooks `LoadLibrary(Ex)W/A` so late-loaded xinput modules (SDL, Unity, UE) get hooked too.
- **Fake focus**: hook `GetForegroundWindow`, `GetActiveWindow`, `GetFocus`, `SetFocus`, `SetForegroundWindow`; subclass the main window to swallow `WM_KILLFOCUS`/`WM_ACTIVATE(WA_INACTIVE)`/`WM_ACTIVATEAPP(FALSE)`/`WM_NCACTIVATE(FALSE)` and run a 5 ms focus-message loop like Proto Input's `StartFocusMessageLoop`.
- **Handle rename**: hook `CreateMutexW/A`, `CreateEventW/A`, `CreateSemaphoreW/A`, `CreateFileMappingW/A`, `CreateNamedPipeW`, `OpenMutexW`: append `_LSA<n>` to listed names (or all names in "aggressive" mode) so single-instance checks fail to see siblings.
- **Window**: `CreateWindowExW` override to viewport size, `SetWindowPos`/`MoveWindow` pin, `GetSystemMetrics(SM_CX/CYSCREEN)`, `EnumDisplayMonitors`/`GetMonitorInfoW`, `EnumDisplaySettingsW` lies so the game "detects" the viewport resolution, `IDXGISwapChain::SetFullscreenState(TRUE)` → forced FALSE (vtable hook after first swapchain creation via `D3D11CreateDeviceAndSwapChain`/`IDXGIFactory::CreateSwapChain` hooks), `ChangeDisplaySettingsExW` → no-op, `ClipCursor` → clamp to viewport.
- **Phase 3**: DirectInput8 `EnumDevices` filter + raw HID (`GetRawInputDeviceList`, `SetupDiEnumDeviceInterfaces`, `CreateFileW` on `\\?\hid#`) filter for PS4/PS5 pads; Windows.Gaming.Input `Gamepad` statics shim backed by XInput; GameInput focus policy.

Init rule: `DllMain` only spawns a thread; all hooking happens there (MinHook `MH_Initialize` + queued hooks + `MH_EnableHook(MH_ALL_HOOKS)`). Export `InjectDllMain` for Sandboxie.

### 5. Layout engine (`lsa-core/src/layout.rs` + `ui/src/lib/layout-editor`)
Model: `Layout { name, player_count, viewports: Vec<Viewport { player, monitor_id, rect: NormalizedRect }> }`, rects normalized 0..1 per monitor so a layout survives resolution changes. Monitors enumerated via `EnumDisplayMonitors` with DPI awareness (`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` in the orchestrator).

**Presets** (chosen in a picker that shows little thumbnails, per player count):
- 2 players: Left | Right · Top / Bottom · One per monitor
- 3 players: 2×2 grid with empty cell (choose which) · Big left + two stacked right · Big top + two bottom · Three columns · Three rows
- 4 players: 2×2 grid · Four columns · Four rows · Big + three small (L-shape) · Two per monitor · One per monitor
Preset generator is a pure function `preset(kind, player_count, monitors) -> Layout`, unit-tested.

**Custom editor** (UI): canvas showing the monitor arrangement; drag to move, drag edges to resize; snap to 1/2, 1/3, 1/4 lines and to other viewports; assign player number per viewport; aspect-ratio badge warns when a viewport is far from 16:9/16:10; "fill gaps" and "equalize" buttons; save as named layout (`%APPDATA%\LSA\layouts\*.toml`); per-game default layout stored in the profile override.

**Runtime**: each viewport → window rect for that instance (`window.rs` strips `WS_CAPTION|WS_THICKFRAME|WS_BORDER`, sets `WS_EX_TOPMOST` optional, `SetWindowPos`), plus the hook DLL's screen-size lies so the game renders at exactly that size. Black **backdrop** window per monitor (bottom-most) hides the desktop. Hotkeys: **spotlight** (temporarily maximize one player's instance, press again to restore), **swap** two players' viewports, **kill all**. Live re-layout: editing the layout while a session runs re-pins windows immediately.

### 6. Controllers (`controllers.rs`)
Player assignment screen: "Player N: press A". Poll XInput slots 0–3 (and raw HID devices for non-XInput pads) to detect which physical controller pressed. Store `{ xinput_slot | hid_path }` per player slot; persist by device identity (container id / HID path) so re-plugging keeps assignments. Mapping sent to each instance via `instance.json`. Keyboard/mouse: untouched (focused window gets it).

### 7. Session orchestrator (`session.rs`)
State machine per instance: `Provisioning → Launching → WaitingForWindow → Positioned → Running → Exited`. Steps: resolve profile → build layout → for each player: prepare isolation (box/env/dir), write `instance.json`, launch via provider (staggered), wait for main window (poll process tree + `EnumWindows`, match by pid tree and optional title regex), strip/pin window, mark ready. Watchdog: if an instance dies, offer restart in place. Stop: terminate instances (boxed via `/terminate`-scoped pids), restore any patched config files from backups, remove backdrops.

---

## Milestones

**M0 — Skeleton + Minecraft (no hooks)**
Workspace, `lsa-core` (profile, layout presets, window manager, exe + prism providers, isolation none/env), `lsa-cli`. Validate: `lsa play minecraft-java --players 4 --layout grid` launches four Prism instances with four accounts into a 2×2 grid, each with its own controller via Controlify, all joined to one LAN world/local server. Also validate any DRM-free exe in a left/right layout.

**M1 — Hook DLL v1 + injector**
`lsa-hook` (XInput remap, fake focus, handle rename, window pin/lies, force windowed), `inject.rs`, `lsa-inject32`. Validate with a non-Steam XInput game: 2–4 instances, each responds only to its controller while unfocused, windows stay pinned.

**M2 — Sandboxie + real Steam per player** (highest risk; do right after M1)
`sandboxie.rs`, box provisioning, Player setup wizard (install Steam in box, log in once, add library), steam provider, `InjectDll` path, `OpenPipePath` state exchange. Validate with the chosen Steam co-op game using one copy per player (Steam Families counts). Record exactly which games connect to the boxed Steam; if a game can't, it gets `provider = "steam-emu"` in its profile.

**M3 — Tauri UI + layout editor**
Library (scan Steam `libraryfolders.vdf`/appmanifests, Prism instances, manual exe), Players & controllers screen, layout picker + custom editor, Play/Stop, session status, hotkeys, log viewer. `lsa-cli` stays as the headless path.

**M4 — Fallback provider + broader input**
`steam-emu` (gbe_fork + symlinked instance dirs), DirectInput/HID filtering, WGI shim. Profiles for 5–10 popular co-op games; profile import/export.

**M5 — Polish**
Per-player audio device routing (Windows per-app endpoint API), multi-monitor presets, crash cleanup (`Start.exe /box:X /terminate`), installer (MSI/NSIS) bundling both hook DLLs, docs.

---

## Dev environment (Windows PC)
- Rust stable MSVC toolchain + targets `x86_64-pc-windows-msvc` and `i686-pc-windows-msvc`; VS 2022 Build Tools (C++ workload, Windows SDK) for MinHook and link.
- Node LTS + `cargo tauri` CLI, WebView2 runtime (ships with Win11).
- Sandboxie-Plus (latest), Prism Launcher, Steam. Separate Microsoft accounts per Minecraft player (or offline accounts + `online-mode=false` server).
- Build: `cargo build -p lsa-hook --target i686-pc-windows-msvc` and `--target x86_64-pc-windows-msvc`; a `justfile`/`build.ps1` copies both DLLs into `target/dist/`.
- Edit on the Mac, push via git, build/run on the PC (or VS Code Remote-SSH into the PC).

## Verification (end to end, per milestone)
1. M0: four Prism instances visible in the chosen layout; each instance only moves when its own controller is used; `lsa play ... --layout lr/tb/grid/custom.toml` all place windows correctly on one and two monitors; `cargo test -p lsa-core` covers preset generation and profile merging.
2. M1: launch a test exe twice with the hook; with window 2 unfocused, controller 2 still drives it and controller 1 does nothing; `instance.json` edit (swap controllers) takes effect live; `rename_handles` lets a single-instance game run twice.
3. M2: wizard produces `LSA_P2`; boxed Steam stays logged in across reboots; boxed game process shows `lsa-hook64.dll` loaded (Process Explorer); two instances on two accounts join one lobby; `Stop` kills only the games.
4. M3: UI round-trip: pick game → assign pads → pick/edit layout → Play → windows match the editor preview; hotkeys work.
Manual test checklist lives in `docs/testing.md`.

## Risks and open items (confirm on the PC during M2/M1)
- Boxed-Steam game connectivity varies per game (overlay, Steam networking). Mitigation: steam-emu fallback per profile; try launching the game **from the boxed Steam UI** vs `-applaunch` if one path fails.
- XInput slot ↔ physical device identity is indirect; assignment persists by slot, with "press A" re-detection each session as the safe default.
- WGI/GameInput-only games (some UE5/Unity titles) ignore background input until the Phase-3 shim exists; profiles flag `input_api = "wgi"` so the UI warns.
- Exclusive-fullscreen-only games need the DXGI hook; a few old D3D9 titles need `IDirect3D9::CreateDevice` presentation-parameter patching (add when encountered).
- Steam Families one-copy-per-player rule and the Sandboxie "Standard box only" requirement must be explained in the wizard.
- Games writing config next to the exe collide without `instance_dir = "symlink"`; the default profile uses `symlink` only when a game is marked as doing that.
- Controlify config field name for pre-selecting a controller needs confirming against the mod version installed.
