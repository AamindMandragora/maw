use crate::env::Env;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("registry entry {name}: {source}")]
    Entry { name: String, source: serde_json::Error },
}

// where one program's files go; paths are specs, see resolve
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Entry {
    pub format: Option<String>,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    pub dir: Option<String>,
    #[serde(default)]
    pub root: bool,
    #[serde(default)]
    pub executable: bool,
}

pub struct Registry {
    entries: BTreeMap<String, Entry>,
}

impl Registry {
    // shipped entries, with the answers recorded in maw.nix paths layered on top
    pub fn new(shipped: &Value, answers: &Value) -> Result<Self, RegistryError> {
        // each shipped entry parsed on its own so errors name the entry
        let mut entries = shipped
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, entry)| {
                let parsed = serde_json::from_value(entry.clone()).map_err(|source| RegistryError::Entry { name: name.clone(), source })?;
                Ok((name.clone(), parsed))
            })
            .collect::<Result<BTreeMap<String, Entry>, _>>()?;

        answers.as_object().into_iter().flatten().for_each(|(name, answer)| apply_answer(entries.entry(name.clone()).or_default(), answer));
        Ok(Registry { entries })
    }

    // the entry for a name, or an empty one meaning ~/.config/<name>/
    pub fn entry(&self, name: &str) -> Entry {
        self.entries.get(name).cloned().unwrap_or_default()
    }

    // destination spec of one file: files.<key>, else <dir>/<key>, with main named config
    pub fn spec(&self, name: &str, key: &str) -> String {
        let entry = self.entry(name);
        entry.files.get(key).cloned().unwrap_or_else(|| {
            let dir = entry.dir.unwrap_or_else(|| name.into());
            format!("{dir}/{}", default_file_name(key))
        })
    }

    // path of one file under out/<name>/: the destination's file name, or the key itself
    pub fn out_name(&self, name: &str, key: &str) -> PathBuf {
        match self.entry(name).files.get(key) {
            Some(spec) => Path::new(spec).file_name().map(PathBuf::from).unwrap_or_else(|| key.into()),
            None => default_file_name(key).into(),
        }
    }
}

// a maw.nix answer is either a dir or a set of per-key paths
fn apply_answer(entry: &mut Entry, answer: &Value) {
    match answer {
        Value::String(dir) => entry.dir = Some(dir.clone()),
        Value::Object(files) => files.iter().filter_map(|(key, spec)| Some((key.clone(), spec.as_str()?.to_string()))).for_each(|(key, spec)| {
            entry.files.insert(key, spec);
        }),
        _ => {}
    }
}

fn default_file_name(key: &str) -> &str {
    if key == "main" { "config" } else { key }
}

// "~/x" is under home, "/x" under the system root, anything else under ~/.config
pub fn resolve(env: &Env, spec: &str) -> PathBuf {
    if let Some(rest) = spec.strip_prefix("~/") {
        env.home.join(rest)
    } else if let Some(rest) = spec.strip_prefix('/') {
        env.sysroot.join(rest)
    } else {
        env.home.join(".config").join(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn registry(answers: Value) -> Registry {
        let shipped = json!({
            "foot": { "format": "ini", "files": { "main": "foot/foot.ini" } },
            "bash": { "files": { "main": "~/.bashrc" } },
            "scripts": { "dir": "~/.local/bin", "executable": true },
            "greetd": { "files": { "main": "/etc/greetd/config.toml" }, "root": true },
        });
        Registry::new(&shipped, &answers).unwrap()
    }

    #[test]
    fn spec_uses_files_then_dir_then_name() {
        let registry = registry(json!({}));
        assert_eq!(registry.spec("foot", "main"), "foot/foot.ini");
        assert_eq!(registry.spec("scripts", "powermenu"), "~/.local/bin/powermenu");
        assert_eq!(registry.spec("unknown", "main"), "unknown/config");
        assert_eq!(registry.spec("unknown", "theme.conf"), "unknown/theme.conf");
    }

    #[test]
    fn answers_override_dir_and_files() {
        let registry = registry(json!({ "wallpapers": "~/Pictures/walls", "foot": { "main": "foot/other.ini" } }));
        assert_eq!(registry.spec("wallpapers", "a.png"), "~/Pictures/walls/a.png");
        assert_eq!(registry.spec("foot", "main"), "foot/other.ini");
        assert_eq!(registry.entry("foot").format.as_deref(), Some("ini"));
    }

    #[test]
    fn out_name_is_destination_file_name() {
        let registry = registry(json!({}));
        assert_eq!(registry.out_name("foot", "main"), PathBuf::from("foot.ini"));
        assert_eq!(registry.out_name("bash", "main"), PathBuf::from(".bashrc"));
        assert_eq!(registry.out_name("unknown", "main"), PathBuf::from("config"));
        assert_eq!(registry.out_name("unknown", "themes/dark"), PathBuf::from("themes/dark"));
    }

    #[test]
    fn resolve_handles_home_root_and_config() {
        let env = Env::new(Path::new("/h"), Path::new("/sys"), Path::new("/share"));
        assert_eq!(resolve(&env, "~/.bashrc"), PathBuf::from("/h/.bashrc"));
        assert_eq!(resolve(&env, "/etc/greetd/config.toml"), PathBuf::from("/sys/etc/greetd/config.toml"));
        assert_eq!(resolve(&env, "foot/foot.ini"), PathBuf::from("/h/.config/foot/foot.ini"));
    }

    #[test]
    fn bad_entry_names_itself() {
        let error = Registry::new(&json!({ "foot": { "root": "yes" } }), &json!({})).err().unwrap();
        assert!(error.to_string().contains("foot"));
    }
}
