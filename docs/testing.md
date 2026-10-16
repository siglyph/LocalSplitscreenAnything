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

## M1: hook DLL (fill in once implemented)

- [ ] Two instances of an XInput game: with window 2 unfocused, pad 2 drives it and pad 1 does nothing.
- [ ] Editing `instance.json` (swap controller slots) takes effect without restarting.
- [ ] `rename_handles` lets a single-instance game start twice.
- [ ] A game that only offers exclusive fullscreen still ends up windowed at the viewport size.

## M2: Sandboxie + real Steam

- [ ] Box `LSA_P2` created with the documented settings; boxed Steam stays logged in after a reboot.
- [ ] Boxed game process shows `lsa_hook64.dll` loaded (Process Explorer) and writes `state.json`.
- [ ] Two accounts, two copies: both instances join one lobby/server.
- [ ] Ctrl+C kills only the games, not the boxed Steam.
