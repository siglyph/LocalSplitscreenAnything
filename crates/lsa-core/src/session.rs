//! A session: one game, 2–4 players, one layout. Plans instance directories and `instance.json`
//! files, applies config patches with backups, launches each instance through its provider,
//! finds and places each main window, keeps windows pinned, and cleans up on stop.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use lsa_proto::{ControllerAssignment, InstanceConfig, PixelRect, INSTANCE_DIR_ENV, PROTO_VERSION};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::ipc;
use crate::isolation;
use crate::layout::{Layout, Monitor};
use crate::patch::{self, BackupSet};
use crate::paths;
use crate::profile::{GameProfile, Placeholders};
use crate::providers::{self, LaunchRequest, PreparedPatch, Provider};
use crate::window::{self, Backdrop, WindowQuery};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSlot {
    pub number: u8,
    pub nickname: String,
    pub controller: ControllerAssignment,
}

impl PlayerSlot {
    pub fn new(number: u8, controller: ControllerAssignment) -> Self {
        Self {
            number,
            nickname: format!("Player {number}"),
            controller,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    /// Plan everything and print command lines, but patch and launch nothing.
    pub dry_run: bool,
    /// Cover the desktop with black windows behind the games.
    pub backdrop: bool,
    /// Keep game windows above everything else.
    pub topmost: bool,
    /// Where session folders go (defaults to the user data directory).
    pub session_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Planned,
    Launching,
    WaitingForWindow,
    Positioned,
    Running,
    Exited(Option<i32>),
    Failed(String),
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Phase::Planned => write!(f, "planned"),
            Phase::Launching => write!(f, "launching"),
            Phase::WaitingForWindow => write!(f, "waiting for window"),
            Phase::Positioned => write!(f, "positioned"),
            Phase::Running => write!(f, "running"),
            Phase::Exited(Some(code)) => write!(f, "exited ({code})"),
            Phase::Exited(None) => write!(f, "exited"),
            Phase::Failed(why) => write!(f, "failed: {why}"),
        }
    }
}

/// One player's game instance.
pub struct Instance {
    pub player: u8,
    pub nickname: String,
    pub controller: ControllerAssignment,
    pub viewport: PixelRect,
    pub monitor: usize,
    /// Directory holding `instance.json`.
    pub dir: PathBuf,
    pub box_name: Option<String>,
    pub phase: Phase,
    pub pid: Option<u32>,
    pub hwnd: Option<u64>,
    pub command_line: String,
    pub notes: Vec<String>,
    pub placeholders: Placeholders,
    pub env: BTreeMap<String, String>,
    child: Option<Child>,
    exe_name: Option<String>,
    title_regex: Option<Regex>,
    preexisting_pids: HashSet<u32>,
}

/// Progress callback payload.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Planned {
        player: u8,
        dir: PathBuf,
        viewport: PixelRect,
    },
    Patched {
        path: PathBuf,
    },
    Launching {
        player: u8,
        command_line: String,
    },
    Note {
        player: u8,
        text: String,
    },
    WindowFound {
        player: u8,
        hwnd: u64,
        title: String,
    },
    Positioned {
        player: u8,
        rect: PixelRect,
    },
    Failed {
        player: u8,
        reason: String,
    },
}

/// Written to the session folder so a crashed orchestrator can still be cleaned up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManifest {
    pub id: String,
    pub profile_id: String,
    pub started_unix_ms: u64,
    pub ended: bool,
    pub instances: Vec<ManifestInstance>,
    pub backups: BackupSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestInstance {
    pub player: u8,
    pub pid: Option<u32>,
    pub hwnd: Option<u64>,
    pub box_name: Option<String>,
    pub dir: PathBuf,
}

pub const MANIFEST_FILE: &str = "session.json";

pub struct Session {
    pub id: String,
    pub profile: GameProfile,
    pub layout: Layout,
    pub monitors: Vec<Monitor>,
    pub dir: PathBuf,
    pub options: SessionOptions,
    pub instances: Vec<Instance>,
    backups: BackupSet,
    backdrop: Option<Backdrop>,
    provider: Box<dyn Provider>,
    started: Option<Instant>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("profile", &self.profile.game.id)
            .field("layout", &self.layout.name)
            .field("instances", &self.instances.len())
            .field("dir", &self.dir)
            .finish()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Session {
    /// Validate inputs, resolve the layout, prepare isolation and write every `instance.json`.
    pub fn plan(
        profile: GameProfile,
        players: Vec<PlayerSlot>,
        layout: Layout,
        monitors: Vec<Monitor>,
        options: SessionOptions,
    ) -> Result<Session> {
        profile.validate()?;
        if players.is_empty() || players.len() > 4 {
            return Err(anyhow!(
                "a session needs 1 to 4 players, got {}",
                players.len()
            ));
        }
        if players.len() > profile.game.max_players as usize {
            return Err(anyhow!(
                "{} supports at most {} players",
                profile.game.name,
                profile.game.max_players
            ));
        }
        let mut numbers: Vec<u8> = players.iter().map(|p| p.number).collect();
        numbers.sort_unstable();
        let expected: Vec<u8> = (1..=players.len() as u8).collect();
        if numbers != expected {
            return Err(anyhow!(
                "player numbers must be exactly 1..={}, got {:?}",
                players.len(),
                numbers
            ));
        }
        if layout.player_count as usize != players.len() {
            return Err(anyhow!(
                "layout '{}' is for {} players but {} were given",
                layout.name,
                layout.player_count,
                players.len()
            ));
        }
        if monitors.is_empty() {
            return Err(anyhow!("no monitors known; on Windows they are detected automatically, elsewhere pass --monitors"));
        }
        let resolved = layout.resolve(&monitors)?;

        let id = paths::new_session_id();
        let root = options
            .session_root
            .clone()
            .unwrap_or_else(paths::sessions_dir);
        let dir = root.join(&id);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating session folder {}", dir.display()))?;
        let provider = providers::provider_for(profile.game.provider);
        let target_exe = profile.target_exe_name().unwrap_or_default();
        let title_regex = match &profile.launch.window.title_regex {
            Some(re) => Some(Regex::new(re)?),
            None => None,
        };

        let mut instances = Vec::with_capacity(players.len());
        for slot in &players {
            let rv = resolved
                .iter()
                .find(|r| r.player == slot.number)
                .ok_or_else(|| anyhow!("layout has no viewport for player {}", slot.number))?;
            let default_dir = dir.join(format!("p{}", slot.number));
            let iso = isolation::prepare(&profile, slot.number, &default_dir, options.dry_run)?;
            let instance_dir = iso.instance_dir.clone().unwrap_or(default_dir);

            let mut placeholders = Placeholders::new();
            placeholders
                .set("player", slot.number.to_string())
                .set("nickname", slot.nickname.clone())
                .set("width", rv.rect.width.to_string())
                .set("height", rv.rect.height.to_string())
                .set("x", rv.rect.x.to_string())
                .set("y", rv.rect.y.to_string())
                .set("monitor", rv.monitor.to_string())
                .set("instance_dir", instance_dir.to_string_lossy().to_string())
                .set("session_id", id.clone());

            let mut env = iso.env.clone();
            for (k, v) in &profile.launch.env {
                env.insert(k.clone(), placeholders.expand(v)?);
            }
            if profile.hooks.inject {
                env.insert(
                    INSTANCE_DIR_ENV.to_string(),
                    instance_dir.to_string_lossy().to_string(),
                );
            }

            let config = InstanceConfig {
                version: PROTO_VERSION,
                session_id: id.clone(),
                player: slot.number,
                nickname: slot.nickname.clone(),
                target_exe: target_exe.clone(),
                controller: slot.controller.clone(),
                viewport: rv.rect,
                hooks: profile.hooks.to_flags(),
                window_title_regex: profile.launch.window.title_regex.clone(),
            };
            ipc::write_instance_config(&instance_dir, &config)
                .with_context(|| format!("writing instance.json for player {}", slot.number))?;

            let mut notes = iso.notes.clone();
            if profile.hooks.inject && iso.box_name.is_none() {
                notes.push(
                    "hook DLL injection for unsandboxed instances arrives with milestone M1".into(),
                );
            }

            instances.push(Instance {
                player: slot.number,
                nickname: slot.nickname.clone(),
                controller: slot.controller.clone(),
                viewport: rv.rect,
                monitor: rv.monitor,
                dir: instance_dir,
                box_name: iso.box_name,
                phase: Phase::Planned,
                pid: None,
                hwnd: None,
                command_line: String::new(),
                notes,
                placeholders,
                env,
                child: None,
                exe_name: None,
                title_regex: title_regex.clone(),
                preexisting_pids: HashSet::new(),
            });
        }
        instances.sort_by_key(|i| i.player);

        Ok(Session {
            id,
            profile,
            layout,
            monitors,
            dir,
            options,
            instances,
            backups: BackupSet::default(),
            backdrop: None,
            provider,
            started: None,
        })
    }

    fn request<'a>(&'a self, inst: &'a Instance) -> LaunchRequest<'a> {
        LaunchRequest {
            profile: &self.profile,
            session_id: &self.id,
            player: inst.player,
            nickname: &inst.nickname,
            viewport: inst.viewport,
            instance_dir: &inst.dir,
            env: inst.env.clone(),
            placeholders: &inst.placeholders,
            box_name: inst.box_name.as_deref(),
            dry_run: self.options.dry_run,
        }
    }

    /// Every patch that would be applied, per player: profile `[[patch]]` entries plus
    /// provider-specific ones.
    pub fn planned_patches(&self) -> Result<Vec<(u8, PreparedPatch)>> {
        let mut out = Vec::new();
        for inst in &self.instances {
            for spec in &self.profile.patch {
                let path = patch::resolve_patch_path(spec, &inst.placeholders, paths::env_lookup)?;
                out.push((
                    inst.player,
                    PreparedPatch {
                        path,
                        spec: spec.clone(),
                    },
                ));
            }
            for p in self.provider.prepare(&self.request(inst))? {
                out.push((inst.player, p));
            }
        }
        Ok(out)
    }

    /// Back up and patch config files. No-op in a dry run.
    pub fn apply_patches(&mut self, on_event: &mut dyn FnMut(SessionEvent)) -> Result<()> {
        let planned = self.planned_patches()?;
        if self.options.dry_run {
            return Ok(());
        }
        let backup_dir = self.dir.join("backups");
        for (player, p) in planned {
            self.backups
                .backup(&p.path, &backup_dir)
                .with_context(|| format!("backing up {}", p.path.display()))?;
            let inst = self
                .instances
                .iter()
                .find(|i| i.player == player)
                .expect("instance exists");
            patch::apply_patch(&p.spec, &p.path, &inst.placeholders)
                .with_context(|| format!("patching {} for player {player}", p.path.display()))?;
            on_event(SessionEvent::Patched {
                path: p.path.clone(),
            });
        }
        self.write_manifest(false)?;
        Ok(())
    }

    /// Launch every instance in player order, waiting for each window before the next start.
    pub fn launch_all(&mut self, on_event: &mut dyn FnMut(SessionEvent)) -> Result<()> {
        self.started = Some(Instant::now());
        for inst in &self.instances {
            on_event(SessionEvent::Planned {
                player: inst.player,
                dir: inst.dir.clone(),
                viewport: inst.viewport,
            });
        }
        if self.options.backdrop && !self.options.dry_run {
            self.backdrop = Backdrop::create(&self.monitors);
        }
        let stagger = Duration::from_millis(self.profile.launch.stagger_ms);
        for i in 0..self.instances.len() {
            if i > 0 && !self.options.dry_run {
                std::thread::sleep(stagger);
            }
            if let Err(e) = self.launch_one(i, on_event) {
                let inst = &mut self.instances[i];
                inst.phase = Phase::Failed(e.to_string());
                on_event(SessionEvent::Failed {
                    player: inst.player,
                    reason: e.to_string(),
                });
                continue;
            }
            if self.options.dry_run {
                continue;
            }
            self.wait_for_window(i, on_event);
        }
        if !self.options.dry_run {
            self.write_manifest(false)?;
        }
        Ok(())
    }

    fn launch_one(&mut self, i: usize, on_event: &mut dyn FnMut(SessionEvent)) -> Result<()> {
        let launched = {
            let inst = &self.instances[i];
            self.provider.launch(&self.request(inst))?
        };
        let inst = &mut self.instances[i];
        inst.command_line = launched.command_line.clone();
        inst.pid = launched.root_pid;
        inst.child = launched.child;
        inst.exe_name = launched.exe_name;
        inst.preexisting_pids = launched.preexisting_pids;
        if inst.title_regex.is_none() {
            if let Some(re) = &launched.title_regex {
                inst.title_regex = Some(Regex::new(re)?);
            }
        }
        inst.phase = if self.options.dry_run {
            Phase::Planned
        } else {
            Phase::Launching
        };
        on_event(SessionEvent::Launching {
            player: inst.player,
            command_line: inst.command_line.clone(),
        });
        for note in launched.notes.iter().chain(inst.notes.iter()) {
            on_event(SessionEvent::Note {
                player: inst.player,
                text: note.clone(),
            });
        }
        Ok(())
    }

    fn claimed_windows(&self, except: usize) -> HashSet<u64> {
        self.instances
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != except)
            .filter_map(|(_, inst)| inst.hwnd)
            .collect()
    }

    fn wait_for_window(&mut self, i: usize, on_event: &mut dyn FnMut(SessionEvent)) {
        let exclude = self.claimed_windows(i);
        let timeout = Duration::from_millis(self.profile.launch.window.wait_ms);
        let class = self.profile.launch.window.class.clone();
        let topmost = self.options.topmost;
        let inst = &mut self.instances[i];
        inst.phase = Phase::WaitingForWindow;
        let query = WindowQuery {
            root_pid: inst.pid,
            exe_name: inst.exe_name.clone(),
            title_regex: inst.title_regex.clone(),
            class,
            exclude,
            preexisting_pids: inst.preexisting_pids.clone(),
        };
        match window::wait_for_main_window(&query, timeout) {
            Some(w) => {
                inst.hwnd = Some(w.hwnd);
                if inst.pid.is_none() || inst.exe_name.is_some() {
                    inst.pid = Some(w.pid);
                }
                on_event(SessionEvent::WindowFound {
                    player: inst.player,
                    hwnd: w.hwnd,
                    title: w.title.clone(),
                });
                let _ = window::strip_decorations(w.hwnd);
                match window::place_window(w.hwnd, inst.viewport, topmost) {
                    Ok(()) => {
                        inst.phase = Phase::Positioned;
                        on_event(SessionEvent::Positioned {
                            player: inst.player,
                            rect: inst.viewport,
                        });
                    }
                    Err(e) => {
                        inst.phase = Phase::Failed(format!("cannot place window: {e}"));
                        on_event(SessionEvent::Failed {
                            player: inst.player,
                            reason: inst.phase.to_string(),
                        });
                    }
                }
            }
            None => {
                let reason = format!("no main window appeared within {} s", timeout.as_secs());
                inst.phase = Phase::Failed(reason.clone());
                on_event(SessionEvent::Failed {
                    player: inst.player,
                    reason,
                });
            }
        }
    }

    /// Check for exits and re-pin windows that moved. Call every few hundred milliseconds.
    pub fn tick(&mut self) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        let topmost = self.options.topmost;
        let pin = self.profile.hooks.pin_window;
        for inst in &mut self.instances {
            if matches!(inst.phase, Phase::Exited(_) | Phase::Failed(_)) {
                continue;
            }
            if let Some(child) = inst.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    // The child may be a launcher that exits while the game lives on.
                    let game_alive = inst.hwnd.is_some_and(window::is_window);
                    if !game_alive {
                        inst.phase = Phase::Exited(status.code());
                        continue;
                    }
                }
            }
            if let Some(h) = inst.hwnd {
                if !window::is_window(h) {
                    inst.phase = Phase::Exited(None);
                    continue;
                }
                if pin {
                    if let Some(rect) = window::window_rect(h) {
                        if rect != inst.viewport
                            && window::place_window(h, inst.viewport, topmost).is_ok()
                        {
                            events.push(SessionEvent::Positioned {
                                player: inst.player,
                                rect: inst.viewport,
                            });
                        }
                    }
                }
                inst.phase = Phase::Running;
            }
        }
        events
    }

    pub fn all_finished(&self) -> bool {
        self.instances
            .iter()
            .all(|i| matches!(i.phase, Phase::Exited(_) | Phase::Failed(_)))
    }

    /// Re-place every window after the layout changed (live re-layout, swap, spotlight).
    pub fn apply_layout(&mut self, layout: Layout) -> Result<()> {
        let resolved = layout.resolve(&self.monitors)?;
        for inst in &mut self.instances {
            if let Some(rv) = resolved.iter().find(|r| r.player == inst.player) {
                inst.viewport = rv.rect;
                inst.monitor = rv.monitor;
                if let Ok(mut cfg) = ipc::read_instance_config(&inst.dir) {
                    cfg.viewport = rv.rect;
                    let _ = ipc::write_instance_config(&inst.dir, &cfg);
                }
                if let Some(h) = inst.hwnd {
                    let _ = window::place_window(h, rv.rect, self.options.topmost);
                }
            }
        }
        self.layout = layout;
        Ok(())
    }

    /// Terminate instances, restore patched files, remove the backdrop. Returns warnings.
    pub fn stop(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        for inst in &mut self.instances {
            if let Some(child) = inst.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
            if let Some(pid) = inst.pid {
                if !self.options.dry_run {
                    if let Err(e) = window::terminate_process(pid) {
                        if inst.hwnd.is_some_and(window::is_window) {
                            warnings.push(format!(
                                "player {}: could not terminate pid {pid}: {e}",
                                inst.player
                            ));
                        }
                    }
                }
            }
            inst.phase = Phase::Exited(None);
        }
        for (path, err) in self.backups.restore_all() {
            warnings.push(format!("could not restore {}: {err}", path.display()));
        }
        self.backdrop = None;
        if !self.options.dry_run {
            if let Err(e) = self.write_manifest(true) {
                warnings.push(format!("could not update session manifest: {e}"));
            }
        }
        warnings
    }

    pub fn manifest(&self, ended: bool) -> SessionManifest {
        SessionManifest {
            id: self.id.clone(),
            profile_id: self.profile.game.id.clone(),
            started_unix_ms: now_ms(),
            ended,
            instances: self
                .instances
                .iter()
                .map(|i| ManifestInstance {
                    player: i.player,
                    pid: i.pid,
                    hwnd: i.hwnd,
                    box_name: i.box_name.clone(),
                    dir: i.dir.clone(),
                })
                .collect(),
            backups: self.backups.clone(),
        }
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join(MANIFEST_FILE)
    }

    fn write_manifest(&self, ended: bool) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.manifest(ended))?;
        std::fs::write(self.manifest_path(), json)?;
        Ok(())
    }
}

/// Clean up after a session whose orchestrator is gone: kill recorded pids and restore files.
pub fn cleanup_manifest(path: &Path) -> Result<Vec<String>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut manifest: SessionManifest = serde_json::from_str(&text)?;
    let mut warnings = Vec::new();
    for inst in &manifest.instances {
        if let Some(pid) = inst.pid {
            if let Err(e) = window::terminate_process(pid) {
                warnings.push(format!("player {}: pid {pid}: {e}", inst.player));
            }
        }
    }
    for (p, err) in manifest.backups.restore_all() {
        warnings.push(format!("could not restore {}: {err}", p.display()));
    }
    manifest.ended = true;
    std::fs::write(path, serde_json::to_string_pretty(&manifest)?)?;
    Ok(warnings)
}

/// Newest session manifest under `root` that has not ended.
pub fn find_unfinished_session(root: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path().join(MANIFEST_FILE);
            let text = std::fs::read_to_string(&p).ok()?;
            let m: SessionManifest = serde_json::from_str(&text).ok()?;
            if m.ended {
                return None;
            }
            let modified = std::fs::metadata(&p).ok()?.modified().ok()?;
            Some((modified, p))
        })
        .collect();
    candidates.sort();
    candidates.pop().map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{preset, PresetKind, PresetOptions};
    use crate::profile::GameProfile;

    const EXE_PROFILE: &str = r#"
[game]
id = "demo"
name = "Demo"
provider = "exe"
path = "C:/Games/Demo/demo.exe"

[isolation]
mode = "env"

[launch]
args = ["-w", "${width}", "-h", "${height}", "-name", "${nickname}"]

[[patch]]
file = "${instance_dir}/settings.ini"
format = "ini"
section = "Video"
set = { Width = "${width}", Fullscreen = 0 }
create_if_missing = true
"#;

    fn monitors() -> Vec<Monitor> {
        vec![Monitor::simple("M1", 0, 0, 1920, 1080, true)]
    }

    fn players(n: u8) -> Vec<PlayerSlot> {
        (1..=n)
            .map(|p| {
                PlayerSlot::new(
                    p,
                    ControllerAssignment::XInputSlot {
                        slot: (p - 1) as u32,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn plan_writes_instance_json_and_expands_placeholders() {
        let root = tempfile::tempdir().unwrap();
        let profile = GameProfile::from_toml_str(EXE_PROFILE).unwrap();
        let layout = preset(
            PresetKind::LeftRight,
            2,
            &monitors(),
            &PresetOptions::default(),
        )
        .unwrap();
        let options = SessionOptions {
            dry_run: true,
            session_root: Some(root.path().to_path_buf()),
            ..Default::default()
        };
        let session = Session::plan(profile, players(2), layout, monitors(), options).unwrap();
        assert_eq!(session.instances.len(), 2);
        let p2 = &session.instances[1];
        assert_eq!(p2.viewport, PixelRect::new(960, 0, 960, 1080));
        let cfg = ipc::read_instance_config(&p2.dir).unwrap();
        assert_eq!(cfg.player, 2);
        assert_eq!(cfg.target_exe, "demo.exe");
        assert_eq!(cfg.controller, ControllerAssignment::XInputSlot { slot: 1 });
        assert!(p2.env["APPDATA"].contains("p2"));
        assert_eq!(p2.env[INSTANCE_DIR_ENV], p2.dir.to_string_lossy());
        let patches = session.planned_patches().unwrap();
        assert_eq!(patches.len(), 2);
        assert!(patches[1].1.path.starts_with(&p2.dir));
    }

    #[test]
    fn dry_run_launch_produces_command_lines_without_spawning() {
        let root = tempfile::tempdir().unwrap();
        let profile = GameProfile::from_toml_str(EXE_PROFILE).unwrap();
        let layout = preset(PresetKind::Grid, 4, &monitors(), &PresetOptions::default()).unwrap();
        let options = SessionOptions {
            dry_run: true,
            session_root: Some(root.path().to_path_buf()),
            ..Default::default()
        };
        let mut session = Session::plan(profile, players(4), layout, monitors(), options).unwrap();
        let mut events = Vec::new();
        session.apply_patches(&mut |e| events.push(e)).unwrap();
        session.launch_all(&mut |e| events.push(e)).unwrap();
        assert!(session.instances.iter().all(|i| i.phase == Phase::Planned));
        assert!(session.instances[2].command_line.contains("-w 960 -h 540"));
        assert!(session.instances[2].command_line.contains("\"Player 3\""));
        assert!(events
            .iter()
            .any(|e| matches!(e, SessionEvent::Launching { player: 4, .. })));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SessionEvent::Patched { .. })),
            "dry run patches nothing"
        );
    }

    #[test]
    fn plan_rejects_bad_player_sets_and_layout_mismatch() {
        let root = tempfile::tempdir().unwrap();
        let profile = GameProfile::from_toml_str(EXE_PROFILE).unwrap();
        let layout = preset(
            PresetKind::LeftRight,
            2,
            &monitors(),
            &PresetOptions::default(),
        )
        .unwrap();
        let options = SessionOptions {
            dry_run: true,
            session_root: Some(root.path().to_path_buf()),
            ..Default::default()
        };
        let err = Session::plan(
            profile.clone(),
            players(3),
            layout.clone(),
            monitors(),
            options.clone(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("for 2 players"));
        let dup = vec![
            PlayerSlot::new(1, ControllerAssignment::None),
            PlayerSlot::new(1, ControllerAssignment::None),
        ];
        let err = Session::plan(profile, dup, layout, monitors(), options).unwrap_err();
        assert!(err.to_string().contains("player numbers"));
    }

    #[test]
    fn manifest_round_trips_and_unfinished_lookup_works() {
        let root = tempfile::tempdir().unwrap();
        let profile = GameProfile::from_toml_str(EXE_PROFILE).unwrap();
        let layout = preset(
            PresetKind::TopBottom,
            2,
            &monitors(),
            &PresetOptions::default(),
        )
        .unwrap();
        let options = SessionOptions {
            dry_run: true,
            session_root: Some(root.path().to_path_buf()),
            ..Default::default()
        };
        let session = Session::plan(profile, players(2), layout, monitors(), options).unwrap();
        let manifest = session.manifest(false);
        std::fs::write(
            session.manifest_path(),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
        assert_eq!(
            find_unfinished_session(root.path()).unwrap(),
            session.manifest_path()
        );
        let warnings = cleanup_manifest(&session.manifest_path()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(find_unfinished_session(root.path()).is_none());
    }
}
