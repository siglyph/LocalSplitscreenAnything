# Handoff: continuing LocalSplitscreenAnything on the gaming PC

Written 2026-10-19 by the Claude session that scaffolded the project on Jude's Mac. From here
on, development happens on the gaming PC (this clone: `C:\Users\Jude\Documents\LocalSplitscreenAnything`).
Read this, then `docs/plan.md` (the approved architecture plan) and `README.md`.

## State of the code

- Milestone M0 is scaffolded and pushed: `lsa-proto`, `lsa-core`, `lsa-cli`, hook DLL
  scaffold, profiles, docs, `build.ps1`. 44 unit tests pass; clippy is clean on macOS and on
  the Windows x64 target; the hook crate type-checks for i686. See `README.md` for layout.
- Nothing has been exercised with a real game yet. Verified on the PC on 2026-10-19 at
  8aa6626: `build.ps1`, `cargo test` (44 passed) and `cargo clippy --workspace --all-targets`
  all exit 0 with no warnings; `monitors`, `layouts preview grid -p 4`, `layouts preview
  big-left -p 3`, `controllers list`, `profiles list` and `play minecraft-java -p 2 --dry-run`
  all exit 0. VS 2026 Build Tools (v18) work although `build.ps1` says 2022.
- The M1 task (real hooks in `crates/lsa-hook` + an injector in `lsa-core`) has a detailed
  brief in the "Hook DLL" section of `docs/plan.md`; nothing of it is implemented. The
  `hooks/mod.rs` stubs report "not implemented yet (milestone M1)".

## Decisions already made (do not re-litigate without Jude)

- Windows only for playing; Rust everywhere (Tauri 2 UI in M3); controllers only (no
  multi-keyboard/mouse); real Steam per player via Sandboxie-Plus Standard boxes (M2), with a
  Goldberg/gbe_fork fallback provider (M4); Minecraft Java via Prism Launcher; user-chosen
  layouts (12 presets + custom TOML, drag editor in the UI later).
- Repo: https://github.com/siglyph/LocalSplitscreenAnything (public, MIT). Commits end with
  `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

## Known facts about this PC (from the PC session's read-only inventory)

- Windows 11 Pro, git 2.47, Rust 1.99.1, VS Build Tools 2026 (`cl.exe` not on PATH: use a
  Developer PowerShell or vcvars64). Steam installed. Prism Launcher 9.1 at
  `%LOCALAPPDATA%\Programs\PrismLauncher`. Sandboxie-Plus NOT installed (needed for M2 only).
  ViGEmBus is installed. RustDesk and Sunshine are installed.
- The clone was owned by BUILTIN\Administrators; git may need
  `-c safe.directory=C:/Users/Jude/Documents/LocalSplitscreenAnything` or a one-time
  `git config --global --add safe.directory ...`.
- One physical display: `lsa monitors` prints `#0 DISPLAY1 3840x2160 at (0, 0) scale 3.00
  primary`. The 1280x720 seen earlier was the logical size at 300% Windows scaling from a
  DPI-unaware query; lsa opts into per-monitor DPI awareness and reports physical pixels.
  Adapters: AMD Radeon iGPU and NVIDIA GeForce RTX 5090 (both 3840x2160) plus a "Meta Virtual
  Monitor" adapter that is not enumerated as a display.
- Prism instances `Player1` and `Player2` exist (created 2026-10-19): copies of Jude's
  "Fabulously Optimized" 6.5.0-beta.3 instance (Minecraft 1.21.4, Fabric 0.16.10, Controlify
  2.0.1, e4mc), `OverrideWindow=true`, `fullscreen:false`, `pauseOnLostFocus:false`, grouped
  as "LSA". Prism has one Microsoft account (`koburas`); an offline account for player 2 is
  being added, then `[prism].accounts` in `profiles/minecraft-java.toml` needs both names and
  the LAN world must have Online Mode off.

## Controller finding (changes the near-term plan)

With two Steam Controllers (2026, via the Valve puck receiver) and two DualSense pads over
Bluetooth, Windows exposes NO XInput device: the Steam Controllers appear as keyboard + mouse +
vendor HID, and Steam claims the DualSenses. Steam Input only synthesises a virtual Xbox pad
for games it launches, or system-wide when the controller's Desktop Layout is a gamepad
template. So `lsa controllers assign` (XInput-based) and the planned M1 XInput remap hook see
nothing by default.

Plan agreed with Jude:
1. Short term: set Steam > Settings > Controller > Desktop Layout to a Gamepad template for
   each pad; Steam then emits a virtual Xbox 360 pad per controller system-wide and
   `lsa controllers list` should show the slots. Enough for the first Minecraft test.
2. Then: give lsa its own input router (promote this from M4): read physical pads over HID
   (SDL3 covers DualSense/DualShock/Switch Pro/older Steam Controller) and emit one virtual
   Xbox pad per player through ViGEmBus; `controllers assign` listens on SDL devices. The
   2026 Steam Controller's protocol may be Steam-only; if so those pads still rely on step 1.
   The M1 hook then filters the virtual pads per instance exactly as planned.

## Immediate next steps on the PC

1. Done 2026-10-19: build, tests, clippy and the dry-run CLI checks (see "State of the code").
2. Done: Prism instances `Player1` and `Player2`. Remaining: the player-2 offline account and
   `[prism].accounts`; Steam Desktop Layout set to a Gamepad template so `controllers list`
   shows the pads; then the first real run, `dist\lsa.exe play minecraft-java -p 2 --layout
   left-right`. Expect to tune `launch.window.title_regex`, `wait_ms` and the options.txt
   patches in `profiles/minecraft-java.toml`. Work through `docs/testing.md` (M0 section).
3. Start M1 (hook DLL + injector) per `docs/plan.md`, then the input router above.
