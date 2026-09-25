use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("maw.nix: unexpected shape: {0}")]
    Shape(serde_json::Error),
    #[error("{path}")]
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
    #[serde(default)]
    pub ignored: Ignored,
}

// installed packages and enabled services maw.nix doesn't declare but was told to leave alone
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ignored {
    #[serde(default)]
    pub packages: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub services: BTreeMap<String, Vec<String>>,
}

impl Ignored {
    pub fn is_empty(&self) -> bool {
        self.packages.values().all(Vec::is_empty) && self.services.values().all(Vec::is_empty)
    }
}

impl MawState {
    // from the evaluated json of maw.nix
    pub fn from_value(value: &Value) -> Result<Self, StateError> {
        serde_json::from_value(value.clone()).map_err(StateError::Shape)
    }

    // the fixed-shape nix text: sorted, one entry per line; ignored only once something is
    pub fn to_nix(&self) -> String {
        let answers = attrs(self.paths.iter().map(|(name, answer)| (name, answer_nix(answer))), 2);
        let ignored = if self.ignored.is_empty() {
            String::new()
        } else {
            let (packages, services) = (lists(&self.ignored.packages, 3), lists(&self.ignored.services, 3));
            format!("  ignored = {{\n    packages = {packages};\n    services = {services};\n  }};\n")
        };
        format!(
            "{{\n  packages = {};\n  services = {};\n  paths = {answers};\n{ignored}}}\n",
            lists(&self.packages, 2),
            lists(&self.services, 2),
        )
    }

    pub fn write(&self, path: &Path) -> Result<(), StateError> {
        fs::write(path, self.to_nix()).map_err(|source| StateError::Io { path: path.into(), source })
    }
}

// a nested attrset, one entry per line, its entries at the given depth
fn attrs<'a>(entries: impl Iterator<Item = (&'a String, String)>, depth: usize) -> String {
    let pad = "  ".repeat(depth);
    let lines: Vec<String> = entries.map(|(name, value)| format!("{pad}{} = {value};\n", attr_name(name))).collect();
    if lines.is_empty() { "{ }".into() } else { format!("{{\n{}{}}}", lines.concat(), "  ".repeat(depth - 1)) }
}

// name -> list of strings, empty lists left out
fn lists(map: &BTreeMap<String, Vec<String>>, depth: usize) -> String {
    attrs(map.iter().filter(|(_, items)| !items.is_empty()).map(|(name, items)| (name, list(items))), depth)
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
pub fn string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"").replace("${", "\\${"))
}

// attribute names are bare when nix allows it, else quoted
pub fn attr_name(name: &str) -> String {
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
    fn ignored_is_written_only_when_used() {
        let mut state = MawState::default();
        state.ignored.packages.insert("xbps".into(), vec!["base-devel".into()]);
        state.ignored.services.insert("system".into(), vec!["agetty-tty3".into()]);
        let expected = "  ignored = {\n    packages = {\n      xbps = [ \"base-devel\" ];\n    };\n    services = {\n      system = [ \"agetty-tty3\" ];\n    };\n  };\n}\n";
        assert!(state.to_nix().ends_with(expected), "{}", state.to_nix());
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(string(r#"a"b\c${d}"#), r#""a\"b\\c\${d}""#);
        assert_eq!(attr_name("in"), "\"in\"");
        assert_eq!(attr_name("swaync-client"), "swaync-client");
    }
}
