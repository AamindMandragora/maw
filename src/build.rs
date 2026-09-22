use crate::env::Env;
use crate::eval::{self, EvalError, RenderedFile};
use crate::inputs::{Inputs, InputsError, combine, hash_bytes};
use crate::registry::{self, Registry, RegistryError};
use crate::repo::{Repo, RepoError};
use crate::runner::Runner;
use std::collections::HashSet;
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
    #[error("{path}: {source}")]
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
}

#[derive(Debug, Default)]
pub struct Report {
    pub evaluated: Vec<String>,
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    pub outputs: Vec<Output>,
}

// renders every module into out/, re-evaluating only modules whose inputs changed
pub fn build(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Report, BuildError> {
    let inputs_file = env.state_dir.join("inputs");
    let mut inputs = Inputs::load(&inputs_file)?;

    // anything every module depends on: config.nix, maw.nix, the maw lib
    let lib_hash = combine(&[
        &inputs.hash(&env.nix_dir().join("lib.nix"))?,
        &inputs.hash(&env.nix_dir().join("default.nix"))?,
        env!("CARGO_PKG_VERSION"),
    ]);
    let maw_hash = inputs.hash(&repo.maw_file())?;
    let shared_hash = combine(&[&inputs.hash(&repo.config_file())?, &maw_hash, &lib_hash]);
    let registry = load_registry(env, runner, repo, &mut inputs, &maw_hash)?;

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

    // write every file whose content changed
    let files: Vec<&RenderedFile> = modules.iter().flat_map(|(_, files, _)| files).collect();
    let outputs: Vec<Output> = files.iter().map(|file| to_output(env, repo, &registry, file)).collect();
    let writes = files
        .iter()
        .zip(&outputs)
        .map(|(file, output)| Ok((output.out.clone(), write_if_changed(&output.out, &file.content, output.executable)?)))
        .collect::<Result<Vec<_>, BuildError>>()?;

    let keep: HashSet<PathBuf> = outputs.iter().map(|output| output.out.clone()).collect();
    let report = Report {
        evaluated: modules.iter().filter(|(_, _, fresh)| *fresh).map(|(name, _, _)| name.clone()).collect(),
        written: writes.into_iter().filter(|(_, written)| *written).map(|(out, _)| out).collect(),
        removed: prune(&repo.out_dir(), &keep)?,
        outputs,
    };

    inputs.save(&inputs_file)?;
    Ok(report)
}

// shipped registry plus maw.nix path answers, each evaluated only when its file changed
fn load_registry(env: &Env, runner: &dyn Runner, repo: &Repo, inputs: &mut Inputs, maw_hash: &str) -> Result<Registry, BuildError> {
    let shipped_hash = inputs.hash(&env.registry_file())?;
    let shipped = eval::eval_file(runner, env, &env.registry_file(), "registry", &shipped_hash)?;
    let state = eval::eval_file(runner, env, &repo.maw_file(), "maw", maw_hash)?;
    Ok(Registry::new(&shipped, &state["paths"])?)
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
    }
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

// deletes files under dir that aren't kept, then any dirs left empty; returns deleted files
fn prune(dir: &Path, keep: &HashSet<PathBuf>) -> Result<Vec<PathBuf>, BuildError> {
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
                let inner = prune(&path, keep)?;
                if fs::read_dir(&path).map_err(io(&path))?.next().is_none() {
                    fs::remove_dir(&path).map_err(io(&path))?;
                }
                Ok(inner)
            } else if keep.contains(&path) {
                Ok(Vec::new())
            } else {
                fs::remove_file(&path).map_err(io(&path))?;
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
    use serde_json::json;

    struct Fixture {
        _dir: tempfile::TempDir,
        env: Env,
        repo: Repo,
        runner: FakeRunner,
    }

    // a repo whose modules/<name>.nix "evaluates" to one file per name, via a fake nix
    fn fixture(modules: &[&str]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::new(&dir.path().join("home"), &dir.path().join("sys"), &dir.path().join("share"));
        fs::create_dir_all(env.nix_dir()).unwrap();
        ["lib.nix", "default.nix"].iter().for_each(|file| fs::write(env.nix_dir().join(file), "").unwrap());
        fs::write(env.registry_file(), "").unwrap();

        let (repo, _) = Repo::init(&env, &FakeRunner::new(|_, _| String::new()), &dir.path().join("dots")).unwrap();
        modules.iter().for_each(|name| fs::write(repo.module_file(name), "").unwrap());

        let runner = FakeRunner::new(fake_nix);
        Fixture { _dir: dir, env, repo, runner }
    }

    // answers registry.nix, maw.nix, and -A modules.<name> like nix-instantiate would
    fn fake_nix(_: &str, args: &[String]) -> String {
        let target = args.last().unwrap();
        if target.ends_with("registry.nix") {
            return json!({ "foot": { "files": { "main": "foot/foot.ini" } }, "scripts": { "dir": "~/.local/bin", "executable": true } }).to_string();
        }
        if target.ends_with("maw.nix") {
            return json!({ "paths": {} }).to_string();
        }
        let name = args[args.len() - 2].trim_start_matches("modules.");
        json!([{ "name": name, "key": "main", "content": format!("{name} v1\n"), "executable": false, "scope": "user" }]).to_string()
    }

    fn module_evals(runner: &FakeRunner) -> usize {
        runner.calls.borrow().iter().filter(|call| call.contains("-A modules.")).count()
    }

    #[test]
    fn first_build_writes_and_second_is_a_no_op() {
        let fixture = fixture(&["foot", "scripts"]);
        let first = build(&fixture.env, &fixture.runner, &fixture.repo).unwrap();
        assert_eq!(first.evaluated, ["foot", "scripts"]);
        assert_eq!(fs::read_to_string(fixture.repo.out_dir().join("foot/foot.ini")).unwrap(), "foot v1\n");

        let calls_before = fixture.runner.calls.borrow().len();
        let second = build(&fixture.env, &fixture.runner, &fixture.repo).unwrap();
        assert!(second.evaluated.is_empty() && second.written.is_empty() && second.removed.is_empty());
        assert_eq!(fixture.runner.calls.borrow().len(), calls_before);
    }

    #[test]
    fn config_change_reevaluates_every_module() {
        let fixture = fixture(&["foot", "scripts"]);
        build(&fixture.env, &fixture.runner, &fixture.repo).unwrap();

        fs::write(fixture.repo.config_file(), "{ changed = true; }").unwrap();
        let report = build(&fixture.env, &fixture.runner, &fixture.repo).unwrap();
        assert_eq!(report.evaluated.len(), 2);
        assert_eq!(module_evals(&fixture.runner), 4);
    }

    #[test]
    fn module_change_reevaluates_only_that_module() {
        let fixture = fixture(&["foot", "scripts"]);
        build(&fixture.env, &fixture.runner, &fixture.repo).unwrap();

        fs::write(fixture.repo.module_file("foot"), "# edited").unwrap();
        assert_eq!(build(&fixture.env, &fixture.runner, &fixture.repo).unwrap().evaluated, ["foot"]);
    }

    #[test]
    fn deleted_module_is_pruned_from_out() {
        let fixture = fixture(&["foot", "scripts"]);
        build(&fixture.env, &fixture.runner, &fixture.repo).unwrap();

        fs::remove_file(fixture.repo.module_file("foot")).unwrap();
        let report = build(&fixture.env, &fixture.runner, &fixture.repo).unwrap();
        assert_eq!(report.removed, [fixture.repo.out_dir().join("foot/foot.ini")]);
        assert!(!fixture.repo.out_dir().join("foot").exists());
    }

    #[test]
    fn outputs_carry_destination_and_mode() {
        let fixture = fixture(&["foot", "scripts"]);
        let report = build(&fixture.env, &fixture.runner, &fixture.repo).unwrap();

        let scripts = report.outputs.iter().find(|output| output.name == "scripts").unwrap();
        assert_eq!(scripts.destination, fixture.env.home.join(".local/bin/config"));
        assert!(scripts.executable && is_executable(&scripts.out));
        assert_eq!(report.outputs[0].destination, fixture.env.home.join(".config/foot/foot.ini"));
    }
}
