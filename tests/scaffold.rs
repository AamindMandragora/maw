use maw::scaffold::xbps_src::XbpsSrc;
use maw::scaffold::{Deps, Meta, SourceEmitter, SourcePkg};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

// the shipped depmap, through nix like maw reads it
fn depmap() -> BTreeMap<String, Option<String>> {
    let output = Command::new("nix-instantiate").args(["--eval", "--strict", "--json", "depmap.nix"]).output().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    value.as_object().unwrap().iter().map(|(name, mapped)| (name.clone(), mapped.as_str().map(String::from))).collect()
}

// real nixpkgs metadata for a go, a rust, and a meson package, scaffolded against a fixed list of void names
#[test]
fn templates_match_goldens() {
    let known = fs::read_to_string("tests/scaffold/void-names.txt").unwrap().lines().map(String::from).collect();
    let deps = Deps::new(depmap(), known);

    ["lazygit", "ripgrep", "fuzzel"].iter().for_each(|name| {
        let meta: Meta = serde_json::from_str(&fs::read_to_string(format!("tests/scaffold/{name}.json")).unwrap()).unwrap();
        let pkg = SourcePkg::from_meta(&meta, name, &format!("nixpkgs '{name}' at test")).unwrap();
        let emitted = XbpsSrc.emit(&pkg, &deps, "Maw Test <maw@test>", "CHECKSUM");

        let golden = Path::new("tests/scaffold").join(format!("{name}.template"));
        if std::env::var_os("MAW_BLESS").is_some() {
            fs::write(&golden, &emitted.text).unwrap();
        }
        assert_eq!(emitted.text, fs::read_to_string(&golden).unwrap(), "{name}");
    });
}
