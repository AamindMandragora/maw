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
    // packages and services only some machines have, by machine name
    #[serde(default)]
    pub hosts: BTreeMap<String, Lists>,
}

// one machine's own packages and services, on top of the shared ones
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Lists {
    #[serde(default)]
    pub packages: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub services: BTreeMap<String, Vec<String>>,
}

impl Lists {
    pub fn is_empty(&self) -> bool {
        self.packages.values().all(Vec::is_empty) && self.services.values().all(Vec::is_empty)
    }
}

// each key's items from both maps, once each
fn union(shared: &BTreeMap<String, Vec<String>>, own: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, Vec<String>> {
    own.iter().fold(shared.clone(), |mut merged, (key, items)| {
        let list = merged.entry(key.clone()).or_default();
        list.extend(items.iter().filter(|item| !list.contains(item)).cloned().collect::<Vec<_>>());
        merged
    })
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
    // what one machine declares: the shared lists plus its own, with no per-machine lists left
    pub fn here(&self, host: &str) -> MawState {
        let own = self.hosts.get(host).cloned().unwrap_or_default();
        MawState { packages: union(&self.packages, &own.packages), services: union(&self.services, &own.services), hosts: BTreeMap::new(), ..self.clone() }
    }

    // this machine's own lists, for writing to
    pub fn own(&mut self, host: &str) -> &mut Lists {
        self.hosts.entry(host.to_string()).or_default()
    }

    // removes a name from a shared list and this machine's own, packages or services, keyed by backend or scope
    pub fn forget(&mut self, host: &str, services: bool, key: &str, name: &str) {
        let drop = |lists: &mut BTreeMap<String, Vec<String>>| {
            if let Some(items) = lists.get_mut(key) {
                items.retain(|item| item != name);
            }
            lists.retain(|_, items| !items.is_empty());
        };
        drop(if services { &mut self.services } else { &mut self.packages });
        if let Some(own) = self.hosts.get_mut(host) {
            drop(if services { &mut own.services } else { &mut own.packages });
        }
        self.tidy();
    }

    // drops machines whose lists are empty, so maw.nix only names ones with something of their own
    pub fn tidy(&mut self) {
        self.hosts.values_mut().for_each(|lists| {
            lists.packages.retain(|_, items| !items.is_empty());
            lists.services.retain(|_, items| !items.is_empty());
        });
        self.hosts.retain(|_, lists| !lists.is_empty());
    }

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
        // per-machine lists, only for machines that have any
        let hosts: Vec<(&String, String)> = self
            .hosts
            .iter()
            .filter(|(_, own)| !own.is_empty())
            .map(|(name, own)| (name, format!("{{\n      packages = {};\n      services = {};\n    }}", lists(&own.packages, 4), lists(&own.services, 4))))
            .collect();
        let hosts = if hosts.is_empty() { String::new() } else { format!("  hosts = {};\n", attrs(hosts.into_iter(), 2)) };
        format!(
            "{{\n  packages = {};\n  services = {};\n  paths = {answers};\n{ignored}{hosts}}}\n",
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
    fn machines_add_their_own_lists_to_the_shared_ones() {
        let mut state = MawState::default();
        state.packages.insert("xbps".into(), vec!["foot".into()]);
        state.own("laptop").packages.insert("xbps".into(), vec!["tlp".into(), "foot".into()]);
        state.own("laptop").services.insert("system".into(), vec!["tlp".into()]);
        state.own("desktop");

        let laptop = state.here("laptop");
        assert_eq!((laptop.packages["xbps"].clone(), laptop.services["system"].clone()), (vec!["foot".to_string(), "tlp".into()], vec!["tlp".to_string()]));
        assert_eq!(state.here("desktop").packages["xbps"], ["foot"]);

        // written back, only machines with lists of their own appear, and they read back the same
        state.tidy();
        let nix = state.to_nix();
        assert!(nix.contains("  hosts = {\n    laptop = {\n      packages = {\n        xbps = [ \"foot\" \"tlp\" ];\n      };\n      services = {\n        system = [ \"tlp\" ];\n      };\n    };\n  };\n"), "{nix}");
        assert!(!nix.contains("desktop"));
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(string(r#"a"b\c${d}"#), r#""a\"b\\c\${d}""#);
        assert_eq!(attr_name("in"), "\"in\"");
        assert_eq!(attr_name("swaync-client"), "swaync-client");
    }
}
