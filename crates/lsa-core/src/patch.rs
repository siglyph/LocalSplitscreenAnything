//! Per-instance config file patching (JSON, INI, key/value files, regex) with backups that
//! are restored when the session ends.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::profile::{PatchFormat, PatchSection, Placeholders, ProfileError};

#[derive(Debug, Error)]
pub enum PatchError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: file does not exist (set create_if_missing = true to create it)")]
    Missing { path: PathBuf },
    #[error("{path}: not valid JSON: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{path}: {msg}")]
    Invalid { path: PathBuf, msg: String },
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

/// Original contents of patched files, so they can be restored.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackupSet {
    pub entries: Vec<BackupEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    pub path: PathBuf,
    /// Copy of the original file; `None` when the file did not exist before patching.
    pub backup_file: Option<PathBuf>,
}

impl BackupSet {
    /// Record `path` (copying it into `backup_dir`) unless it is already recorded.
    pub fn backup(&mut self, path: &Path, backup_dir: &Path) -> io::Result<()> {
        if self.entries.iter().any(|e| e.path == path) {
            return Ok(());
        }
        fs::create_dir_all(backup_dir)?;
        let backup_file = if path.exists() {
            let name = format!(
                "{}-{}.orig",
                self.entries.len(),
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            );
            let dest = backup_dir.join(name);
            fs::copy(path, &dest)?;
            Some(dest)
        } else {
            None
        };
        self.entries.push(BackupEntry {
            path: path.to_path_buf(),
            backup_file,
        });
        Ok(())
    }

    /// Put every file back. Returns the failures instead of stopping at the first one.
    pub fn restore_all(&self) -> Vec<(PathBuf, io::Error)> {
        let mut failures = Vec::new();
        for e in self.entries.iter().rev() {
            let result = match &e.backup_file {
                Some(src) => fs::copy(src, &e.path).map(|_| ()),
                None => match fs::remove_file(&e.path) {
                    Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                    other => other,
                },
            };
            if let Err(err) = result {
                failures.push((e.path.clone(), err));
            }
        }
        failures
    }

    pub fn save(&self, manifest: &Path) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(manifest, json)
    }

    pub fn load(manifest: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(manifest)?;
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// Resolve the target file of a patch: `%VAR%` then `${placeholder}` expansion.
pub fn resolve_patch_path(
    spec: &PatchSection,
    placeholders: &Placeholders,
    env: impl Fn(&str) -> Option<String>,
) -> Result<PathBuf, ProfileError> {
    let with_env = crate::profile::expand_env_refs(&spec.file, env);
    Ok(PathBuf::from(placeholders.expand(&with_env)?))
}

/// Apply one patch to `path`. The caller is responsible for backing the file up first.
pub fn apply_patch(
    spec: &PatchSection,
    path: &Path,
    placeholders: &Placeholders,
) -> Result<(), PatchError> {
    let io_err = |source: io::Error| PatchError::Io {
        path: path.to_path_buf(),
        source,
    };
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if spec.create_if_missing {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(io_err)?;
                }
                String::new()
            } else {
                return Err(PatchError::Missing {
                    path: path.to_path_buf(),
                });
            }
        }
        Err(e) => return Err(io_err(e)),
    };

    let expand_value = |v: &toml::Value| -> Result<toml::Value, PatchError> {
        Ok(match v {
            toml::Value::String(s) => toml::Value::String(placeholders.expand(s)?),
            other => other.clone(),
        })
    };

    let new_text = match spec.format {
        PatchFormat::Json => {
            let mut doc: serde_json::Value = if text.trim().is_empty() {
                serde_json::Value::Object(Default::default())
            } else {
                serde_json::from_str(&text).map_err(|source| PatchError::Json {
                    path: path.to_path_buf(),
                    source,
                })?
            };
            for (key, value) in &spec.set {
                let value = expand_value(value)?;
                set_json_path(&mut doc, key, toml_to_json(&value));
            }
            let mut out = serde_json::to_string_pretty(&doc).expect("serializing json value");
            out.push('\n');
            out
        }
        PatchFormat::Ini => {
            let section = spec.section.as_deref().ok_or_else(|| PatchError::Invalid {
                path: path.to_path_buf(),
                msg: "ini patch needs `section`".into(),
            })?;
            let mut out = text;
            for (key, value) in &spec.set {
                out = set_ini(&out, section, key, &value_to_string(&expand_value(value)?));
            }
            out
        }
        PatchFormat::KeyValueColon | PatchFormat::KeyValueEquals => {
            let sep = if spec.format == PatchFormat::KeyValueColon {
                ':'
            } else {
                '='
            };
            let mut out = text;
            for (key, value) in &spec.set {
                out = set_key_value(&out, sep, key, &value_to_string(&expand_value(value)?));
            }
            out
        }
        PatchFormat::Regex => {
            let pattern = spec.pattern.as_deref().unwrap_or("");
            let replace = placeholders.expand(spec.replace.as_deref().unwrap_or(""))?;
            let re = regex::Regex::new(pattern).map_err(|e| PatchError::Invalid {
                path: path.to_path_buf(),
                msg: e.to_string(),
            })?;
            re.replace_all(&text, replace.as_str()).into_owned()
        }
    };

    fs::write(path, new_text).map_err(io_err)
}

/// Render a TOML scalar the way config files expect it (strings unquoted).
pub fn value_to_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    }
}

pub fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::from(*i),
        toml::Value::Float(f) => serde_json::Value::from(*f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        toml::Value::Array(a) => serde_json::Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => serde_json::Value::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

/// Set `a.b.c` inside a JSON document, creating objects along the way.
pub fn set_json_path(doc: &mut serde_json::Value, dotted: &str, value: serde_json::Value) {
    let mut cur = doc;
    let parts: Vec<&str> = dotted.split('.').collect();
    for (i, part) in parts.iter().enumerate() {
        if !cur.is_object() {
            *cur = serde_json::Value::Object(Default::default());
        }
        let map = cur.as_object_mut().expect("object");
        if i + 1 == parts.len() {
            map.insert((*part).to_string(), value);
            return;
        }
        cur = map
            .entry((*part).to_string())
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
    }
}

fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Set `key=value` under `[section]`, preserving everything else.
pub fn set_ini(text: &str, section: &str, key: &str, value: &str) -> String {
    let eol = line_ending(text);
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let header = format!("[{section}]");
    let section_start = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case(&header));
    match section_start {
        Some(start) => {
            let end = lines[start + 1..]
                .iter()
                .position(|l| l.trim_start().starts_with('['))
                .map(|off| start + 1 + off)
                .unwrap_or(lines.len());
            let existing = lines[start + 1..end].iter().position(|l| {
                l.split_once('=')
                    .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
            });
            match existing {
                Some(off) => lines[start + 1 + off] = format!("{key}={value}"),
                None => {
                    // Insert before trailing blank lines of the section.
                    let mut insert_at = end;
                    while insert_at > start + 1 && lines[insert_at - 1].trim().is_empty() {
                        insert_at -= 1;
                    }
                    lines.insert(insert_at, format!("{key}={value}"));
                }
            }
        }
        None => {
            if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.push(format!("{key}={value}"));
        }
    }
    let mut out = lines.join(eol);
    out.push_str(eol);
    out
}

/// Replace the `key<sep>value` line or append one.
pub fn set_key_value(text: &str, sep: char, key: &str, value: &str) -> String {
    let eol = line_ending(text);
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let existing = lines
        .iter()
        .position(|l| l.split_once(sep).is_some_and(|(k, _)| k.trim() == key));
    let new_line = format!("{key}{sep}{value}");
    match existing {
        Some(i) => lines[i] = new_line,
        None => lines.push(new_line),
    }
    let mut out = lines.join(eol);
    out.push_str(eol);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn spec(format: PatchFormat, file: &str, set: &[(&str, toml::Value)]) -> PatchSection {
        PatchSection {
            file: file.to_string(),
            format,
            section: Some("Video".into()),
            set: set
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
            pattern: None,
            replace: None,
            create_if_missing: false,
        }
    }

    #[test]
    fn json_patch_sets_nested_and_expands_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.json");
        fs::write(
            &file,
            r#"{"video": {"fullscreen": true, "keep": 1}, "other": "x"}"#,
        )
        .unwrap();
        let mut ph = Placeholders::new();
        ph.set("width", "960");
        let s = spec(
            PatchFormat::Json,
            file.to_str().unwrap(),
            &[
                ("video.fullscreen", toml::Value::Boolean(false)),
                ("video.width", toml::Value::String("${width}".into())),
            ],
        );
        apply_patch(&s, &file, &ph).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(doc["video"]["fullscreen"], false);
        assert_eq!(doc["video"]["width"], "960");
        assert_eq!(doc["video"]["keep"], 1);
        assert_eq!(doc["other"], "x");
    }

    #[test]
    fn ini_patch_replaces_in_section_and_creates_section() {
        let text = "[Audio]\nvolume=3\n\n[Video]\nwidth=1\nheight=2\n\n[Other]\nz=1\n";
        let out = set_ini(text, "video", "width", "960");
        assert!(out.contains("[Video]\nwidth=960\nheight=2\n"));
        let out = set_ini(&out, "Video", "fullscreen", "0");
        assert!(out.contains("height=2\nfullscreen=0\n\n[Other]"));
        let out = set_ini("a=1\n", "New", "k", "v");
        assert_eq!(out, "a=1\n\n[New]\nk=v\n");
    }

    #[test]
    fn key_value_patch_handles_colon_and_equals_and_crlf() {
        let out = set_key_value("fullscreen:true\r\nfov:0.5\r\n", ':', "fullscreen", "false");
        assert_eq!(out, "fullscreen:false\r\nfov:0.5\r\n");
        let out = set_key_value("", '=', "OverrideWindow", "true");
        assert_eq!(out, "OverrideWindow=true\n");
    }

    #[test]
    fn missing_file_errors_unless_create_requested() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("options.txt");
        let ph = Placeholders::new();
        let mut s = spec(
            PatchFormat::KeyValueColon,
            file.to_str().unwrap(),
            &[("fullscreen", toml::Value::Boolean(false))],
        );
        assert!(matches!(
            apply_patch(&s, &file, &ph),
            Err(PatchError::Missing { .. })
        ));
        s.create_if_missing = true;
        apply_patch(&s, &file, &ph).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "fullscreen:false\n");
    }

    #[test]
    fn backups_restore_and_delete_created_files() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("a.txt");
        let created = dir.path().join("b.txt");
        fs::write(&existing, "orig").unwrap();
        let mut set = BackupSet::default();
        set.backup(&existing, &dir.path().join("bak")).unwrap();
        set.backup(&created, &dir.path().join("bak")).unwrap();
        set.backup(&existing, &dir.path().join("bak")).unwrap(); // idempotent
        assert_eq!(set.entries.len(), 2);
        fs::write(&existing, "patched").unwrap();
        fs::write(&created, "new").unwrap();
        let manifest = dir.path().join("backups.json");
        set.save(&manifest).unwrap();
        let loaded = BackupSet::load(&manifest).unwrap();
        assert!(loaded.restore_all().is_empty());
        assert_eq!(fs::read_to_string(&existing).unwrap(), "orig");
        assert!(!created.exists());
    }
}
