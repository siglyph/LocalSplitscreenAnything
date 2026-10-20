# Manual test checklist

Run on the Windows gaming PC after `.\build.ps1`. Tick items as they pass; note the game and
version next to failures.

## M0: layout and launching (no hooks)

- [ ] `cargo test` passes; `cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc` is clean.
- [ ] `lsa monitors` lists every display with the right size, position and scale (check a
      150% DPI display reports physical pixels).
- [ ] `lsa layouts preview grid -p 4` draws the expected arrangement for the real monitors;
      `two-per-monitor -p 4` on two displays puts players 3 and 4 on the second one.
- [ ] `lsa controllers list` shows connected pads; `lsa controllers assign -p 2` detects the
      right pad for each player and ignores a pad that is already assigned.
- [ ] `lsa play example-exe --dry-run` (with a real DRM-free game path) prints sensible commands.
- [ ] Minecraft: two Prism instances with two accounts, `lsa play minecraft-java -p 2 --layout left-right`;
      both windows appear borderless side by side; each keeps rendering when the other is
      focused; player 1 opens to LAN and player 2 joins; each Controlify picks its own pad.
- [ ] Same with 4 players in `grid`; `--backdrop` hides the desktop; Ctrl+C closes all four and
      restores `options.txt` / `instance.cfg`.
- [ ] A game that moves its own window at start gets re-pinned by the session loop.
- [ ] Kill `lsa play` from Task Manager mid-session, then `lsa stop` terminates the instances and
      restores patched files.

## M1: hook DLL + injector

The hook DLL (`lsa_hook{64,32}.dll`) and injector (`lsa-core::inject` + `lsa-inject32.exe`)
are built by `.\build.ps1`. `cargo build --profile hook -p lsa-hook` must succeed for both
`x86_64-pc-windows-msvc` and `i686-pc-windows-msvc` (MinHook is compiled from C, so run from a
Developer PowerShell or after `vcvars64.bat`). `cargo test` and
`cargo clippy --workspace --all-targets` cover the pure logic (slot mapping, name rewrite,
rect math, command-line quoting).

### The XInput probe (no real game needed)

`tools/xinput-probe` is a stand-in game: a window that shows which XInput slots it sees, the
buttons held on each, whether it believes it is focused, and the screen size it detects. It is
its own workspace. Build both bitnesses, then make a profile pointing at it:

```powershell
cargo build --manifest-path tools\xinput-probe\Cargo.toml --release --target x86_64-pc-windows-msvc
# optional 32-bit build to test lsa_hook32.dll + lsa-inject32.exe:
cargo build --manifest-path tools\xinput-probe\Cargo.toml --release --target i686-pc-windows-msvc
```

Copy `profiles/example-exe.toml` to `profiles/xinput-probe.toml`, set `game.path` to the built
`xinput-probe.exe`, `provider = "exe"`, `isolation.mode = "none"`, leave `[hooks] inject = true`.
Then (needs two XInput pads; with Steam Controllers/DualSense, first set each pad's Steam
Desktop Layout to a Gamepad template, per `docs/HANDOFF.md`):

```powershell
dist\lsa.exe controllers list                 # confirm two virtual Xbox pads appear
dist\lsa.exe play xinput-probe -p 2 --layout left-right -c 1=0 -c 2=1
```

Checklist:

- [ ] Both probe windows appear borderless, side by side, each at its viewport size; each
      reports that detected screen size, not the full desktop (screen-size lie + window pin).
- [ ] Each window shows a pad on **slot 0 only** (slots 1-3 read `-`); physically moving pad 2
      changes only player 2's window, pad 1 only player 1's, even while the other is focused.
- [ ] Both windows report `foreground ... yes` at the same time (fake focus).
- [ ] Editing the running session's `instance.json` to swap `controller.slot` (or
      `dist\lsa.exe` re-run with swapped `-c`) makes each window follow the other pad within
      ~half a second, without a restart (hot reload).
- [ ] `state.json` next to each `instance.json` lists the installed hooks and the window
      handle, with no entries under `errors`.
- [ ] A game using a listed single-instance mutex starts twice with that name in
      `[hooks] rename_handles` (verify the sibling's mutex name with Process Explorer).
- [ ] A game that only offers exclusive fullscreen still ends up windowed at the viewport size
      (`force_windowed`: DXGI `SetFullscreenState`/D3D9 `Windowed`).

## M2: Sandboxie + real Steam

- [ ] Box `LSA_P2` created with the documented settings; boxed Steam stays logged in after a reboot.
- [ ] Boxed game process shows `lsa_hook64.dll` loaded (Process Explorer) and writes `state.json`.
- [ ] Two accounts, two copies: both instances join one lobby/server.
- [ ] Ctrl+C kills only the games, not the boxed Steam.
