use crate::activate::{self, ActivateError};
use crate::build::{self, BuildError};
use crate::env::Env;
use crate::inputs::{Inputs, InputsError};
use crate::registry::{self, Registry};
use crate::repo::Repo;
use crate::runner::{RunError, Runner};
use crate::state::{MawState, PathAnswer, StateError, attr_name, string};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("no module or static files named {0}; create one with `maw new {0}`")]
    NoSuch(String),
    #[error("{0} already exists")]
    Exists(String),
    #[error("unknown format {0}; one of: {formats}", formats = FORMATS.join(", "))]
    Format(String),
    #[error("{0} is already managed by maw")]
    Managed(String),
    #[error(transparent)]
    Activate(#[from] ActivateError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error(transparent)]
    Inputs(#[from] InputsError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> EditError + '_ {
    move |source| EditError::Io { path: path.into(), source }
}

pub const FORMATS: [&str; 8] = ["css", "ini", "json", "kdl", "keyValue", "raw", "shell", "toml"];

// one file a new module will hold: its role, format, and the live content to import, if any
struct Seed {
    key: String,
    format: String,
    live: Option<(PathBuf, String)>,
}

// one file `maw add` copies, and where its copy goes
struct Added {
    file: PathBuf,
    copy: PathBuf,
    // the registry name and key, or None for a loose file at the top of static/
    placed: Option<(String, String)>,
}

// what `maw edit <name>` opens: config.nix, the module, or the static dir
pub fn edit_target(repo: &Repo, name: &str) -> Result<PathBuf, EditError> {
    let candidates = [repo.config_file(), repo.module_file(name), repo.static_dir().join(name)];
    let found = match name {
        "config" => Some(&candidates[0]),
        _ => candidates[1..].iter().find(|path| path.exists()),
    };
    found.cloned().ok_or_else(|| EditError::NoSuch(name.into()))
}

// opens a path in the editor, which may carry its own args like "code -w"
pub fn open_editor(runner: &dyn Runner, editor: &str, path: &Path) -> Result<(), EditError> {
    let mut words = editor.split_whitespace().map(String::from);
    let program = words.next().unwrap_or_else(|| "vi".into());
    let args: Vec<String> = words.chain([path.display().to_string()]).collect();
    Ok(runner.interactive(&program, &args)?)
}

// the registry and maw.nix state, stamping inputs along the way
pub fn load(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<(Registry, MawState), EditError> {
    let inputs_file = env.state_dir.join("inputs");
    let mut inputs = Inputs::load(&inputs_file)?;
    let loaded = build::load_registry(env, runner, repo, &mut inputs)?;
    inputs.save(&inputs_file)?;
    Ok(loaded)
}

// writes modules/<name>.nix for every file the registry knows, importing live files as lib.raw
pub fn new_module(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str, format: Option<&str>) -> Result<PathBuf, EditError> {
    let module = repo.module_file(name);
    if module.exists() {
        return Err(EditError::Exists(env.pretty(&module)));
    }
    if let Some(unknown) = format.filter(|format| !FORMATS.contains(format)) {
        return Err(EditError::Format(unknown.into()));
    }

    let (registry, _) = load(env, runner, repo)?;
    let entry = registry.entry(name);
    let default_format = format.map(String::from).or(entry.format.clone()).unwrap_or_else(|| "raw".into());
    let keys: Vec<String> = if entry.files.is_empty() { vec!["main".into()] } else { entry.files.keys().cloned().collect() };

    // several files each take the format their extension suggests
    let seeds: Vec<Seed> = keys
        .iter()
        .map(|key| {
            let spec = registry.spec(name, key);
            let format = if keys.len() > 1 { format_of(&spec).unwrap_or(&default_format).to_string() } else { default_format.clone() };
            let live = live_content(repo, &registry::resolve(env, &spec));
            Seed { key: key.clone(), format, live }
        })
        .collect();

    fs::write(&module, scaffold(env, name, &seeds)).map_err(io(&module))?;
    Ok(module)
}

// the format a destination's extension implies
fn format_of(spec: &str) -> Option<&'static str> {
    let extension = Path::new(spec).extension()?.to_str()?;
    [("css", "css"), ("json", "json"), ("jsonc", "json"), ("kdl", "kdl"), ("ini", "ini"), ("toml", "toml")]
        .into_iter()
        .find_map(|(ext, format)| (ext == extension).then_some(format))
}

// a live file's content, unless it's missing, unreadable, or already linked into the repo
fn live_content(repo: &Repo, path: &Path) -> Option<(PathBuf, String)> {
    if is_managed(repo, path) {
        return None;
    }
    Some((path.into(), fs::read_to_string(path).ok()?))
}

fn is_managed(repo: &Repo, path: &Path) -> bool {
    fs::read_link(path).is_ok_and(|target| target.starts_with(&repo.root))
}

// module text: one settings, or a files set when there are several
fn scaffold(env: &Env, name: &str, seeds: &[Seed]) -> String {
    let header = format!("{{ config, lib, ... }}:\nlib.program {} {{\n", string(name));
    if let [only] = seeds {
        return format!("{header}  format = {};\n{}}}\n", string(&only.format), settings(env, "settings", only, 2));
    }

    // one shared format when every file agrees, else one per file
    let same = seeds.iter().all(|seed| seed.format == seeds[0].format);
    let format = if same {
        string(&seeds[0].format)
    } else {
        let entries: Vec<String> = seeds.iter().map(|seed| format!("{} = {};", attr_name(&seed.key), string(&seed.format))).collect();
        format!("{{ {} }}", entries.join(" "))
    };
    let files: String = seeds.iter().map(|seed| settings(env, &attr_name(&seed.key), seed, 4)).collect();
    format!("{header}  format = {format};\n  files = {{\n{files}  }};\n}}\n")
}

// `<label> = <value>;` at an indent: the imported file as lib.raw, else an empty value
fn settings(env: &Env, label: &str, seed: &Seed, indent: usize) -> String {
    let pad = " ".repeat(indent);
    match &seed.live {
        Some((path, content)) => format!("{pad}# imported from {}\n{pad}{label} = lib.raw {};\n", env.pretty(path), indented_string(content, indent)),
        None if seed.format == "raw" => format!("{pad}{label} = \"\";\n"),
        None => format!("{pad}{label} = {{ }};\n"),
    }
}

// a nix '' string holding text exactly, its lines indented two past the closing quotes
fn indented_string(text: &str, indent: usize) -> String {
    let escaped = text.replace("''", "'''").replace("${", "''${");
    let pad = " ".repeat(indent + 2);
    let lines: Vec<String> = escaped.lines().map(|line| if line.is_empty() { String::new() } else { format!("{pad}{line}") }).collect();

    // a trailing newline puts the closing quotes on their own line
    let close = if text.ends_with('\n') { format!("\n{}''", " ".repeat(indent)) } else { "''".into() };
    format!("''\n{}{close}", lines.join("\n"))
}

// copies a file, or every file in a dir, into static/ so each lands back where it came from; returns (file, copy) pairs
pub fn add(env: &Env, runner: &dyn Runner, repo: &Repo, path: &Path, name: Option<&str>) -> Result<Vec<(PathBuf, PathBuf)>, EditError> {
    let path = std::path::absolute(path).map_err(io(path))?;
    if is_managed(repo, &path) {
        return Err(EditError::Managed(env.pretty(&path)));
    }
    let (registry, mut state) = load(env, runner, repo)?;

    let files = if path.is_dir() { activate::walk(&path)? } else { vec![path.clone()] };
    let added: Vec<Added> = files.iter().map(|file| added(env, repo, &registry, &path, file, name)).collect();
    if let Some(taken) = added.iter().find(|added| added.copy.exists()) {
        return Err(EditError::Exists(env.pretty(&taken.copy)));
    }

    // copy, then record an answer wherever the registry alone would put the copy somewhere else
    let answers = added
        .iter()
        .map(|added| {
            fs::create_dir_all(added.copy.parent().unwrap()).map_err(io(&added.copy))?;
            fs::copy(&added.file, &added.copy).map_err(io(&added.file))?;
            Ok((destination(env, &registry, added) != Some(added.file.clone())).then(|| answer(env, added)))
        })
        .collect::<Result<Vec<_>, EditError>>()?;

    let answers: Vec<(String, String, String)> = answers.into_iter().flatten().collect();
    if !answers.is_empty() {
        answers.into_iter().for_each(|(name, key, spec)| record(&mut state, name, key, spec));
        state.write(&repo.maw_file())?;
    }
    Ok(added.into_iter().map(|added| (added.file, added.copy)).collect())
}

// where one added file goes in static/: explicit name, registry, ~/.config/<name>/, or loose
fn added(env: &Env, repo: &Repo, registry: &Registry, root: &Path, file: &Path, name: Option<&str>) -> Added {
    let file_name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let relative = file.strip_prefix(root).ok().filter(|relative| !relative.as_os_str().is_empty());
    let under_config = || {
        let mut parts = file.strip_prefix(env.home.join(".config")).ok()?.components();
        let name = parts.next()?.as_os_str().to_string_lossy().into_owned();
        let key = parts.as_path().to_string_lossy().into_owned();
        (!key.is_empty()).then_some((name, key))
    };

    let placed = match name {
        Some(name) => Some((name.into(), relative.map_or(file_name.clone(), |relative| relative.to_string_lossy().into_owned()))),
        None => registry.locate(env, file).or_else(under_config),
    };
    let copy = match &placed {
        Some((name, key)) => repo.static_dir().join(name).join(key),
        None => repo.static_dir().join(&file_name),
    };
    Added { file: file.into(), copy, placed }
}

// where activation would put a copy without any new answer; None if it would have to ask
fn destination(env: &Env, registry: &Registry, added: &Added) -> Option<PathBuf> {
    let file_name = added.copy.file_name()?.to_string_lossy().into_owned();
    let (name, key) = match &added.placed {
        Some(placed) => placed.clone(),
        None => (activate::category(&added.file)?.to_string(), file_name),
    };
    Some(registry::resolve(env, &registry.spec(&name, &key)))
}

// (name, key, spec) that sends an added file back to its original path
fn answer(env: &Env, added: &Added) -> (String, String, String) {
    let file_name = added.copy.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let (name, key) = added.placed.clone().unwrap_or((file_name.clone(), file_name));
    let spec = match added.file.strip_prefix(&env.home) {
        Ok(relative) => format!("~/{}", relative.display()),
        Err(_) => Path::new("/").join(added.file.strip_prefix(&env.sysroot).unwrap_or(&added.file)).display().to_string(),
    };
    (name, key, spec)
}

// adds one per-file answer to maw.nix paths, replacing a dir answer for that name
fn record(state: &mut MawState, name: String, key: String, spec: String) {
    let mut files = match state.paths.remove(&name) {
        Some(PathAnswer::Files(files)) => files,
        _ => BTreeMap::new(),
    };
    files.insert(key, spec);
    state.paths.insert(name, PathAnswer::Files(files));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{fixture, write};
    use std::os::unix::fs::symlink;

    #[test]
    fn edit_target_prefers_config_then_module_then_static() {
        let fixture = fixture(&["foot"]);
        write(&fixture.repo.static_dir().join("nvim/init.lua"), "");
        assert_eq!(edit_target(&fixture.repo, "config").unwrap(), fixture.repo.config_file());
        assert_eq!(edit_target(&fixture.repo, "foot").unwrap(), fixture.repo.module_file("foot"));
        assert_eq!(edit_target(&fixture.repo, "nvim").unwrap(), fixture.repo.static_dir().join("nvim"));
        assert!(matches!(edit_target(&fixture.repo, "nope"), Err(EditError::NoSuch(_))));
    }

    #[test]
    fn editor_keeps_its_own_args() {
        let fixture = fixture(&[]);
        open_editor(&fixture.runner, "code -w", Path::new("/dots/modules/foot.nix")).unwrap();
        assert_eq!(fixture.runner.calls.borrow()[0], "code -w /dots/modules/foot.nix");
    }

    #[test]
    fn new_module_uses_the_registry_format() {
        let fixture = fixture(&[]);
        let module = new_module(&fixture.env, &fixture.runner, &fixture.repo, "foot", None).unwrap();
        let expected = "{ config, lib, ... }:\nlib.program \"foot\" {\n  format = \"ini\";\n  settings = { };\n}\n";
        assert_eq!(fs::read_to_string(module).unwrap(), expected);
    }

    #[test]
    fn new_module_imports_live_files_per_format() {
        let fixture = fixture(&[]);
        write(&fixture.env.home.join(".config/waybar/style.css"), "window { color: red; }\n");
        let module = new_module(&fixture.env, &fixture.runner, &fixture.repo, "waybar", None).unwrap();

        let text = fs::read_to_string(module).unwrap();
        assert!(text.contains("format = { main = \"json\"; style = \"css\"; };"));
        assert!(text.contains("    main = { };\n"));
        assert!(text.contains("    # imported from ~/.config/waybar/style.css\n    style = lib.raw ''\n      window { color: red; }\n    '';"));
    }

    #[test]
    fn new_module_refuses_bad_format_and_existing_module() {
        let fixture = fixture(&["foot"]);
        assert!(matches!(new_module(&fixture.env, &fixture.runner, &fixture.repo, "x", Some("yaml")), Err(EditError::Format(_))));
        assert!(matches!(new_module(&fixture.env, &fixture.runner, &fixture.repo, "foot", None), Err(EditError::Exists(_))));
    }

    #[test]
    fn indented_string_escapes_and_keeps_the_last_newline() {
        assert_eq!(indented_string("a ''b'' ${c}\n\nd\n", 2), "''\n    a '''b''' ''${c}\n\n    d\n  ''");
        assert_eq!(indented_string("no newline", 0), "''\n  no newline''");
    }

    #[test]
    fn add_files_under_config_by_program_name() {
        let fixture = fixture(&[]);
        let file = fixture.env.home.join(".config/nvim/lua/keys.lua");
        write(&file, "-- keys");

        let copies = add(&fixture.env, &fixture.runner, &fixture.repo, &fixture.env.home.join(".config/nvim"), None).unwrap();
        assert_eq!(copies, [(file, fixture.repo.static_dir().join("nvim/lua/keys.lua"))]);
        assert!(!fs::read_to_string(fixture.repo.maw_file()).unwrap().contains("nvim"));
    }

    #[test]
    fn add_uses_registry_destinations() {
        let fixture = fixture(&[]);
        write(&fixture.env.home.join(".bashrc"), "alias ll='ls -l'");
        let copies = add(&fixture.env, &fixture.runner, &fixture.repo, &fixture.env.home.join(".bashrc"), None).unwrap();
        assert_eq!(copies[0].1, fixture.repo.static_dir().join("bash/.bashrc"));
    }

    #[test]
    fn add_records_where_an_unknown_file_came_from() {
        let fixture = fixture(&[]);
        write(&fixture.env.home.join("notes.txt"), "hi");
        let copies = add(&fixture.env, &fixture.runner, &fixture.repo, &fixture.env.home.join("notes.txt"), None).unwrap();

        assert_eq!(copies[0].1, fixture.repo.static_dir().join("notes.txt"));
        assert!(fs::read_to_string(fixture.repo.maw_file()).unwrap().contains(r#""notes.txt" = { "notes.txt" = "~/notes.txt"; };"#));
    }

    #[test]
    fn add_with_a_name_records_the_original_path() {
        let fixture = fixture(&[]);
        write(&fixture.env.home.join(".tmux.conf"), "set -g mouse on");
        add(&fixture.env, &fixture.runner, &fixture.repo, &fixture.env.home.join(".tmux.conf"), Some("tmux")).unwrap();

        assert!(fixture.repo.static_dir().join("tmux/.tmux.conf").is_file());
        assert!(fs::read_to_string(fixture.repo.maw_file()).unwrap().contains(r#"tmux = { ".tmux.conf" = "~/.tmux.conf"; };"#));
    }

    #[test]
    fn add_refuses_managed_and_existing_files() {
        let fixture = fixture(&[]);
        let copy = fixture.repo.static_dir().join("nvim/init.lua");
        write(&copy, "");
        let live = fixture.env.home.join(".config/nvim/init.lua");
        write(&live, "");
        assert!(matches!(add(&fixture.env, &fixture.runner, &fixture.repo, &live, None), Err(EditError::Exists(_))));

        fs::remove_file(&live).unwrap();
        symlink(&copy, &live).unwrap();
        assert!(matches!(add(&fixture.env, &fixture.runner, &fixture.repo, &live, None), Err(EditError::Managed(_))));
    }
}
