use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("maw.nix: unexpected shape: {0}")]
    Shape(serde_json::Error),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

// a registry answer: a dir for every file, or a destination per file
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PathAnswer {
    Dir(String),
    Files(BTreeMap<String, String>),
}

// everything maw.nix holds; maw writes it, the user only reads it
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MawState {
    #[serde(default)]
    pub packages: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub services: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub paths: BTreeMap<String, PathAnswer>,
}

impl MawState {
    // from the evaluated json of maw.nix
    pub fn from_value(value: &Value) -> Result<Self, StateError> {
        serde_json::from_value(value.clone()).map_err(StateError::Shape)
    }

    // the fixed-shape nix text: sorted, one entry per line
    pub fn to_nix(&self) -> String {
        let lists = |map: &BTreeMap<String, Vec<String>>| attrs(map.iter().map(|(name, items)| (name, list(items))));
        let answers = attrs(self.paths.iter().map(|(name, answer)| (name, answer_nix(answer))));
        format!(
            "{{\n  packages = {};\n  services = {};\n  paths = {};\n}}\n",
            lists(&self.packages),
            lists(&self.services),
            answers
        )
    }

    pub fn write(&self, path: &Path) -> Result<(), StateError> {
        fs::write(path, self.to_nix()).map_err(|source| StateError::Io { path: path.into(), source })
    }
}

// a nested attrset, one entry per line, indented under a top-level key
fn attrs<'a>(entries: impl Iterator<Item = (&'a String, String)>) -> String {
    let lines: Vec<String> = entries.map(|(name, value)| format!("    {} = {value};\n", attr_name(name))).collect();
    if lines.is_empty() { "{ }".into() } else { format!("{{\n{}  }}", lines.concat()) }
}

fn list(items: &[String]) -> String {
    let mut sorted: Vec<String> = items.iter().map(|item| string(item)).collect();
    sorted.sort();
    if sorted.is_empty() { "[ ]".into() } else { format!("[ {} ]", sorted.join(" ")) }
}

// an answer inline: a string, or { key = "spec"; ... }
fn answer_nix(answer: &PathAnswer) -> String {
    match answer {
        PathAnswer::Dir(dir) => string(dir),
        PathAnswer::Files(files) => {
            let entries: Vec<String> = files.iter().map(|(key, spec)| format!("{} = {};", attr_name(key), string(spec))).collect();
            format!("{{ {} }}", entries.join(" "))
        }
    }
}

// a nix string literal, escaping quotes, backslashes, and interpolation
fn string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"").replace("${", "\\${"))
}

// attribute names are bare when nix allows it, else quoted
fn attr_name(name: &str) -> String {
    let mut chars = name.chars();
    let starts_ok = chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_');
    let rest_ok = chars.all(|char| char.is_ascii_alphanumeric() || "_'-".contains(char));
    let keyword = ["if", "then", "else", "let", "in", "with", "rec", "inherit", "assert", "or"].contains(&name);
    if starts_ok && rest_ok && !keyword { name.into() } else { string(name) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_state_matches_the_scaffold() {
        assert_eq!(MawState::default().to_nix(), "{\n  packages = { };\n  services = { };\n  paths = { };\n}\n");
    }

    #[test]
    fn entries_are_sorted_one_per_line() {
        let state = MawState::from_value(&json!({
            "packages": { "xbps": ["niri", "foot"], "cargo": ["bat"] },
            "paths": { "notes.txt": { "notes.txt": "~/notes.txt" }, "wallpapers": "~/Pictures" },
        }))
        .unwrap();

        let expected = r#"{
  packages = {
    cargo = [ "bat" ];
    xbps = [ "foot" "niri" ];
  };
  services = { };
  paths = {
    "notes.txt" = { "notes.txt" = "~/notes.txt"; };
    wallpapers = "~/Pictures";
  };
}
"#;
        assert_eq!(state.to_nix(), expected);
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(string(r#"a"b\c${d}"#), r#""a\"b\\c\${d}""#);
        assert_eq!(attr_name("in"), "\"in\"");
        assert_eq!(attr_name("swaync-client"), "swaync-client");
    }
}
