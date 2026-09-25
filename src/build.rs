use crate::backup;
use crate::env::Env;
use crate::eval::{self, EvalError, RenderedFile};
use crate::init::runit::Runit;
use crate::init::{InitBackend, Scope};
use crate::inputs::{Inputs, InputsError, combine, hash_bytes, load_json, save_json};
use crate::registry::{self, Registry, RegistryError};
use crate::repo::{Repo, RepoError};
use crate::runner::Runner;
use crate::state::{MawState, StateError};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    Eval(#[from] EvalError),
    #[error(transparent)]
    Inputs(#[from] InputsError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error("{path}")]
    Io { path: PathBuf, source: std::io::Error },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> BuildError + '_ {
    move |source| BuildError::Io { path: path.into(), source }
}

// one rendered file under out/ and where it will be placed
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub name: String,
    pub key: String,
    pub out: PathBuf,
    pub destination: PathBuf,
    pub root: bool,
    pub executable: bool,
    pub hash: String,
    pub content: String,
    pub service: Option<ServiceRef>,
}

// the service a file belongs to, so activation can enable and restart it
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServiceRef {
    pub name: String,
    pub scope: Scope,
    pub enable: bool,
}

// dry_run writes nothing to out/; force overwrites files edited in place; no_build (activation only) skips nix and uses out/.maw/index.json
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub dry_run: bool,
    pub force: bool,
    pub no_build: bool,
}

#[derive(Debug, Default)]
pub struct Report {
    pub evaluated: Vec<String>,
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    pub outputs: Vec<Output>,
    // out/ files edited in place, left alone
    pub drifted: Vec<Output>,
    // edited out/ files moved aside by --force: (file, backup)
    pub backups: Vec<(PathBuf, PathBuf)>,
    pub registry: Registry,
    pub state: MawState,
    pub settings: Settings,
}

fn yes() -> bool {
    true
}

// maw's settings, from `maw = { ... };` in config.nix
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    // commit the repo and record a generation after each activation that changes something
    #[serde(default = "yes")]
    pub auto_commit: bool,
    // where maw keeps its void-packages clone for building srcpkgs; ~/ is home
    pub void_packages: Option<String>,
    // where maw keeps its nixpkgs clone for `src new --from-nix`; ~/ is home
    pub nixpkgs: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto_commit: true, void_packages: None, nixpkgs: None }
    }
}

impl Settings {
    // the void-packages clone: the configured path (~/ is home, relative is under home), else maw's own under ~/.local/share
    pub fn void_packages(&self, env: &Env) -> PathBuf {
        match self.void_packages.as_deref() {
            Some(path) => env.home.join(path.strip_prefix("~/").unwrap_or(path)),
            None => env.home.join(".local/share/maw/void-packages"),
        }
    }

    // the nixpkgs clone, configured or maw's own
    pub fn nixpkgs(&self, env: &Env) -> PathBuf {
        match self.nixpkgs.as_deref() {
            Some(path) => env.home.join(path.strip_prefix("~/").unwrap_or(path)),
            None => env.home.join(".local/share/maw/nixpkgs"),
        }
    }
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.evaluated.is_empty() && self.written.is_empty() && self.removed.is_empty() && self.drifted.is_empty()
    }
}

// what happened to one out/ file
#[derive(Debug, PartialEq)]
enum Placement {
    Unchanged,
    Written,
    Drifted,
    Replaced(PathBuf),
}

// renders every module into out/, re-evaluating only modules whose inputs changed
pub fn build(env: &Env, runner: &dyn Runner, repo: &Repo, options: Options) -> Result<Report, BuildError> {
    let inputs_file = env.state_dir.join("inputs");
    let mut inputs = Inputs::load(&inputs_file)?;

    let shared_hash = shared_hash(env, repo, &mut inputs)?;
    let (registry, state) = load_registry(env, runner, repo, &mut inputs)?;
    let settings = eval_settings(env, runner, repo, &shared_hash)?;

    // evaluate each module, keyed by its own file plus the shared inputs
    let modules = repo
        .module_names()?
        .into_iter()
        .map(|name| {
            let key = combine(&[&inputs.hash(&repo.module_file(&name))?, &shared_hash]);
            let (files, fresh) = eval::eval_module(runner, env, &repo.root, &name, &key)?;
            Ok((name, files, fresh))
        })
        .collect::<Result<Vec<_>, BuildError>>()?;

    let files: Vec<&RenderedFile> = modules.iter().flat_map(|(_, files, _)| files).collect();
    let mut outputs: Vec<Output> = files.iter().flat_map(|file| to_outputs(env, repo, &registry, file)).collect();
    outputs.extend(local_repo(env, repo, &state, &settings));
    let placements = place_all(env, &outputs, options)?;

    // sort each file into the report by what happened to it
    let placed = || outputs.iter().zip(&placements);
    // the activation index lives in out/ too, written by activate rather than build
    let keep: HashSet<PathBuf> = outputs.iter().map(|output| output.out.clone()).chain([index_file(repo)]).collect();
    let report = Report {
        evaluated: modules.iter().filter(|(_, _, fresh)| *fresh).map(|(name, _, _)| name.clone()).collect(),
        written: placed().filter(|(_, placement)| !matches!(placement, Placement::Unchanged | Placement::Drifted)).map(|(output, _)| output.out.clone()).collect(),
        removed: prune(&repo.out_dir(), &keep, options.dry_run)?,
        drifted: placed().filter(|(_, placement)| **placement == Placement::Drifted).map(|(output, _)| output.clone()).collect(),
        backups: placed()
            .filter_map(|(output, placement)| match placement {
                Placement::Replaced(backup) => Some((output.out.clone(), backup.clone())),
                _ => None,
            })
            .collect(),
        outputs,
        registry,
        state,
        settings,
    };

    inputs.save(&inputs_file)?;
    Ok(report)
}

// places every file in out/, checked against the hash maw last wrote there, and records the new hashes
fn place_all(env: &Env, outputs: &[Output], options: Options) -> Result<Vec<Placement>, BuildError> {
    let recorded_file = env.state_dir.join("outputs");
    let recorded: BTreeMap<PathBuf, String> = load_json(&recorded_file)?;
    let placements = outputs
        .iter()
        .map(|output| place(env, output, recorded.get(&output.out), options))
        .collect::<Result<Vec<_>, BuildError>>()?;

    // drifted files keep their old record so they stay drifted until forced
    let now_recorded: BTreeMap<PathBuf, String> = outputs
        .iter()
        .zip(&placements)
        .filter_map(|(output, placement)| match placement {
            Placement::Drifted => Some((output.out.clone(), recorded.get(&output.out)?.clone())),
            _ => Some((output.out.clone(), output.hash.clone())),
        })
        .collect();
    if now_recorded != recorded && !options.dry_run {
        save_json(&recorded_file, &now_recorded)?;
    }
    Ok(placements)
}

// where activate records every file's source and destination, for activating without nix
pub fn index_file(repo: &Repo) -> PathBuf {
    repo.out_dir().join(".maw/index.json")
}

// /etc/xbps.d/10-maw-local.conf, making the void-packages clone's builds a repo xbps always sees; only while a source package is declared
fn local_repo(env: &Env, repo: &Repo, state: &MawState, settings: &Settings) -> Option<Output> {
    let declared = state.packages.get("xbps")?;
    declared.iter().any(|name| repo.srcpkgs_dir().join(name).join("template").is_file()).then(|| {
        let content = format!("# written by maw: packages built from srcpkgs/\nrepository={}\n", settings.void_packages(env).join("hostdir/binpkgs").display());
        Output {
            name: ".maw".into(),
            key: "10-maw-local.conf".into(),
            out: repo.out_dir().join(".maw/10-maw-local.conf"),
            destination: env.sysroot.join("etc/xbps.d/10-maw-local.conf"),
            root: true,
            executable: false,
            hash: hash_bytes(content.as_bytes()),
            content,
            service: None,
        }
    })
}

// what every module depends on: config.nix, maw.nix, and the maw lib
fn shared_hash(env: &Env, repo: &Repo, inputs: &mut Inputs) -> Result<String, BuildError> {
    let lib_hash = combine(&[
        &inputs.hash(&env.nix_dir().join("lib.nix"))?,
        &inputs.hash(&env.nix_dir().join("default.nix"))?,
        env!("CARGO_PKG_VERSION"),
    ]);
    Ok(combine(&[&inputs.hash(&repo.config_file())?, &inputs.hash(&repo.maw_file())?, &lib_hash]))
}

fn eval_settings(env: &Env, runner: &dyn Runner, repo: &Repo, key: &str) -> Result<Settings, BuildError> {
    let value = eval::eval_settings(runner, env, &repo.root, key)?;
    Ok(serde_json::from_value(value).map_err(|source| EvalError::Shape { what: "config.nix maw".into(), source })?)
}

// maw's settings alone, without building any module
pub fn settings(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Settings, BuildError> {
    let inputs_file = env.state_dir.join("inputs");
    let mut inputs = Inputs::load(&inputs_file)?;
    let key = shared_hash(env, repo, &mut inputs)?;
    inputs.save(&inputs_file)?;
    eval_settings(env, runner, repo, &key)
}

// shipped registry plus maw.nix path answers, each evaluated only when its file changed
pub fn load_registry(env: &Env, runner: &dyn Runner, repo: &Repo, inputs: &mut Inputs) -> Result<(Registry, MawState), BuildError> {
    let shipped_hash = inputs.hash(&env.registry_file())?;
    let shipped = eval::eval_file(runner, env, &env.registry_file(), "registry", &shipped_hash)?;
    let maw_hash = inputs.hash(&repo.maw_file())?;
    let state = eval::eval_file(runner, env, &repo.maw_file(), "maw", &maw_hash)?;
    Ok((Registry::new(&shipped, &state["paths"])?, MawState::from_value(&state)?))
}

// the out/ files a rendered file becomes: itself, or a service's files from the init backend
fn to_outputs(env: &Env, repo: &Repo, registry: &Registry, file: &RenderedFile) -> Vec<Output> {
    match &file.service {
        Some(service) => service_outputs(env, repo, &file.name, Scope::parse(&file.scope), service),
        None => vec![to_output(env, repo, registry, file)],
    }
}

// a service's files under out/sv/<name>/, headed for its definition dir
fn service_outputs(env: &Env, repo: &Repo, name: &str, scope: Scope, service: &crate::init::ServiceDef) -> Vec<Output> {
    let service_ref = ServiceRef { name: name.into(), scope, enable: service.enable };
    Runit
        .render(name, scope, service)
        .into_iter()
        .map(|(path, content, executable)| Output {
            name: name.into(),
            key: path.clone(),
            out: repo.out_dir().join("sv").join(name).join(&path),
            destination: Runit.definition(env, scope, name).join(&path),
            root: scope == Scope::System,
            executable,
            hash: hash_bytes(content.as_bytes()),
            content,
            service: Some(service_ref.clone()),
        })
        .collect()
}

// where a rendered file lives in out/ and at its destination
fn to_output(env: &Env, repo: &Repo, registry: &Registry, file: &RenderedFile) -> Output {
    let entry = registry.entry(&file.name);
    Output {
        name: file.name.clone(),
        key: file.key.clone(),
        out: repo.out_dir().join(&file.name).join(registry.out_name(&file.name, &file.key)),
        destination: registry::resolve(env, &registry.spec(&file.name, &file.key)),
        root: entry.root || file.scope == "root",
        executable: entry.executable || file.executable,
        hash: hash_bytes(file.content.as_bytes()),
        content: file.content.clone(),
        service: None,
    }
}

// writes one out/ file, unless it was edited since maw last wrote it; --force backs up the edit first
fn place(env: &Env, output: &Output, recorded: Option<&String>, options: Options) -> Result<Placement, BuildError> {
    let current = fs::read(&output.out).ok().map(|bytes| hash_bytes(&bytes));
    let edited = matches!((&current, recorded), (Some(current), Some(written)) if current != written && *current != output.hash);
    if edited && (!options.force || options.dry_run) {
        return Ok(Placement::Drifted);
    }
    if options.dry_run {
        let stale = current.as_ref() != Some(&output.hash) || is_executable(&output.out) != output.executable;
        return Ok(if stale { Placement::Written } else { Placement::Unchanged });
    }

    let backup = if edited { Some(backup::backup(env, &output.out).map_err(io(&output.out))?) } else { None };
    let written = write_if_changed(&output.out, &output.content, output.executable)?;
    Ok(match (backup, written) {
        (Some(backup), _) => Placement::Replaced(backup),
        (None, true) => Placement::Written,
        (None, false) => Placement::Unchanged,
    })
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

// writes content unless the file already holds it with the right mode; true if written
fn write_if_changed(path: &Path, content: &str, executable: bool) -> Result<bool, BuildError> {
    let same_content = fs::read(path).is_ok_and(|existing| existing == content.as_bytes());
    if same_content && is_executable(path) == executable {
        return Ok(false);
    }

    fs::create_dir_all(path.parent().unwrap()).map_err(io(path))?;
    fs::write(path, content).map_err(io(path))?;
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(io(path))?;
    Ok(true)
}

// deletes files under dir that aren't kept, then any dirs left empty; returns deleted files, or with dry_run the ones it would delete
fn prune(dir: &Path, keep: &HashSet<PathBuf>, dry_run: bool) -> Result<Vec<PathBuf>, BuildError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(BuildError::Io { path: dir.into(), source }),
    };

    // recurse into dirs, delete unkept files, and flatten what was deleted
    let removed = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .map(|path| {
            if path.is_dir() {
                let inner = prune(&path, keep, dry_run)?;
                if !dry_run && fs::read_dir(&path).map_err(io(&path))?.next().is_none() {
                    fs::remove_dir(&path).map_err(io(&path))?;
                }
                Ok(inner)
            } else if keep.contains(&path) {
                Ok(Vec::new())
            } else {
                if !dry_run {
                    fs::remove_file(&path).map_err(io(&path))?;
                }
                Ok(vec![path])
            }
        })
        .collect::<Result<Vec<Vec<PathBuf>>, BuildError>>()?;
    Ok(removed.concat())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use crate::testing::fixture;

    fn module_evals(runner: &FakeRunner) -> usize {
        runner.calls.borrow().iter().filter(|call| call.contains("-A modules.")).count()
    }

    #[test]
    fn first_build_writes_and_second_is_a_no_op() {
        let fixture = fixture(&["foot", "scripts"]);
        let first = build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap();
        assert_eq!(first.evaluated, ["foot", "scripts"]);
        assert_eq!(fs::read_to_string(fixture.repo.out_dir().join("foot/foot.ini")).unwrap(), "foot v1\n");

        let calls_before = fixture.runner.calls.borrow().len();
        let second = build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap();
        assert!(second.is_empty());
        assert_eq!(fixture.runner.calls.borrow().len(), calls_before);
    }

    #[test]
    fn config_change_reevaluates_every_module() {
        let fixture = fixture(&["foot", "scripts"]);
        build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap();

        fs::write(fixture.repo.config_file(), "{ changed = true; }").unwrap();
        let report = build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap();
        assert_eq!(report.evaluated.len(), 2);
        assert_eq!(module_evals(&fixture.runner), 4);
    }

    #[test]
    fn module_change_reevaluates_only_that_module() {
        let fixture = fixture(&["foot", "scripts"]);
        build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap();

        fs::write(fixture.repo.module_file("foot"), "# edited").unwrap();
        assert_eq!(build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap().evaluated, ["foot"]);
    }

    #[test]
    fn deleted_module_is_pruned_from_out() {
        let fixture = fixture(&["foot", "scripts"]);
        build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap();

        fs::remove_file(fixture.repo.module_file("foot")).unwrap();
        let report = build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap();
        assert_eq!(report.removed, [fixture.repo.out_dir().join("foot/foot.ini")]);
        assert!(!fixture.repo.out_dir().join("foot").exists());
    }

    #[test]
    fn outputs_carry_destination_and_mode() {
        let fixture = fixture(&["foot", "scripts"]);
        let report = build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap();

        let scripts = report.outputs.iter().find(|output| output.name == "scripts").unwrap();
        assert_eq!(scripts.destination, fixture.env.home.join(".local/bin/config"));
        assert!(scripts.executable && is_executable(&scripts.out));
        assert_eq!(report.outputs[0].destination, fixture.env.home.join(".config/foot/foot.ini"));
    }

    #[test]
    fn void_packages_defaults_to_maws_own_clone() {
        let env = Env::new(Path::new("/h"), Path::new("/"), Path::new("/s"));
        let settings = |path: Option<&str>| Settings { void_packages: path.map(String::from), ..Settings::default() };
        assert_eq!(settings(None).void_packages(&env), PathBuf::from("/h/.local/share/maw/void-packages"));
        assert_eq!(settings(Some("~/src/vp")).void_packages(&env), PathBuf::from("/h/src/vp"));
        assert_eq!(settings(Some("/opt/vp")).void_packages(&env), PathBuf::from("/opt/vp"));
    }

    #[test]
    fn dry_run_reports_without_writing() {
        let fixture = fixture(&["foot"]);
        let report = build(&fixture.env, &fixture.runner, &fixture.repo, Options { dry_run: true, ..Options::default() }).unwrap();
        assert_eq!(report.written, [fixture.repo.out_dir().join("foot/foot.ini")]);
        assert!(!fixture.repo.out_dir().join("foot").exists());
        assert!(!fixture.env.state_dir.join("outputs").exists());
    }

    #[test]
    fn edited_out_file_is_drift_until_forced() {
        let fixture = fixture(&["foot"]);
        build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap();
        let out = fixture.repo.out_dir().join("foot/foot.ini");
        fs::write(&out, "edited through the link\n").unwrap();

        // a module change would normally rewrite out/, but the edit is kept
        fs::write(fixture.repo.config_file(), "{ changed = true; }").unwrap();
        let report = build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap();
        assert_eq!(report.drifted.len(), 1);
        assert_eq!(fs::read_to_string(&out).unwrap(), "edited through the link\n");
        assert_eq!(build(&fixture.env, &fixture.runner, &fixture.repo, Options::default()).unwrap().drifted.len(), 1);

        let forced = build(&fixture.env, &fixture.runner, &fixture.repo, Options { force: true, ..Options::default() }).unwrap();
        assert_eq!(fs::read_to_string(&out).unwrap(), "foot v1\n");
        assert_eq!(fs::read_to_string(&forced.backups[0].1).unwrap(), "edited through the link\n");
    }
}
