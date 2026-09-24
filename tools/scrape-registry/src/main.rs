// adds registry.nix entries for programs home-manager configures, Void packages, and the registry lacks
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

// a program's config files, main first, as registry path specs
type Programs = BTreeMap<String, Vec<String>>;

// modules whose literal paths aren't the program's own config, and why
const SKIP: [(&str, &str); 2] = [
    ("carapace", "writes completion files into other shells' dirs"),
    ("neovim", "its literal paths are plugin files; init.lua is interpolated"),
];

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let [home_manager, registry] = args.as_slice() else {
        eprintln!("usage: scrape-registry <home-manager checkout> <registry.nix>");
        std::process::exit(2);
    };
    if let Err(error) = run(Path::new(home_manager), Path::new(registry)) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

// scrapes, filters, and appends; prints the names it added
fn run(home_manager: &Path, registry: &Path) -> Result<(), String> {
    let programs = scan(&home_manager.join("modules/programs"));
    let existing = registry_names(registry)?;
    let packaged = void_packages()?;

    let added = additions(&programs, &existing, &packaged);
    if added.is_empty() {
        println!("nothing to add");
        return Ok(());
    }
    let text = fs::read_to_string(registry).map_err(|error| format!("{}: {error}", registry.display()))?;
    fs::write(registry, append(&text, &added)).map_err(|error| format!("{}: {error}", registry.display()))?;
    added.keys().for_each(|name| println!("add {name}"));
    Ok(())
}

// every module under dir: <name>.nix files and <name>/ dirs, with the literal config paths they write
fn scan(dir: &Path) -> Programs {
    let entries = fs::read_dir(dir).into_iter().flatten().filter_map(|entry| entry.ok().map(|entry| entry.path()));
    entries
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let text: String = nix_files(&path).iter().filter_map(|file| fs::read_to_string(file).ok()).collect();
            let specs = main_first(&name, specs(&text));
            (!specs.is_empty() && !SKIP.iter().any(|(skipped, _)| *skipped == name)).then_some((name, specs))
        })
        .collect()
}

// a module's .nix files: itself, or everything in its dir
fn nix_files(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        let files = fs::read_dir(path).into_iter().flatten().filter_map(|entry| entry.ok().map(|entry| entry.path()));
        files.filter(|file| file.extension().is_some_and(|ext| ext == "nix")).collect()
    } else if path.extension().is_some_and(|ext| ext == "nix") {
        vec![path.to_path_buf()]
    } else {
        Vec::new()
    }
}

// literal destinations in module text, first occurrence first: configFile."x" under ~/.config, home.file."x" under ~/
fn specs(text: &str) -> Vec<String> {
    let found = |marker: &str, prefix: &str| -> Vec<(usize, String)> {
        text.match_indices(marker)
            .filter_map(|(at, _)| {
                let rest = &text[at + marker.len()..];
                let path = &rest[..rest.find('"')?];
                // interpolated paths need evaluation, and Library/ is macOS
                let usable = !path.contains("${") && !path.starts_with("Library/") && !path.is_empty();
                usable.then(|| (at, format!("{prefix}{path}")))
            })
            .collect()
    };

    let mut all: Vec<(usize, String)> = found("configFile.\"", "").into_iter().chain(found("home.file.\"", "~/")).collect();
    all.sort();
    let mut seen = HashSet::new();
    all.into_iter().map(|(_, spec)| spec).filter(|spec| seen.insert(spec.clone())).collect()
}

// moves the file most likely to be the program's own config to the front: named config, settings, init, <name>rc, or after the program
fn main_first(name: &str, mut specs: Vec<String>) -> Vec<String> {
    let name = name.to_lowercase();
    let looks_main = |spec: &String| {
        let file = Path::new(spec).file_name().map(|file| file.to_string_lossy().trim_start_matches('.').to_lowercase()).unwrap_or_default();
        let stem = file.split('.').next().unwrap_or_default().to_string();
        ["config", "settings", "init"].contains(&stem.as_str()) || stem == format!("{name}rc") || stem.contains(&name)
    };
    if let Some(index) = specs.iter().position(looks_main) {
        let main = specs.remove(index);
        specs.insert(0, main);
    }
    specs
}

// names the registry already has, from nix itself; maw never parses nix
fn registry_names(registry: &Path) -> Result<HashSet<String>, String> {
    let output = Command::new("nix-instantiate")
        .args(["--eval", "--strict", "--json"])
        .arg(registry)
        .output()
        .map_err(|error| format!("nix-instantiate: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    Ok(value.as_object().into_iter().flatten().map(|(name, _)| name.to_lowercase()).collect())
}

// every package name in the Void repos, lowercased
fn void_packages() -> Result<HashSet<String>, String> {
    let output = Command::new("xbps-query").args(["-Rs", ""]).output().map_err(|error| format!("xbps-query: {error}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).lines().filter_map(package_name).collect())
}

// "[-] foot-1.28.0_1   description" -> foot
fn package_name(line: &str) -> Option<String> {
    let pkgver = line.split_whitespace().nth(1)?;
    Some(pkgver.rsplit_once('-')?.0.to_lowercase())
}

// scraped programs Void packages and the registry doesn't have yet
fn additions(programs: &Programs, existing: &HashSet<String>, packaged: &HashSet<String>) -> Programs {
    let wanted = |name: &String| packaged.contains(&name.to_lowercase()) && !existing.contains(&name.to_lowercase());
    programs.iter().filter(|(name, _)| wanted(name)).map(|(name, specs)| (name.clone(), specs.clone())).collect()
}

// the format maw would render a file in, by extension; none means raw
fn format_of(spec: &str) -> Option<&'static str> {
    let extension = Path::new(spec).extension()?.to_str()?;
    [("ini", "ini"), ("toml", "toml"), ("json", "json"), ("jsonc", "json"), ("kdl", "kdl"), ("css", "css")]
        .into_iter()
        .find_map(|(ext, format)| (ext == extension).then_some(format))
}

// roles for a program's files: main first, then each file's stem, numbered when two collide
fn roles(specs: &[String]) -> Vec<(String, &String)> {
    let mut taken = HashSet::new();
    specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let stem = Path::new(spec).file_stem().map(|stem| stem.to_string_lossy().trim_start_matches('.').to_lowercase()).unwrap_or_default();
            let base = if index == 0 || stem.is_empty() || stem == "main" { "main".to_string() } else { stem };
            let role = std::iter::once(base.clone()).chain((2..).map(|n| format!("{base}{n}"))).find(|role| !taken.contains(role)).unwrap();
            taken.insert(role.clone());
            (role, spec)
        })
        .collect()
}

// one registry entry in registry.nix's style
fn entry(name: &str, specs: &[String]) -> String {
    let format = format_of(&specs[0]).map(|format| format!("    format = \"{format}\";\n")).unwrap_or_default();
    let roles = roles(specs);
    let files = match roles.as_slice() {
        [(_, spec)] => format!("    files.main = \"{spec}\";\n"),
        _ => format!("    files = {{\n{}    }};\n", roles.iter().map(|(role, spec)| format!("      {} = \"{spec}\";\n", attr(role))).collect::<String>()),
    };
    format!("  {} = {{\n{format}{files}  }};\n", attr(name))
}

// nix attribute names are bare when nix allows it
fn attr(name: &str) -> String {
    let bare = name.chars().next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && name.chars().all(|char| char.is_ascii_alphanumeric() || "_'-".contains(char));
    if bare { name.to_string() } else { format!("\"{name}\"") }
}

// the registry with new entries before its closing brace, under a marker the first time
fn append(text: &str, added: &Programs) -> String {
    let marker = "  # from home-manager by tools/scrape-registry; edit or move freely\n";
    let close = text.rfind('}').unwrap_or(text.len());
    let (head, tail) = text.split_at(close);
    let header = if head.contains(marker) { "" } else { marker };
    let entries: String = added.iter().map(|(name, specs)| entry(name, specs)).collect();
    format!("{head}{header}{entries}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn specs_keep_literal_paths_in_order() {
        let text = r#"
            xdg.configFile."waybar/config".source = x;
            xdg.configFile."waybar/style.css".text = y;
            home.file.".tmate.conf".text = z;
            xdg.configFile."${cfg.dir}/x" = w;
            home.file."Library/Application Support/x/y.toml" = v;
            xdg.configFile."waybar/config".source = again;
        "#;
        assert_eq!(specs(text), ["waybar/config", "waybar/style.css", "~/.tmate.conf"]);
    }

    #[test]
    fn config_like_files_become_main() {
        let order = |name: &str, specs: &[&str]| main_first(name, specs.iter().map(|spec| spec.to_string()).collect());
        assert_eq!(order("feh", &["feh/buttons", "feh/keys", "feh/themes"])[0], "feh/buttons");
        assert_eq!(order("mpvpaper", &["mpvpaper/pauselist", "mpvpaper/config"])[0], "mpvpaper/config");
        assert_eq!(order("offlineimap", &["offlineimap/get_settings.py", "offlineimap/offlineimaprc"])[0], "offlineimap/offlineimaprc");
        assert_eq!(order("screen", &["~/.screenrc"])[0], "~/.screenrc");
    }

    #[test]
    fn skipped_modules_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("carapace.nix"), r#"xdg.configFile."fish/completions/x.fish" = x;"#);
        assert!(scan(dir.path()).is_empty());
    }

    #[test]
    fn scan_reads_files_and_module_dirs() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("foot.nix"), r#"xdg.configFile."foot/foot.ini" = x;"#);
        write(&dir.path().join("aerc/default.nix"), r#"xdg.configFile."aerc/aerc.conf" = x;"#);
        write(&dir.path().join("templated.nix"), r#"xdg.configFile."${dir}/x" = x;"#);
        let programs = scan(dir.path());
        assert_eq!(programs.keys().collect::<Vec<_>>(), ["aerc", "foot"]);
    }

    #[test]
    fn only_packaged_programs_the_registry_lacks_are_added() {
        let programs: Programs = [("foot", "foot/foot.ini"), ("Kitty", "kitty/kitty.conf"), ("aerospace", "aerospace/x.toml")]
            .into_iter()
            .map(|(name, spec)| (name.to_string(), vec![spec.to_string()]))
            .collect();
        let existing: HashSet<String> = ["foot".to_string()].into();
        let packaged: HashSet<String> = ["foot".to_string(), "kitty".to_string()].into();
        assert_eq!(additions(&programs, &existing, &packaged).keys().collect::<Vec<_>>(), ["Kitty"]);
    }

    #[test]
    fn entries_match_the_registry_style() {
        assert_eq!(entry("foot", &["foot/foot.ini".into()]), "  foot = {\n    format = \"ini\";\n    files.main = \"foot/foot.ini\";\n  };\n");
        let waybar = entry("waybar", &["waybar/config".into(), "waybar/style.css".into()]);
        assert_eq!(waybar, "  waybar = {\n    files = {\n      main = \"waybar/config\";\n      style = \"waybar/style.css\";\n    };\n  };\n");
        assert_eq!(roles(&["a/config".into(), "a/x/config".into(), "a/y/config".into()]).iter().map(|(role, _)| role.as_str()).collect::<Vec<_>>(), ["main", "config", "config2"]);
    }

    #[test]
    fn append_adds_a_marker_once() {
        let added: Programs = [("kitty".to_string(), vec!["kitty/kitty.conf".to_string()])].into();
        let once = append("{\n  foot.dir = \"x\";\n}\n", &added);
        assert!(once.ends_with("  # from home-manager by tools/scrape-registry; edit or move freely\n  kitty = {\n    files.main = \"kitty/kitty.conf\";\n  };\n}\n"));
        let more: Programs = [("mpv".to_string(), vec!["mpv/mpv.conf".to_string()])].into();
        assert_eq!(append(&once, &more).matches("# from home-manager").count(), 1);
    }

    #[test]
    fn repo_lines_give_package_names() {
        assert_eq!(package_name("[-] alacritty-terminfo-0.17.0_1  terminfo").as_deref(), Some("alacritty-terminfo"));
        assert_eq!(package_name("[*] Waybar-0.15.0_4  bar").as_deref(), Some("waybar"));
    }
}
