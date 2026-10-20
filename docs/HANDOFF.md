# Handoff: continuing LocalSplitscreenAnything on the gaming PC

Written 2026-10-19 by the Claude session that scaffolded the project on Jude's Mac. From here
on, development happens on the gaming PC (this clone: `C:\Users\Jude\Documents\LocalSplitscreenAnything`).
Read this, then `docs/plan.md` (the approved architecture plan) and `README.md`.

## State of the code

- Milestone M0 is scaffolded and pushed: `lsa-proto`, `lsa-core`, `lsa-cli`, hook DLL
  scaffold, profiles, docs, `build.ps1`. 44 unit tests pass; clippy is clean on macOS and on
  the Windows x64 target; the hook crate type-checks for i686. See `README.md` for layout.
- Nothing has been exercised with a real game yet. A PC session ran `build.ps1` and
  `lsa controllers list` successfully (build output was not relayed back in full).
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
- Windows reported ONE display at 1280x720 during a remote session; that is probably the
  remote/headless adapter, not the physical monitor. Check `lsa monitors` at the real
  resolution before judging window placement.

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

1. In a Developer PowerShell: `.\build.ps1`, `cargo test`, `cargo clippy --workspace --all-targets`.
   Fix anything Windows-specific that the Mac could not catch (the windows crate 0.61 API was
   only type-checked, never run).
2. `dist\lsa.exe monitors`, `layouts preview grid -p 4`, `profiles list`,
   `play minecraft-java -p 2 --dry-run`. Work through `docs/testing.md` (M0 section).
3. Create two Prism instances named `Player1` and `Player2` (Fabric + Controlify), add the
   accounts, then `dist\lsa.exe play minecraft-java -p 2 --layout left-right`. Expect to tune
   `launch.window.title_regex`, `wait_ms` and the options.txt patches in `profiles/minecraft-java.toml`.
4. Start M1 (hook DLL + injector) per `docs/plan.md`, then the input router above.
