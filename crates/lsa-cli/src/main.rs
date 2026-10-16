//! `lsa`: headless command line for LocalSplitscreenAnything.
//!
//! Everything the Tauri UI will do can be driven from here, which is how milestones M0–M2
//! are tested on the Windows PC. Layout previews and dry runs also work on macOS/Linux.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use lsa_core::controllers;
use lsa_core::layout::{self, ascii_preview, Corner, Layout, Monitor, PresetKind, PresetOptions};
use lsa_core::paths;
use lsa_core::profile::{GameProfile, ProfileStore};
use lsa_core::proto::{ControllerAssignment, PixelRect};
use lsa_core::session::{self, PlayerSlot, Session, SessionEvent, SessionOptions};
use lsa_core::window;

#[derive(Parser)]
#[command(
    name = "lsa",
    version,
    about = "Run 2-4 instances of a game as local splitscreen"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List, show or validate game profiles.
    Profiles {
        #[command(subcommand)]
        cmd: ProfilesCmd,
    },
    /// List, preview or save layouts.
    Layouts {
        #[command(subcommand)]
        cmd: LayoutsCmd,
    },
    /// Show connected monitors.
    Monitors,
    /// List controllers or assign them to players by pressing A.
    Controllers {
        #[command(subcommand)]
        cmd: ControllersCmd,
    },
    /// Launch a splitscreen session.
    Play(PlayArgs),
    /// Clean up after a session whose `lsa play` is gone (kill instances, restore files).
    Stop {
        /// Session manifest (`session.json`); defaults to the newest unfinished session.
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ProfilesCmd {
    /// List every profile found in the profile directories.
    List,
    /// Print the merged profile for an id or path.
    Show { id: String },
    /// Parse and validate a profile file.
    Validate { path: PathBuf },
    /// Print the directories profiles are loaded from.
    Dirs,
}

#[derive(Subcommand)]
enum LayoutsCmd {
    /// List presets (optionally for a player/monitor count).
    List {
        #[arg(short, long)]
        players: Option<u8>,
        #[arg(short, long)]
        monitors: Option<usize>,
    },
    /// Draw a preset or a saved layout file.
    Preview {
        /// Preset id (grid, left-right, big-left, ...) or path to a layout .toml.
        layout: String,
        #[arg(short, long, default_value_t = 2)]
        players: u8,
        /// Monitor list such as `1920x1080,2560x1440+1920+0` (default: detected monitors).
        #[arg(short, long)]
        monitors: Option<String>,
        /// For `grid` with 3 players: which cell stays empty (tl, tr, bl, br).
        #[arg(long)]
        empty_cell: Option<String>,
    },
    /// Write a preset out as a layout .toml you can edit by hand.
    Save {
        layout: String,
        #[arg(short, long, default_value_t = 2)]
        players: u8,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(short, long)]
        monitors: Option<String>,
        #[arg(long)]
        empty_cell: Option<String>,
    },
}

#[derive(Subcommand)]
enum ControllersCmd {
    /// Show XInput slots and whether a controller is connected.
    List,
    /// Ask each player in turn to press A and print the resulting `--controller` flags.
    Assign {
        #[arg(short, long, default_value_t = 2)]
        players: u8,
    },
}

#[derive(Args)]
struct PlayArgs {
    /// Profile id (see `lsa profiles list`) or path to a profile .toml.
    profile: String,
    #[arg(short, long, default_value_t = 2)]
    players: u8,
    /// Preset id or layout .toml (default: the profile's default_layout, else left-right/grid).
    #[arg(short, long)]
    layout: Option<String>,
    /// Controller per player, e.g. `-c 1=0 -c 2=1` (XInput slot), `-c 3=none`.
    #[arg(short = 'c', long = "controller")]
    controllers: Vec<String>,
    /// Ask each player to press A instead of using `--controller`.
    #[arg(long)]
    assign: bool,
    /// Nickname per player, e.g. `--nickname 1=Jude`.
    #[arg(long = "nickname")]
    nicknames: Vec<String>,
    /// Plan and print command lines without patching or launching anything.
    #[arg(long)]
    dry_run: bool,
    /// Cover the desktop with black windows behind the games.
    #[arg(long)]
    backdrop: bool,
    /// Keep game windows above everything else.
    #[arg(long)]
    topmost: bool,
    /// Monitor list for dry runs on machines without Windows (see `layouts preview`).
    #[arg(short, long)]
    monitors: Option<String>,
    #[arg(long)]
    empty_cell: Option<String>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Profiles { cmd } => profiles(cmd),
        Cmd::Layouts { cmd } => layouts(cmd),
        Cmd::Monitors => {
            let monitors = window::enumerate_monitors();
            if monitors.is_empty() {
                bail!("no monitors detected (monitor detection needs Windows)");
            }
            print_monitors(&monitors);
            Ok(())
        }
        Cmd::Controllers { cmd } => controllers_cmd(cmd),
        Cmd::Play(args) => play(args),
        Cmd::Stop { manifest } => stop(manifest),
    }
}

fn store() -> ProfileStore {
    ProfileStore::new(paths::profile_dirs())
}

fn profiles(cmd: ProfilesCmd) -> Result<()> {
    match cmd {
        ProfilesCmd::List => {
            let list = store().list()?;
            if list.is_empty() {
                println!("no profiles found in:");
                for d in paths::profile_dirs() {
                    println!("  {}", d.display());
                }
                return Ok(());
            }
            println!(
                "{:<18} {:<28} {:<10} {:<7} path",
                "id", "name", "provider", "players"
            );
            for (path, p) in list {
                println!(
                    "{:<18} {:<28} {:<10} {:<7} {}",
                    p.game.id,
                    p.game.name,
                    p.game.provider.id(),
                    p.game.max_players,
                    path.display()
                );
            }
        }
        ProfilesCmd::Show { id } => {
            let p = store().load(&id)?;
            print!("{}", p.to_toml_string()?);
        }
        ProfilesCmd::Validate { path } => {
            let p = store().load_path(&path)?;
            println!(
                "ok: {} ({}, provider {})",
                p.game.id,
                p.game.name,
                p.game.provider.id()
            );
        }
        ProfilesCmd::Dirs => {
            for d in paths::profile_dirs() {
                println!(
                    "{}{}",
                    d.display(),
                    if d.is_dir() { "" } else { "  (missing)" }
                );
            }
        }
    }
    Ok(())
}

/// Parse `1920x1080,2560x1440+1920+0`.
fn parse_monitors(spec: &str) -> Result<Vec<Monitor>> {
    let mut out = Vec::new();
    for (i, part) in spec.split(',').enumerate() {
        let part = part.trim();
        let (size, offset) = match part.split_once('+') {
            Some((s, rest)) => (s, Some(rest)),
            None => (part, None),
        };
        let (w, h) = size
            .split_once('x')
            .ok_or_else(|| anyhow!("monitor '{part}' must look like 1920x1080[+X+Y]"))?;
        let width: i32 = w.parse().context("monitor width")?;
        let height: i32 = h.parse().context("monitor height")?;
        let (x, y) = match offset {
            Some(o) => {
                let (x, y) = o
                    .split_once('+')
                    .ok_or_else(|| anyhow!("monitor offset in '{part}' must be +X+Y"))?;
                (
                    x.parse().context("monitor x")?,
                    y.parse().context("monitor y")?,
                )
            }
            None => (
                out.iter()
                    .map(|m: &Monitor| m.rect.right())
                    .max()
                    .unwrap_or(0),
                0,
            ),
        };
        out.push(Monitor {
            id: format!("M{}", i + 1),
            name: format!("Monitor {}", i + 1),
            rect: PixelRect::new(x, y, width, height),
            primary: i == 0,
            scale: 1.0,
        });
    }
    Ok(out)
}

fn monitors_from(spec: Option<&str>) -> Result<Vec<Monitor>> {
    if let Some(s) = spec {
        return parse_monitors(s);
    }
    let detected = window::enumerate_monitors();
    if detected.is_empty() {
        bail!("no monitors detected; pass --monitors 1920x1080 (detection needs Windows)");
    }
    Ok(detected)
}

fn print_monitors(monitors: &[Monitor]) {
    for (i, m) in monitors.iter().enumerate() {
        println!(
            "#{i} {:<12} {}x{} at ({}, {}) scale {:.2}{}",
            m.name,
            m.rect.width,
            m.rect.height,
            m.rect.x,
            m.rect.y,
            m.scale,
            if m.primary { "  primary" } else { "" }
        );
    }
}

fn parse_corner(s: Option<&str>) -> Result<Corner> {
    Ok(match s.map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("br") | Some("bottom-right") => Corner::BottomRight,
        Some("bl") | Some("bottom-left") => Corner::BottomLeft,
        Some("tr") | Some("top-right") => Corner::TopRight,
        Some("tl") | Some("top-left") => Corner::TopLeft,
        Some(other) => bail!("unknown empty cell '{other}' (tl, tr, bl, br)"),
    })
}

/// Resolve `name` to a layout: a .toml path, a saved layout, or a preset.
fn resolve_layout(
    name: &str,
    players: u8,
    monitors: &[Monitor],
    empty_cell: Option<&str>,
) -> Result<Layout> {
    let path = PathBuf::from(name);
    if path.extension().is_some_and(|e| e == "toml") && path.is_file() {
        let text = std::fs::read_to_string(&path)?;
        let layout =
            Layout::from_toml_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        layout.validate(monitors.len())?;
        return Ok(layout);
    }
    let saved = paths::layouts_dir().join(format!("{name}.toml"));
    if saved.is_file() {
        let text = std::fs::read_to_string(&saved)?;
        let layout = Layout::from_toml_str(&text)?;
        layout.validate(monitors.len())?;
        return Ok(layout);
    }
    let kind = PresetKind::parse(name)?;
    let options = PresetOptions {
        grid_empty_cell: parse_corner(empty_cell)?,
    };
    Ok(layout::preset(kind, players, monitors, &options)?)
}

fn default_layout_name(players: u8, profile: Option<&GameProfile>) -> String {
    if let Some(name) = profile.and_then(|p| p.default_layout.clone()) {
        return name;
    }
    match players {
        1 => "columns".to_string(),
        2 => "left-right".to_string(),
        _ => "grid".to_string(),
    }
}

fn layouts(cmd: LayoutsCmd) -> Result<()> {
    match cmd {
        LayoutsCmd::List { players, monitors } => {
            let monitors = monitors.unwrap_or_else(|| window::enumerate_monitors().len().max(1));
            println!("{:<26} description", "preset");
            for kind in PresetKind::ALL {
                let ok = match players {
                    Some(p) => kind.supports(p, monitors),
                    None => true,
                };
                if ok {
                    println!("{:<26} {}", kind.id(), kind.description());
                }
            }
            let dir = paths::layouts_dir();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                let saved: Vec<String> = entries
                    .flatten()
                    .filter_map(|e| {
                        e.path()
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                    })
                    .collect();
                if !saved.is_empty() {
                    println!("\nsaved layouts in {}:", dir.display());
                    for s in saved {
                        println!("  {s}");
                    }
                }
            }
        }
        LayoutsCmd::Preview {
            layout,
            players,
            monitors,
            empty_cell,
        } => {
            let monitors = monitors_from(monitors.as_deref())?;
            let l = resolve_layout(&layout, players, &monitors, empty_cell.as_deref())?;
            print!("{}", ascii_preview(&l, &monitors));
            for rv in l.resolve(&monitors)? {
                println!(
                    "player {}: {}x{} at ({}, {}) on monitor #{}",
                    rv.player, rv.rect.width, rv.rect.height, rv.rect.x, rv.rect.y, rv.monitor
                );
            }
            for (player, aspect) in l.odd_aspect_viewports(&monitors) {
                println!("note: player {player}'s viewport has an unusual aspect ratio ({aspect:.2}); the game may letterbox");
            }
        }
        LayoutsCmd::Save {
            layout,
            players,
            out,
            monitors,
            empty_cell,
        } => {
            let monitors = monitors_from(monitors.as_deref())?;
            let l = resolve_layout(&layout, players, &monitors, empty_cell.as_deref())?;
            std::fs::write(&out, l.to_toml_string()?)?;
            println!("wrote {}", out.display());
        }
    }
    Ok(())
}

fn controllers_cmd(cmd: ControllersCmd) -> Result<()> {
    match cmd {
        ControllersCmd::List => {
            for pad in controllers::xinput_slots() {
                println!(
                    "XInput slot {}: {}",
                    pad.slot,
                    if pad.connected { "connected" } else { "-" }
                );
            }
            if !cfg!(windows) {
                println!("(controller detection needs Windows)");
            }
        }
        ControllersCmd::Assign { players } => {
            let assigned = assign_interactively(players)?;
            let flags: Vec<String> = assigned
                .iter()
                .map(|(p, a)| match a {
                    ControllerAssignment::XInputSlot { slot } => format!("-c {p}={slot}"),
                    ControllerAssignment::HidPath { path } => format!("-c {p}=hid:{path}"),
                    ControllerAssignment::None => format!("-c {p}=none"),
                })
                .collect();
            println!("\nuse: lsa play <profile> -p {players} {}", flags.join(" "));
        }
    }
    Ok(())
}

fn assign_interactively(players: u8) -> Result<Vec<(u8, ControllerAssignment)>> {
    let mut assigned: Vec<(u8, ControllerAssignment)> = Vec::new();
    let mut taken: Vec<u32> = Vec::new();
    for p in 1..=players {
        println!("Player {p}: press A on your controller...");
        match controllers::wait_for_a_button(&taken, Duration::from_secs(60)) {
            Some(slot) => {
                println!("  player {p} -> XInput slot {slot}");
                taken.push(slot);
                assigned.push((p, ControllerAssignment::XInputSlot { slot }));
            }
            None => bail!("no controller pressed A for player {p} within 60 s"),
        }
    }
    Ok(assigned)
}

fn parse_nicknames(specs: &[String]) -> Result<BTreeMap<u8, String>> {
    let mut out = BTreeMap::new();
    for s in specs {
        let (p, name) = s
            .split_once('=')
            .ok_or_else(|| anyhow!("--nickname expects PLAYER=NAME, got '{s}'"))?;
        let p: u8 = p
            .trim()
            .parse()
            .map_err(|_| anyhow!("bad player in '{s}'"))?;
        out.insert(p, name.trim().to_string());
    }
    Ok(out)
}

fn describe(e: &SessionEvent) -> String {
    match e {
        SessionEvent::Planned {
            player,
            dir,
            viewport,
        } => format!(
            "player {player}: {}x{} at ({}, {}), instance dir {}",
            viewport.width,
            viewport.height,
            viewport.x,
            viewport.y,
            dir.display()
        ),
        SessionEvent::Patched { path } => format!("patched {}", path.display()),
        SessionEvent::Launching {
            player,
            command_line,
        } => format!("player {player}: {command_line}"),
        SessionEvent::Note { player, text } => format!("player {player}: note: {text}"),
        SessionEvent::WindowFound {
            player,
            hwnd,
            title,
        } => format!("player {player}: window {hwnd:#x} \"{title}\""),
        SessionEvent::Positioned { player, rect } => format!(
            "player {player}: placed at ({}, {}) {}x{}",
            rect.x, rect.y, rect.width, rect.height
        ),
        SessionEvent::Failed { player, reason } => format!("player {player}: FAILED: {reason}"),
    }
}

fn play(args: PlayArgs) -> Result<()> {
    let profile = store().load(&args.profile)?;
    let monitors = monitors_from(args.monitors.as_deref())?;
    let layout_name = args
        .layout
        .clone()
        .unwrap_or_else(|| default_layout_name(args.players, Some(&profile)));
    let layout = resolve_layout(
        &layout_name,
        args.players,
        &monitors,
        args.empty_cell.as_deref(),
    )?;

    let mut controllers_by_player: BTreeMap<u8, ControllerAssignment> = BTreeMap::new();
    if args.assign {
        for (p, a) in assign_interactively(args.players)? {
            controllers_by_player.insert(p, a);
        }
    }
    for spec in &args.controllers {
        let (p, a) = controllers::parse_assignment(spec).map_err(|e| anyhow!(e))?;
        controllers_by_player.insert(p, a);
    }
    let nicknames = parse_nicknames(&args.nicknames)?;
    let players: Vec<PlayerSlot> = (1..=args.players)
        .map(|p| {
            let controller = controllers_by_player.get(&p).cloned().unwrap_or(
                ControllerAssignment::XInputSlot {
                    slot: (p - 1) as u32,
                },
            );
            let mut slot = PlayerSlot::new(p, controller);
            if let Some(n) = nicknames.get(&p) {
                slot.nickname = n.clone();
            }
            slot
        })
        .collect();

    // Dry runs plan into a scratch folder that is removed afterwards.
    let dry_run_root = args
        .dry_run
        .then(|| std::env::temp_dir().join(format!("lsa-dry-run-{}", std::process::id())));
    let options = SessionOptions {
        dry_run: args.dry_run,
        backdrop: args.backdrop,
        topmost: args.topmost,
        session_root: dry_run_root.clone(),
    };
    let mut session = Session::plan(profile, players, layout, monitors, options)?;
    println!(
        "session {} for {} ({} players, layout {})",
        session.id,
        session.profile.game.name,
        session.instances.len(),
        session.layout.name
    );
    print!("{}", ascii_preview(&session.layout, &session.monitors));
    for inst in &session.instances {
        let controller = match &inst.controller {
            ControllerAssignment::XInputSlot { slot } => format!("XInput slot {slot}"),
            ControllerAssignment::HidPath { path } => format!("HID {path}"),
            ControllerAssignment::None => "no controller".to_string(),
        };
        println!(
            "player {} ({}): {}x{} at ({}, {}), {}{}",
            inst.player,
            inst.nickname,
            inst.viewport.width,
            inst.viewport.height,
            inst.viewport.x,
            inst.viewport.y,
            controller,
            inst.box_name
                .as_ref()
                .map(|b| format!(", box {b}"))
                .unwrap_or_default()
        );
    }
    if args.dry_run {
        println!("\ndry run: patches that would be applied:");
        for (player, p) in session.planned_patches()? {
            println!(
                "  player {player}: {} ({:?})",
                p.path.display(),
                p.spec.format
            );
        }
        println!();
    }

    let mut log = |e: SessionEvent| println!("{}", describe(&e));
    session.apply_patches(&mut log)?;
    session.launch_all(&mut log)?;
    if args.dry_run {
        if let Some(root) = dry_run_root {
            let _ = std::fs::remove_dir_all(root);
        }
        println!("\ndry run complete; nothing was launched");
        return Ok(());
    }

    let stop_flag = Arc::new(AtomicBool::new(false));
    {
        let flag = stop_flag.clone();
        ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
            .context("installing Ctrl+C handler")?;
    }
    println!("\nrunning; press Ctrl+C to stop every instance and restore config files");
    let mut last_phases: Vec<String> = session
        .instances
        .iter()
        .map(|i| i.phase.to_string())
        .collect();
    while !stop_flag.load(Ordering::SeqCst) && !session.all_finished() {
        for e in session.tick() {
            println!("{}", describe(&e));
        }
        for (i, inst) in session.instances.iter().enumerate() {
            let phase = inst.phase.to_string();
            if phase != last_phases[i] {
                println!("player {}: {phase}", inst.player);
                last_phases[i] = phase;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    println!("stopping...");
    for w in session.stop() {
        println!("warning: {w}");
    }
    println!("done");
    Ok(())
}

fn stop(manifest: Option<PathBuf>) -> Result<()> {
    let path = match manifest {
        Some(p) => p,
        None => session::find_unfinished_session(&paths::sessions_dir()).ok_or_else(|| {
            anyhow!(
                "no unfinished session found under {}",
                paths::sessions_dir().display()
            )
        })?,
    };
    println!("cleaning up {}", path.display());
    for w in session::cleanup_manifest(&path)? {
        println!("warning: {w}");
    }
    println!("done");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_monitor_specs() {
        let m = parse_monitors("1920x1080,2560x1440+1920+0").unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[1].rect, PixelRect::new(1920, 0, 2560, 1440));
        let auto = parse_monitors("1920x1080,1920x1080").unwrap();
        assert_eq!(
            auto[1].rect.x, 1920,
            "second monitor is placed to the right by default"
        );
        assert!(parse_monitors("wide").is_err());
    }

    #[test]
    fn default_layouts_by_player_count() {
        assert_eq!(default_layout_name(2, None), "left-right");
        assert_eq!(default_layout_name(3, None), "grid");
        assert_eq!(default_layout_name(4, None), "grid");
    }

    #[test]
    fn nicknames_parse() {
        let n = parse_nicknames(&["1=Jude".into(), "2=Sam".into()]).unwrap();
        assert_eq!(n[&1], "Jude");
        assert!(parse_nicknames(&["Jude".into()]).is_err());
    }
}
