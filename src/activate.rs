use crate::backend::{self, BackendError};
use crate::backup;
use crate::build::{self, BuildError, Report};
pub use crate::build::Options;
use crate::env::Env;
use crate::init::Scope;
use crate::inputs::{Inputs, InputsError, load_json, save_json};
use crate::registry::{self, Registry};
use crate::packages::{self, PackagesError, Target};
use crate::repo::Repo;
use crate::runner::{RunError, Runner};
use crate::state::{MawState, PathAnswer, StateError};
use crate::system;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ActivateError {
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error(transparent)]
    Inputs(#[from] InputsError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("maw.nix declares packages for {0}, which maw has no backend for")]
    UnknownBackend(String),
    #[error(transparent)]
    Run(#[from] RunError),
    #[error(transparent)]
    Packages(Box<PackagesError>),
    #[error("no {scope} service {name}: nothing in its definition dir and no module defines it")]
    UnknownService { scope: Scope, name: String },
    #[error("{destination} comes from both {first} and {second}")]
    Conflict { destination: PathBuf, first: PathBuf, second: PathBuf },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ActivateError + '_ {
    move |source| ActivateError::Io { path: path.into(), source }
}

// asks the user something; None when nobody can answer
pub trait Ask {
    fn ask(&self, question: &str) -> Option<String>;
}

// one linked file as the last activation left it
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Linked {
    source: PathBuf,
    hash: String,
}

// what the last activation placed: links keyed by destination, plus root copies and services
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Manifest {
    files: BTreeMap<PathBuf, Linked>,
    #[serde(default)]
    system: system::Record,
}

// a file that should be in place: linked, or copied as root when root is set
#[derive(Debug, Clone)]
pub struct Wanted {
    pub destination: PathBuf,
    pub source: PathBuf,
    pub hash: String,
    pub root: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    // a declared package that isn't installed
    Install { backend: String, package: String },
    // backup: an existing file or foreign link is moved aside first
    Link { destination: PathBuf, backup: bool },
    // our link, pointed at a new source
    Relink { destination: PathBuf },
    // content changed behind an existing link; nothing to do on disk
    Update { destination: PathBuf },
    Unlink { destination: PathBuf },
    // the link was replaced by something else
    Replaced { destination: PathBuf },
    // the file was edited through its link
    Edited { destination: PathBuf },
    // a loose static/ file with no known destination
    Unplaced { file: PathBuf },
    // a root file written with sudo; backup: what's there now is saved first
    Copy { destination: PathBuf, backup: bool },
    // a root file maw copied that nothing declares anymore
    Delete { destination: PathBuf },
    Enable { scope: Scope, name: String },
    // stopped with sv down, then unlinked
    Disable { scope: Scope, name: String },
    // the definition dir of a service maw wrote and nothing declares anymore, supervise state and all
    Purge { scope: Scope, name: String },
    // a running service whose files changed
    Restart { scope: Scope, name: String },
}

#[derive(Debug, Default)]
pub struct Activation {
    pub build: Report,
    pub steps: Vec<Step>,
    // (file, where it was moved)
    pub backups: Vec<(PathBuf, PathBuf)>,
    // loose static/ files the user placed, recorded in maw.nix
    pub answered: Vec<String>,
}

// everything an activation would do, before any link is touched
pub struct Planned {
    pub build: Report,
    pub wanted: Vec<Wanted>,
    pub copies: Vec<Wanted>,
    pub steps: Vec<Step>,
    manifest: Manifest,
    next: Manifest,
}

// builds, plans links against the manifest and live files, and applies the plan unless dry_run
pub fn activate(env: &Env, runner: &dyn Runner, repo: &Repo, ask: &dyn Ask, options: Options) -> Result<Activation, ActivateError> {
    let answered = if options.dry_run { Vec::new() } else { place_loose(env, runner, repo, ask)? };
    let planned = plan_activation(env, runner, repo, options)?;
    if options.dry_run {
        return Ok(Activation { build: planned.build, steps: planned.steps, backups: Vec::new(), answered });
    }

    install_missing(env, runner, repo, &planned.steps)?;
    let mut backups = apply(env, &planned.steps, &planned.wanted)?;
    backups.extend(system::apply(env, runner, &planned.steps, &planned.copies)?);
    if planned.next != planned.manifest {
        save_json(&env.state_dir.join("manifest"), &planned.next)?;
    }
    Ok(Activation { build: planned.build, steps: planned.steps, backups, answered })
}

// builds (or with dry_run only renders), then plans every link without touching one
pub fn plan_activation(env: &Env, runner: &dyn Runner, repo: &Repo, options: Options) -> Result<Planned, ActivateError> {
    let build = build::build(env, runner, repo, options)?;

    let inputs_file = env.state_dir.join("inputs");
    let mut inputs = Inputs::load(&inputs_file)?;
    let (statics, unplaced) = static_files(env, repo, &build.registry, &mut inputs)?;
    inputs.save(&inputs_file)?;

    let (copies, wanted): (Vec<Wanted>, Vec<Wanted>) = wanted(&build, statics)?.into_iter().partition(|file| file.root);
    let edited: HashSet<PathBuf> = build.drifted.iter().map(|output| output.out.clone()).collect();
    let manifest: Manifest = load_json(&env.state_dir.join("manifest"))?;
    let (links, mut next) = plan(&manifest, &wanted, &edited, options.force);
    let (copy_steps, copied) = system::plan_copies(&manifest.system, &copies, options.force);

    // services restart when any file they're made of is written or changes behind its link
    let changed: HashSet<PathBuf> = links.iter().chain(&copy_steps).filter_map(written).cloned().collect();
    let placed: HashSet<PathBuf> = manifest.files.keys().chain(manifest.system.copied.keys()).cloned().collect();
    let (service_steps, services) = system::plan_services(env, &build, &manifest.system, &changed, &placed)?;
    next.system = system::Record { copied, services };

    // packages, links, root copies, services, then loose files nobody placed
    let unplaced = unplaced.into_iter().map(|file| Step::Unplaced { file });
    let steps = missing_packages(env, runner, &build.state)?.into_iter().chain(links).chain(copy_steps).chain(service_steps).chain(unplaced).collect();
    Ok(Planned { build, wanted, copies, steps, manifest, next })
}

// the destination a step writes new content to, if any
fn written(step: &Step) -> Option<&PathBuf> {
    match step {
        Step::Link { destination, .. } | Step::Relink { destination } | Step::Update { destination } | Step::Copy { destination, .. } => Some(destination),
        _ => None,
    }
}

// declared packages their backend doesn't have installed; backends with nothing declared aren't asked
fn missing_packages(env: &Env, runner: &dyn Runner, state: &MawState) -> Result<Vec<Step>, ActivateError> {
    let per_backend = state
        .packages
        .iter()
        .filter(|(_, names)| !names.is_empty())
        .map(|(name, names)| {
            let backend = backend::for_name(name, runner, env).ok_or_else(|| ActivateError::UnknownBackend(name.clone()))?;
            let installed: HashSet<String> = backend.list()?.into_iter().map(|pkg| pkg.source).collect();
            let missing = names.iter().filter(|package| !installed.contains(backend::spec_base(package)));
            Ok(missing.map(|package| Step::Install { backend: name.clone(), package: package.clone() }).collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>, ActivateError>>()?;
    Ok(per_backend.concat())
}

// installs every missing package, building srcpkgs templates first
fn install_missing(env: &Env, runner: &dyn Runner, repo: &Repo, steps: &[Step]) -> Result<(), ActivateError> {
    let targets: Vec<Target> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Install { backend, package } => Some(Target { backend: backend.clone(), spec: package.clone() }),
            _ => None,
        })
        .collect();
    if targets.is_empty() {
        return Ok(());
    }
    packages::install_targets(env, runner, repo, &targets).map_err(|error| ActivateError::Packages(Box::new(error)))
}

// the plan, plus the manifest it leaves behind once applied
fn plan(manifest: &Manifest, wanted: &[Wanted], edited: &HashSet<PathBuf>, force: bool) -> (Vec<Step>, Manifest) {
    let wanted_destinations: HashSet<&PathBuf> = wanted.iter().map(|file| &file.destination).collect();

    // links no longer wanted are removed, if they're still ours
    let removals = manifest
        .files
        .iter()
        .filter(|(destination, linked)| !wanted_destinations.contains(destination) && links_to(destination, &linked.source))
        .map(|(destination, _)| Step::Unlink { destination: destination.clone() });

    let decisions: Vec<(Option<Step>, &Wanted)> = wanted.iter().map(|file| (decide(manifest, file, edited, force), file)).collect();

    // drifted files keep their old record, everything else is recorded as wanted
    let files = decisions
        .iter()
        .filter_map(|(step, file)| match step {
            Some(Step::Replaced { .. } | Step::Edited { .. }) => Some((file.destination.clone(), manifest.files.get(&file.destination)?.clone())),
            _ => Some((file.destination.clone(), Linked { source: file.source.clone(), hash: file.hash.clone() })),
        })
        .collect();

    let steps = removals.chain(decisions.into_iter().filter_map(|(step, _)| step)).collect();
    (steps, Manifest { files, system: system::Record::default() })
}

// the step one wanted file needs, judged from the manifest and what's on disk
fn decide(manifest: &Manifest, file: &Wanted, edited: &HashSet<PathBuf>, force: bool) -> Option<Step> {
    let destination = file.destination.clone();
    let previous = manifest.files.get(&file.destination);
    let exists = fs::symlink_metadata(&file.destination).is_ok();

    if edited.contains(&file.source) {
        Some(Step::Edited { destination })
    } else if links_to(&file.destination, &file.source) {
        (previous.map(|linked| &linked.hash) != Some(&file.hash)).then_some(Step::Update { destination })
    } else if !exists {
        Some(Step::Link { destination, backup: false })
    } else if previous.is_some_and(|linked| links_to(&file.destination, &linked.source)) {
        Some(Step::Relink { destination })
    } else if previous.is_some() && !force {
        Some(Step::Replaced { destination })
    } else {
        Some(Step::Link { destination, backup: true })
    }
}

pub fn links_to(link: &Path, target: &Path) -> bool {
    fs::read_link(link).is_ok_and(|current| current == target)
}

// carries out the steps that touch disk; returns the backups made
fn apply(env: &Env, steps: &[Step], wanted: &[Wanted]) -> Result<Vec<(PathBuf, PathBuf)>, ActivateError> {
    let sources: BTreeMap<&PathBuf, &PathBuf> = wanted.iter().map(|file| (&file.destination, &file.source)).collect();
    let link = |destination: &PathBuf| -> Result<(), ActivateError> {
        fs::create_dir_all(destination.parent().unwrap()).map_err(io(destination))?;
        symlink(sources[destination], destination).map_err(io(destination))
    };

    // each step yields the backup it made, if any
    let backups = steps
        .iter()
        .map(|step| match step {
            Step::Link { destination, backup: true } => {
                let moved = backup::backup(env, destination).map_err(io(destination))?;
                link(destination)?;
                Ok(Some((destination.clone(), moved)))
            }
            Step::Link { destination, backup: false } => link(destination).map(|_| None),
            Step::Relink { destination } => {
                fs::remove_file(destination).map_err(io(destination))?;
                link(destination).map(|_| None)
            }
            Step::Unlink { destination } => fs::remove_file(destination).map_err(io(destination)).map(|_| None),
            _ => Ok(None),
        })
        .collect::<Result<Vec<_>, ActivateError>>()?;
    Ok(backups.into_iter().flatten().collect())
}

// every file to place, rendered or static, one source per destination
fn wanted(build: &Report, statics: Vec<Wanted>) -> Result<Vec<Wanted>, ActivateError> {
    let rendered = build.outputs.iter().map(|output| Wanted {
        destination: output.destination.clone(),
        source: output.out.clone(),
        hash: output.hash.clone(),
        root: output.root,
    });

    // keyed by destination, failing on the first destination claimed twice
    let by_destination = rendered.chain(statics).try_fold(BTreeMap::new(), |mut by_destination: BTreeMap<PathBuf, Wanted>, file| {
        if let Some(first) = by_destination.get(&file.destination) {
            return Err(ActivateError::Conflict { destination: file.destination, first: first.source.clone(), second: file.source });
        }
        by_destination.insert(file.destination.clone(), file);
        Ok(by_destination)
    })?;
    Ok(by_destination.into_values().collect())
}

// static/<name>/<path> goes where the registry puts <name>'s <path>; loose files go by answer or heuristic
fn static_files(env: &Env, repo: &Repo, registry: &Registry, inputs: &mut Inputs) -> Result<(Vec<Wanted>, Vec<PathBuf>), ActivateError> {
    let root = repo.static_dir();
    let mut unplaced = Vec::new();

    // (name, key) for each file, or unplaced if a loose file has neither answer nor category
    let named: Vec<(String, String, PathBuf)> = walk(&root)?
        .into_iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(&root).unwrap().to_path_buf();
            let mut parts = relative.components();
            let first = parts.next()?.as_os_str().to_string_lossy().into_owned();
            let rest = parts.as_path().to_string_lossy().into_owned();
            if !rest.is_empty() {
                return Some((first, rest, path));
            }
            match loose_name(registry, &path) {
                Some(name) => Some((name, first, path)),
                None => {
                    unplaced.push(relative);
                    None
                }
            }
        })
        .collect();

    // root-scope names are copied rather than linked
    let wanted = named
        .into_iter()
        .map(|(name, key, path)| {
            let destination = registry::resolve(env, &registry.spec(&name, &key));
            Ok(Wanted { destination, hash: inputs.hash(&path)?, source: path, root: registry.entry(&name).root })
        })
        .collect::<Result<Vec<_>, ActivateError>>()?;
    Ok((wanted, unplaced))
}

// a loose file's registry name: its own if maw.nix answers for it, else its category
fn loose_name(registry: &Registry, path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    if registry.has(&file_name) {
        return Some(file_name);
    }
    category(path).map(String::from)
}

// scripts, wallpapers, or fonts, when exactly one fits
pub fn category(path: &Path) -> Option<&'static str> {
    let extension = path.extension().map(|ext| ext.to_string_lossy().to_lowercase()).unwrap_or_default();
    let executable = fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0);
    let image = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "jxl", "avif"].contains(&extension.as_str());
    let font = ["ttf", "otf", "ttc", "woff", "woff2", "pcf", "bdf"].contains(&extension.as_str());

    let matches: Vec<&str> = [(executable, "scripts"), (image, "wallpapers"), (font, "fonts")]
        .into_iter()
        .filter_map(|(fits, name)| fits.then_some(name))
        .collect();
    if let [only] = matches[..] { Some(only) } else { None }
}

// asks where each unplaceable loose file goes and records the answers in maw.nix; returns the files answered
fn place_loose(env: &Env, runner: &dyn Runner, repo: &Repo, ask: &dyn Ask) -> Result<Vec<String>, ActivateError> {
    let mut inputs = Inputs::load(&env.state_dir.join("inputs"))?;
    let (registry, mut state) = build::load_registry(env, runner, repo, &mut inputs)?;

    // loose files with no answer and no category, asked one at a time
    let answers: Vec<(String, PathAnswer)> = walk(&repo.static_dir())?
        .into_iter()
        .filter(|path| path.parent() == Some(repo.static_dir().as_path()) && loose_name(&registry, path).is_none())
        .filter_map(|path| {
            let file_name = path.file_name()?.to_string_lossy().into_owned();
            let question = format!("static/{file_name}: [s]cript, [w]allpaper, [f]ont, or a path: ");
            let answer = parse_answer(&ask.ask(&question)?, &file_name, &registry)?;
            Some((file_name, answer))
        })
        .collect();

    if answers.is_empty() {
        return Ok(Vec::new());
    }
    let names = answers.iter().map(|(name, _)| name.clone()).collect();
    state.paths.extend(answers);
    state.write(&repo.maw_file())?;
    Ok(names)
}

// a category letter becomes that category's dir, a path ending in / is a dir, anything else is the file's destination
fn parse_answer(answer: &str, file_name: &str, registry: &Registry) -> Option<PathAnswer> {
    let answer = answer.trim();
    let category = match answer {
        "" => return None,
        "s" | "script" => "scripts",
        "w" | "wallpaper" => "wallpapers",
        "f" | "font" => "fonts",
        dir if dir.ends_with('/') => return Some(PathAnswer::Dir(dir.trim_end_matches('/').into())),
        path => return Some(PathAnswer::Files(BTreeMap::from([(file_name.into(), path.into())]))),
    };
    Some(PathAnswer::Dir(registry.entry(category).dir.unwrap_or_else(|| category.into())))
}

// every non-directory under dir, sorted; a missing dir has none
pub fn walk(dir: &Path) -> Result<Vec<PathBuf>, ActivateError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(ActivateError::Io { path: dir.into(), source }),
    };

    // symlinked dirs are treated as files so a loop can't recurse forever
    let mut files = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .map(|path| if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) { walk(&path) } else { Ok(vec![path]) })
        .collect::<Result<Vec<Vec<PathBuf>>, _>>()?
        .concat();
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, Fixture, write};
    use serde_json::json;
    use std::time::SystemTime;

    // answers every question the same way
    struct Answer(Option<&'static str>);

    impl Ask for Answer {
        fn ask(&self, _: &str) -> Option<String> {
            self.0.map(String::from)
        }
    }

    // foot and scripts modules plus a static/nvim file
    fn fixture() -> Fixture {
        let fixture = testing::fixture(&["foot", "scripts"]);
        write(&fixture.repo.static_dir().join("nvim/init.lua"), "vim.o.number = true\n");
        fixture
    }

    fn run(fixture: &Fixture, options: Options) -> Activation {
        activate(&fixture.env, &fixture.runner, &fixture.repo, &Answer(None), options).unwrap()
    }

    fn foot(fixture: &Fixture) -> PathBuf {
        fixture.env.home.join(".config/foot/foot.ini")
    }

    // every path under dir with its mtime and link target, to prove nothing was written
    fn snapshot(dir: &Path) -> Vec<(PathBuf, SystemTime, Option<PathBuf>)> {
        walk(dir)
            .unwrap()
            .into_iter()
            .map(|path| (path.clone(), fs::symlink_metadata(&path).unwrap().modified().unwrap(), fs::read_link(&path).ok()))
            .collect()
    }

    #[test]
    fn links_outputs_and_statics_then_second_run_writes_nothing() {
        let fixture = fixture();
        let first = run(&fixture, Options::default());
        assert_eq!(first.steps.len(), 3);
        assert_eq!(fs::read_link(foot(&fixture)).unwrap(), fixture.repo.out_dir().join("foot/foot.ini"));
        assert_eq!(fs::read_to_string(fixture.env.home.join(".config/nvim/init.lua")).unwrap(), "vim.o.number = true\n");

        let before = snapshot(fixture.dir.path());
        let second = run(&fixture, Options::default());
        assert!(second.steps.is_empty() && second.build.is_empty());
        assert_eq!(snapshot(fixture.dir.path()), before);
    }

    #[test]
    fn declared_packages_that_are_missing_get_installed_first() {
        let fixture = fixture();
        fs::write(fixture.repo.maw_file(), "{\n  packages = {\n    xbps = [ \"bash\" \"foot\" ];\n  };\n  services = { };\n  paths = { };\n}\n").unwrap();

        let activation = run(&fixture, Options::default());
        assert_eq!(activation.steps[0], Step::Install { backend: "xbps".into(), package: "foot".into() });
        let calls = fixture.runner.calls.borrow();
        let install = calls.iter().position(|call| call.starts_with("xbps-install")).unwrap();
        assert!(calls[install].ends_with("-y foot"));
    }

    #[test]
    fn nothing_declared_means_xbps_is_never_asked() {
        let fixture = fixture();
        run(&fixture, Options::default());
        assert!(!fixture.runner.calls.borrow().iter().any(|call| call.starts_with("xbps")));
    }

    #[test]
    fn existing_file_is_backed_up_once() {
        let fixture = fixture();
        write(&foot(&fixture), "hand written\n");

        let activation = run(&fixture, Options::default());
        let (file, moved) = &activation.backups[0];
        assert_eq!(file, &foot(&fixture));
        assert_eq!(fs::read_to_string(moved).unwrap(), "hand written\n");
        assert!(fs::symlink_metadata(foot(&fixture)).unwrap().is_symlink());
        assert!(run(&fixture, Options::default()).backups.is_empty());
    }

    #[test]
    fn removed_module_is_unlinked() {
        let fixture = fixture();
        run(&fixture, Options::default());
        fs::remove_file(fixture.repo.module_file("foot")).unwrap();

        let activation = run(&fixture, Options::default());
        assert!(activation.steps.contains(&Step::Unlink { destination: foot(&fixture) }));
        assert!(fs::symlink_metadata(foot(&fixture)).is_err());
    }

    #[test]
    fn replaced_link_is_drift_until_forced() {
        let fixture = fixture();
        run(&fixture, Options::default());
        fs::remove_file(foot(&fixture)).unwrap();
        write(&foot(&fixture), "saved by the app\n");

        let drift = run(&fixture, Options::default());
        assert_eq!(drift.steps, [Step::Replaced { destination: foot(&fixture) }]);
        assert_eq!(fs::read_to_string(foot(&fixture)).unwrap(), "saved by the app\n");

        let forced = run(&fixture, Options { force: true, ..Options::default() });
        assert_eq!(forced.backups.len(), 1);
        assert!(links_to(&foot(&fixture), &fixture.repo.out_dir().join("foot/foot.ini")));
    }

    #[test]
    fn edit_through_link_is_drift() {
        let fixture = fixture();
        run(&fixture, Options::default());
        fs::write(foot(&fixture), "edited\n").unwrap();
        fs::write(fixture.repo.config_file(), "{ changed = true; }").unwrap();

        assert_eq!(run(&fixture, Options::default()).steps, [Step::Edited { destination: foot(&fixture) }]);
        assert_eq!(fs::read_to_string(foot(&fixture)).unwrap(), "edited\n");
    }

    #[test]
    fn content_change_is_an_update() {
        let fixture = fixture();
        run(&fixture, Options::default());
        fs::write(fixture.repo.static_dir().join("nvim/init.lua"), "-- changed\n").unwrap();

        let destination = fixture.env.home.join(".config/nvim/init.lua");
        assert_eq!(run(&fixture, Options::default()).steps, [Step::Update { destination }]);
    }

    #[test]
    fn dry_run_links_nothing() {
        let fixture = fixture();
        let activation = run(&fixture, Options { dry_run: true, ..Options::default() });
        assert_eq!(activation.steps.len(), 3);
        assert!(fs::symlink_metadata(foot(&fixture)).is_err());
        assert!(!fixture.env.state_dir.join("manifest").exists());
    }

    #[test]
    fn loose_files_go_by_category() {
        let fixture = fixture();
        write(&fixture.repo.static_dir().join("sunset.png"), "png");
        let script = fixture.repo.static_dir().join("powermenu");
        write(&script, "#!/bin/sh\n");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        run(&fixture, Options::default());
        assert!(fixture.env.home.join(".local/share/wallpapers/sunset.png").is_file());
        assert!(fixture.env.home.join(".local/bin/powermenu").is_file());
    }

    #[test]
    fn unknown_loose_file_is_asked_and_recorded() {
        let fixture = fixture();
        write(&fixture.repo.static_dir().join("notes.txt"), "hi");

        let unanswered = run(&fixture, Options::default());
        assert!(unanswered.steps.contains(&Step::Unplaced { file: "notes.txt".into() }));

        let answered = activate(&fixture.env, &fixture.runner, &fixture.repo, &Answer(Some("~/notes.txt\n")), Options::default()).unwrap();
        assert_eq!(answered.answered, ["notes.txt"]);
        assert!(fs::read_to_string(fixture.repo.maw_file()).unwrap().contains(r#""notes.txt" = { "notes.txt" = "~/notes.txt"; };"#));
    }

    #[test]
    fn two_sources_for_one_destination_fail() {
        let fixture = fixture();
        write(&fixture.repo.static_dir().join("foot/foot.ini"), "static copy");
        let result = activate(&fixture.env, &fixture.runner, &fixture.repo, &Answer(None), Options::default());
        assert!(matches!(result, Err(ActivateError::Conflict { .. })));
    }

    #[test]
    fn answers_parse_to_categories_or_paths() {
        let registry = Registry::new(&json!({ "scripts": { "dir": "~/.local/bin" } }), &json!({})).unwrap();
        assert_eq!(parse_answer("s\n", "x", &registry), Some(PathAnswer::Dir("~/.local/bin".into())));
        assert_eq!(parse_answer("f", "x", &registry), Some(PathAnswer::Dir("fonts".into())));
        assert_eq!(parse_answer("~/docs/", "x", &registry), Some(PathAnswer::Dir("~/docs".into())));
        assert_eq!(parse_answer("", "x", &registry), None);
        assert_eq!(parse_answer("~/x.txt", "x", &registry), Some(PathAnswer::Files(BTreeMap::from([("x".into(), "~/x.txt".into())]))));
    }

    #[test]
    fn category_needs_exactly_one_match() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("a.PNG");
        write(&image, "");
        assert_eq!(category(&image), Some("wallpapers"));

        fs::set_permissions(&image, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(category(&image), None);
        assert_eq!(category(Path::new("/nope/notes.txt")), None);
    }
}
