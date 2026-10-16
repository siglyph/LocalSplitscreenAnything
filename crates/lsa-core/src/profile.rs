//! Game profiles: one TOML file per game describing how to launch it, how to isolate
//! instances, which hooks to apply and which config files to patch per instance.
//!
//! `_default.toml` is merged underneath every game profile (tables merge recursively, arrays
//! and scalars in the game profile win), so a profile only has to state what is special.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use lsa_proto::HookFlags;
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// File name of the profile merged underneath every other profile.
pub const DEFAULT_PROFILE_FILE: &str = "_default.toml";

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    #[error("profile '{0}' is invalid: {1}")]
    Invalid(String, String),
    #[error("unknown placeholder '${{{0}}}'")]
    UnknownPlaceholder(String),
    #[error("no profile with id '{0}' in {1:?}")]
    NotFound(String, Vec<PathBuf>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    /// Run an executable directly.
    Exe,
    /// Real Steam client per player (host slot unsandboxed, others in Sandboxie boxes).
    Steam,
    /// Minecraft Java through Prism Launcher.
    Prism,
    /// Steam game with the gbe_fork emulator per instance (fallback).
    SteamEmu,
}

impl ProviderKind {
    pub fn id(&self) -> &'static str {
        match self {
            ProviderKind::Exe => "exe",
            ProviderKind::Steam => "steam",
            ProviderKind::Prism => "prism",
            ProviderKind::SteamEmu => "steam-emu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IsolationMode {
    /// Nothing: instances share everything.
    #[default]
    None,
    /// Per-instance `APPDATA`/`LOCALAPPDATA`/`USERPROFILE`/`TEMP` for games that honour them.
    Env,
    /// One Sandboxie-Plus box per player slot.
    Sandboxie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstanceDirMode {
    /// Launch every instance from the installed game folder.
    #[default]
    None,
    /// Per-instance folder of directory junctions / file hardlinks (Nucleus style).
    Symlink,
    /// Per-instance full copy of the game folder.
    Copy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameSection {
    /// Short identifier; also the profile file name without `.toml`.
    pub id: String,
    pub name: String,
    pub provider: ProviderKind,
    #[serde(default)]
    pub steam_appid: Option<u32>,
    /// Executable file name of the process to manage/hook (may differ from the launcher).
    #[serde(default)]
    pub executable: Option<String>,
    /// For `exe`: full path of the executable. For others: optional game folder override.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default = "default_max_players")]
    pub max_players: u8,
    #[serde(default)]
    pub notes: Option<String>,
}

fn default_max_players() -> u8 {
    4
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IsolationSection {
    pub mode: IsolationMode,
    pub instance_dir: InstanceDirMode,
    /// Environment variables redirected into the instance directory in `env` mode.
    pub env_redirect: Vec<String>,
}

impl Default for IsolationSection {
    fn default() -> Self {
        Self {
            mode: IsolationMode::None,
            instance_dir: InstanceDirMode::None,
            env_redirect: ["APPDATA", "LOCALAPPDATA", "USERPROFILE", "TEMP", "TMP"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowMatch {
    /// Regex matched against window titles when picking the main window.
    pub title_regex: Option<String>,
    /// Exact window class name, if the game has a distinctive one.
    pub class: Option<String>,
    /// How long to wait for the main window to appear.
    pub wait_ms: u64,
}

impl Default for WindowMatch {
    fn default() -> Self {
        Self {
            title_regex: None,
            class: None,
            wait_ms: 60_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchSection {
    /// Command line arguments; `${...}` placeholders are expanded per instance.
    pub args: Vec<String>,
    /// Delay between starting consecutive instances.
    pub stagger_ms: u64,
    pub window: WindowMatch,
    /// Extra environment variables for the game process.
    pub env: BTreeMap<String, String>,
}

impl Default for LaunchSection {
    fn default() -> Self {
        Self {
            args: Vec::new(),
            stagger_ms: 3_000,
            window: WindowMatch::default(),
            env: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HooksSection {
    /// Inject the hook DLL at all. `false` for games that need no hooks (Minecraft).
    pub inject: bool,
    pub xinput: bool,
    pub fake_focus: bool,
    pub force_windowed: bool,
    pub lie_screen_size: bool,
    pub pin_window: bool,
    pub clip_cursor: bool,
    pub rename_handles: Vec<String>,
    pub rename_all_handles: bool,
}

impl Default for HooksSection {
    fn default() -> Self {
        let f = HookFlags::default();
        Self {
            inject: true,
            xinput: f.xinput,
            fake_focus: f.fake_focus,
            force_windowed: f.force_windowed,
            lie_screen_size: f.lie_screen_size,
            pin_window: f.pin_window,
            clip_cursor: f.clip_cursor,
            rename_handles: f.rename_handles,
            rename_all_handles: f.rename_all_handles,
        }
    }
}

impl HooksSection {
    pub fn to_flags(&self) -> HookFlags {
        HookFlags {
            xinput: self.xinput,
            fake_focus: self.fake_focus,
            force_windowed: self.force_windowed,
            lie_screen_size: self.lie_screen_size,
            pin_window: self.pin_window,
            clip_cursor: self.clip_cursor,
            rename_handles: self.rename_handles.clone(),
            rename_all_handles: self.rename_all_handles,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PatchFormat {
    /// JSON document; `set` keys are dotted paths (`video.fullscreen`).
    Json,
    /// INI file; `section` selects the section, `set` keys are entries.
    Ini,
    /// `key:value` lines (Minecraft `options.txt`).
    KeyValueColon,
    /// `key=value` lines (Prism `instance.cfg`, many launchers).
    KeyValueEquals,
    /// Regex replacement: `pattern` → `replace` on the whole file.
    Regex,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchSection {
    /// File to patch; `%VAR%` and `${...}` are expanded per instance.
    pub file: String,
    pub format: PatchFormat,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub set: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub replace: Option<String>,
    #[serde(default)]
    pub create_if_missing: bool,
}

/// Minecraft Java through Prism Launcher.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrismSection {
    /// Path to `prismlauncher.exe`; auto-detected when empty.
    pub launcher: Option<String>,
    /// Prism data directory holding `instances/`; auto-detected when empty.
    pub data_dir: Option<String>,
    /// Prism instance folder names, one per player slot.
    pub instances: Vec<String>,
    /// Prism account profile names, one per player slot (fewer is allowed; the rest use the
    /// launcher default).
    pub accounts: Vec<String>,
    /// Server every instance joins on launch (`host:port`).
    pub server: Option<String>,
    /// Patch `options.txt` so the game runs windowed and keeps running when unfocused.
    pub patch_options: bool,
    /// Write the window size into the Prism instance settings.
    pub patch_instance_cfg: bool,
}

impl Default for PrismSection {
    fn default() -> Self {
        Self {
            launcher: None,
            data_dir: None,
            instances: Vec::new(),
            accounts: Vec::new(),
            server: None,
            patch_options: true,
            patch_instance_cfg: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SteamSection {
    /// Run player 1 on the host's unsandboxed Steam client.
    pub host_slot_unsandboxed: bool,
    /// Path of `steam.exe`; auto-detected from the registry when empty.
    pub steam_exe: Option<String>,
}

impl Default for SteamSection {
    fn default() -> Self {
        Self {
            host_slot_unsandboxed: true,
            steam_exe: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameProfile {
    pub game: GameSection,
    #[serde(default)]
    pub isolation: IsolationSection,
    #[serde(default)]
    pub launch: LaunchSection,
    #[serde(default)]
    pub hooks: HooksSection,
    #[serde(default)]
    pub patch: Vec<PatchSection>,
    #[serde(default)]
    pub prism: Option<PrismSection>,
    #[serde(default)]
    pub steam: Option<SteamSection>,
    /// Default layout preset id for this game (e.g. `grid`).
    #[serde(default)]
    pub default_layout: Option<String>,
}

impl GameProfile {
    /// Parse a profile on its own (no `_default.toml` merge).
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Parse `game_text` merged on top of `default_text`.
    pub fn from_toml_with_default(
        game_text: &str,
        default_text: Option<&str>,
    ) -> Result<Self, toml::de::Error> {
        let game: toml::Value = toml::from_str(game_text)?;
        let merged = match default_text {
            Some(d) => {
                let mut base: toml::Value = toml::from_str(d)?;
                merge_toml(&mut base, game);
                base
            }
            None => game,
        };
        merged.try_into()
    }

    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Sanity checks that do not need the file system.
    pub fn validate(&self) -> Result<(), ProfileError> {
        let invalid = |msg: &str| ProfileError::Invalid(self.game.id.clone(), msg.to_string());
        if self.game.id.is_empty()
            || !self
                .game
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(invalid(
                "id must be non-empty and contain only letters, digits, '-' or '_'",
            ));
        }
        if !(1..=4).contains(&self.game.max_players) {
            return Err(invalid("max_players must be between 1 and 4"));
        }
        match self.game.provider {
            ProviderKind::Exe => {
                if self.game.path.as_deref().unwrap_or("").is_empty() {
                    return Err(invalid("exe provider needs game.path"));
                }
            }
            ProviderKind::Steam | ProviderKind::SteamEmu => {
                if self.game.steam_appid.is_none() {
                    return Err(invalid("steam providers need game.steam_appid"));
                }
                if self.game.executable.as_deref().unwrap_or("").is_empty() {
                    return Err(invalid(
                        "steam providers need game.executable (the game's exe name)",
                    ));
                }
            }
            ProviderKind::Prism => {
                let prism = self
                    .prism
                    .as_ref()
                    .ok_or_else(|| invalid("prism provider needs a [prism] section"))?;
                if prism.instances.is_empty() {
                    return Err(invalid(
                        "[prism].instances must list at least one Prism instance folder",
                    ));
                }
            }
        }
        if let Some(re) = &self.launch.window.title_regex {
            Regex::new(re).map_err(|e| invalid(&format!("launch.window.title_regex: {e}")))?;
        }
        for p in &self.patch {
            if p.format == PatchFormat::Regex {
                let pat = p
                    .pattern
                    .as_deref()
                    .ok_or_else(|| invalid("regex patch needs `pattern`"))?;
                Regex::new(pat).map_err(|e| invalid(&format!("patch pattern: {e}")))?;
                if p.replace.is_none() {
                    return Err(invalid("regex patch needs `replace`"));
                }
            } else if p.set.is_empty() {
                return Err(invalid(&format!("patch for {} sets nothing", p.file)));
            }
            if p.format == PatchFormat::Ini && p.section.is_none() {
                return Err(invalid(&format!(
                    "ini patch for {} needs `section`",
                    p.file
                )));
            }
        }
        Ok(())
    }

    /// Executable file name the hook DLL should activate in.
    pub fn target_exe_name(&self) -> Option<String> {
        if let Some(exe) = &self.game.executable {
            return Some(exe.clone());
        }
        self.game
            .path
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .map(|n| n.to_string_lossy().to_string())
    }
}

/// Deep-merge `over` into `base`: tables merge recursively, everything else is replaced.
pub fn merge_toml(base: &mut toml::Value, over: toml::Value) {
    match (base, over) {
        (toml::Value::Table(b), toml::Value::Table(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(bv) if bv.is_table() && v.is_table() => merge_toml(bv, v),
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

/// Values substituted for `${name}` placeholders in args, paths and patches.
#[derive(Debug, Clone, Default)]
pub struct Placeholders {
    pub values: BTreeMap<String, String>,
}

impl Placeholders {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) -> &mut Self {
        self.values.insert(key.to_string(), value.into());
        self
    }

    /// Replace every `${name}`; unknown names are an error so typos surface early.
    pub fn expand(&self, text: &str) -> Result<String, ProfileError> {
        let re = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("static regex");
        let mut err: Option<ProfileError> = None;
        let out = re.replace_all(text, |caps: &regex::Captures| {
            let key = &caps[1];
            match self.values.get(key) {
                Some(v) => v.clone(),
                None => {
                    if err.is_none() {
                        err = Some(ProfileError::UnknownPlaceholder(key.to_string()));
                    }
                    String::new()
                }
            }
        });
        match err {
            Some(e) => Err(e),
            None => Ok(out.into_owned()),
        }
    }

    pub fn expand_all(&self, items: &[String]) -> Result<Vec<String>, ProfileError> {
        items.iter().map(|s| self.expand(s)).collect()
    }
}

/// Expand Windows-style `%NAME%` environment references using `lookup`. Unknown names are
/// left untouched, matching what `cmd.exe` does.
pub fn expand_env_refs(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let re = Regex::new(r"%([A-Za-z_][A-Za-z0-9_()]*)%").expect("static regex");
    re.replace_all(text, |caps: &regex::Captures| {
        lookup(&caps[1]).unwrap_or_else(|| caps[0].to_string())
    })
    .into_owned()
}

/// Where profiles are looked up, in priority order (later directories override earlier ones
/// by id).
#[derive(Debug, Clone, Default)]
pub struct ProfileStore {
    pub dirs: Vec<PathBuf>,
}

impl ProfileStore {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self { dirs }
    }

    fn read(path: &Path) -> Result<String, ProfileError> {
        fs::read_to_string(path).map_err(|source| ProfileError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    fn default_text(&self) -> Result<Option<String>, ProfileError> {
        // The last directory that has a _default.toml wins, like profiles do.
        let mut found = None;
        for dir in &self.dirs {
            let p = dir.join(DEFAULT_PROFILE_FILE);
            if p.is_file() {
                found = Some(Self::read(&p)?);
            }
        }
        Ok(found)
    }

    pub fn load_path(&self, path: &Path) -> Result<GameProfile, ProfileError> {
        let text = Self::read(path)?;
        let default = self.default_text()?;
        let profile =
            GameProfile::from_toml_with_default(&text, default.as_deref()).map_err(|source| {
                ProfileError::Parse {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                }
            })?;
        profile.validate()?;
        Ok(profile)
    }

    /// Find a profile by id (`terraria`) or by path to a `.toml` file.
    pub fn load(&self, id_or_path: &str) -> Result<GameProfile, ProfileError> {
        let as_path = Path::new(id_or_path);
        if as_path.extension().is_some_and(|e| e == "toml") && as_path.is_file() {
            return self.load_path(as_path);
        }
        let mut found: Option<PathBuf> = None;
        for dir in &self.dirs {
            let p = dir.join(format!("{id_or_path}.toml"));
            if p.is_file() {
                found = Some(p);
            }
        }
        match found {
            Some(p) => self.load_path(&p),
            None => Err(ProfileError::NotFound(
                id_or_path.to_string(),
                self.dirs.clone(),
            )),
        }
    }

    /// Every profile in every directory, deduplicated by id (later directories win).
    pub fn list(&self) -> Result<Vec<(PathBuf, GameProfile)>, ProfileError> {
        let mut by_id: BTreeMap<String, (PathBuf, GameProfile)> = BTreeMap::new();
        for dir in &self.dirs {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let is_profile = path.extension().is_some_and(|e| e == "toml")
                    && path.file_name().is_some_and(|n| n != DEFAULT_PROFILE_FILE);
                if !is_profile {
                    continue;
                }
                let profile = self.load_path(&path)?;
                by_id.insert(profile.game.id.clone(), (path, profile));
            }
        }
        Ok(by_id.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = r#"
[game]
id = "_default"
name = "default"
provider = "exe"
path = "unused"

[launch]
stagger_ms = 5000

[hooks]
xinput = true
rename_all_handles = false
"#;

    const TERRARIA: &str = r#"
[game]
id = "terraria"
name = "Terraria"
provider = "steam"
steam_appid = 105600
executable = "Terraria.exe"

[launch]
args = ["-w", "${width}", "-h", "${height}"]

[hooks]
rename_all_handles = true

[[patch]]
file = "%USERPROFILE%/Documents/My Games/Terraria/config.json"
format = "json"
set = { Fullscreen = false, DisplayWidth = "${width}" }
"#;

    #[test]
    fn merge_keeps_defaults_and_overrides() {
        let p = GameProfile::from_toml_with_default(TERRARIA, Some(DEFAULT)).unwrap();
        assert_eq!(p.game.id, "terraria");
        assert_eq!(p.game.provider, ProviderKind::Steam);
        assert_eq!(p.launch.stagger_ms, 5000, "inherited from default");
        assert_eq!(p.launch.args.len(), 4, "arrays come from the game profile");
        assert!(p.hooks.rename_all_handles, "game overrides default");
        assert!(p.hooks.xinput, "inherited");
        p.validate().unwrap();
        assert_eq!(p.target_exe_name().as_deref(), Some("Terraria.exe"));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let bad = "[game]\nid='x'\nname='x'\nprovider='exe'\npath='a'\n[launch]\nargz=[]\n";
        assert!(GameProfile::from_toml_str(bad).is_err());
    }

    #[test]
    fn validation_catches_missing_provider_fields() {
        let exe_without_path = "[game]\nid='x'\nname='x'\nprovider='exe'\n";
        let p = GameProfile::from_toml_str(exe_without_path).unwrap();
        assert!(
            matches!(p.validate(), Err(ProfileError::Invalid(_, m)) if m.contains("game.path"))
        );

        let prism_without_section = "[game]\nid='mc'\nname='mc'\nprovider='prism'\n";
        let p = GameProfile::from_toml_str(prism_without_section).unwrap();
        assert!(matches!(p.validate(), Err(ProfileError::Invalid(_, m)) if m.contains("[prism]")));

        let bad_regex = "[game]\nid='x'\nname='x'\nprovider='exe'\npath='a'\n[launch.window]\ntitle_regex='('\n";
        let p = GameProfile::from_toml_str(bad_regex).unwrap();
        assert!(
            matches!(p.validate(), Err(ProfileError::Invalid(_, m)) if m.contains("title_regex"))
        );
    }

    #[test]
    fn placeholders_expand_and_reject_unknown() {
        let mut ph = Placeholders::new();
        ph.set("width", "960").set("player", "2");
        assert_eq!(
            ph.expand("-w ${width} -p ${player}").unwrap(),
            "-w 960 -p 2"
        );
        assert!(
            matches!(ph.expand("${nope}"), Err(ProfileError::UnknownPlaceholder(k)) if k == "nope")
        );
        assert_eq!(ph.expand("no placeholders").unwrap(), "no placeholders");
    }

    #[test]
    fn env_refs_expand_like_cmd() {
        let out = expand_env_refs("%APPDATA%/x/%MISSING%/y", |k| {
            (k == "APPDATA").then(|| "C:/AD".to_string())
        });
        assert_eq!(out, "C:/AD/x/%MISSING%/y");
    }

    #[test]
    fn store_loads_by_id_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(DEFAULT_PROFILE_FILE), DEFAULT).unwrap();
        fs::write(dir.path().join("terraria.toml"), TERRARIA).unwrap();
        let store = ProfileStore::new(vec![dir.path().to_path_buf()]);
        let p = store.load("terraria").unwrap();
        assert_eq!(p.launch.stagger_ms, 5000);
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].1.game.id, "terraria");
        assert!(matches!(
            store.load("nope"),
            Err(ProfileError::NotFound(..))
        ));
    }

    #[test]
    fn hooks_section_maps_to_flags() {
        let h = HooksSection {
            rename_handles: vec![r"Global\X".into()],
            fake_focus: false,
            ..Default::default()
        };
        let f = h.to_flags();
        assert!(!f.fake_focus);
        assert_eq!(f.rename_handles, vec!["Global\\X".to_string()]);
    }
}
