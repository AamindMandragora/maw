use maw::scaffold::xbps_src::XbpsSrc;
use maw::scaffold::{Deps, Meta, ScaffoldError, SourceEmitter, SourcePkg, aur};
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

// the shipped depmap against a fixed list of void names
fn deps() -> Deps {
    let known = fs::read_to_string("tests/scaffold/void-names.txt").unwrap().lines().map(String::from).collect();
    Deps::new(depmap(), known)
}

// a template against its golden, or rewriting the golden under MAW_BLESS
fn check(golden: &str, text: &str) {
    let golden = Path::new("tests/scaffold").join(golden);
    if std::env::var_os("MAW_BLESS").is_some() {
        fs::write(&golden, text).unwrap();
    }
    assert_eq!(text, fs::read_to_string(&golden).unwrap(), "{}", golden.display());
}

// real aur metadata and PKGBUILDs for a meson, a rust, a go, and a prebuilt package
#[test]
fn aur_templates_match_goldens() {
    let deps = deps();
    ["wlogout", "paru", "yay", "yay-bin"].iter().for_each(|name| {
        let info: serde_json::Value = serde_json::from_str(&fs::read_to_string(format!("tests/scaffold/{name}.aur.json")).unwrap()).unwrap();
        let pkgbuild = fs::read_to_string(format!("tests/scaffold/{name}.PKGBUILD")).unwrap();
        let pkg = aur::source_pkg(&info, &pkgbuild, name.trim_end_matches("-bin")).unwrap();
        check(&format!("{name}.aur.template"), &XbpsSrc.emit(&pkg, &deps, "Maw Test <maw@test>", "CHECKSUM").text);
    });
}

// a -git package is a checkout, which xbps-src can't build from
#[test]
fn aur_checkouts_are_refused() {
    let info: serde_json::Value = serde_json::from_str(&fs::read_to_string("tests/scaffold/paru-git.aur.json").unwrap()).unwrap();
    let pkgbuild = fs::read_to_string("tests/scaffold/paru-git.PKGBUILD").unwrap();
    assert!(matches!(aur::source_pkg(&info, &pkgbuild, "paru"), Err(ScaffoldError::Vcs(_))));
}

// real nixpkgs metadata for a go, a rust, and a meson package, scaffolded against a fixed list of void names
#[test]
fn templates_match_goldens() {
    let deps = deps();

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
