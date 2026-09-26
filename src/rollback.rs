use crate::backend::srcpkgs::{self, SrcPkgs};
use crate::backend::xbps::Xbps;
use crate::backend::{self, Backend, BackendError, NAMES, SystemBackend, spec_base, split_pkgver};
use crate::edit::{self, EditError};
use crate::env::Env;
use crate::eval::{self, EvalError};
use crate::generations::{self, Generation, GenerationsError};
use crate::inputs::{InputsError, hash_bytes, load_json, save_json};
use crate::packages::{self, PackagesError, Target, declared};
use crate::repo::Repo;
use crate::runner::{RunError, Runner};
use crate::state::{MawState, StateError};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RollbackError {
    #[error("no generation {0}; `maw generations` lists them")]
    NoGeneration(u32),
    #[error("no earlier generation to roll back to")]
    NothingEarlier,
    #[error("generation {0} has no commit to restore")]
    NoCommit(u32),
    #[error("{0} has uncommitted changes; commit them first with `maw commit`")]
    Dirty(String),
    #[error(transparent)]
    Generations(#[from] GenerationsError),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error(transparent)]
    Run(#[from] RunError),
    #[error(transparent)]
    Eval(#[from] EvalError),
    #[error(transparent)]
    Edit(#[from] EditError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Inputs(#[from] InputsError),
    #[error(transparent)]
    Packages(#[from] PackagesError),
    #[error("{path}")]
    Io { path: PathBuf, source: std::io::Error },
}

// what rolling back does to packages; files follow from restoring the repo
#[derive(Debug)]
pub struct Plan {
    pub generation: Generation,
    // exact versions to install: xbps pkgvers, cargo name@version, go path@version
    pub versions: Vec<Target>,
    // declared now but not then
    pub removes: Vec<Target>,
    // recorded versions nowhere to be found, with why; these stay as they are
    pub kept: Vec<(Target, String)>,
    // xbps packages left behind the repo's newest version, held so sync skips them
    pub holds: Vec<String>,
}

// the generation a number names, or without one the one before the latest
fn target(env: &Env, number: Option<u32>) -> Result<Generation, RollbackError> {
    let all = generations::load(env)?;
    let found = match number {
        Some(number) => all.into_iter().find(|generation| generation.number == number).ok_or(RollbackError::NoGeneration(number))?,
        None => all.into_iter().rev().nth(1).ok_or(RollbackError::NothingEarlier)?,
    };
    if found.commit.is_empty() {
        return Err(RollbackError::NoCommit(found.number));
    }
    Ok(found)
}

fn git(runner: &dyn Runner, repo: &Repo, args: &[&str]) -> Result<String, RunError> {
    let args: Vec<String> = ["-C".to_string(), repo.root.display().to_string()].into_iter().chain(args.iter().map(|arg| arg.to_string())).collect();
    runner.run("git", &args)
}

// maw.nix as it was at a commit, evaluated from a copy in the cache
fn state_at(env: &Env, runner: &dyn Runner, repo: &Repo, commit: &str) -> Result<MawState, RollbackError> {
    let text = git(runner, repo, &["show", &format!("{commit}:maw.nix")])?;
    let copy = env.cache_dir().join("rollback-maw.nix");
    fs::create_dir_all(env.cache_dir()).map_err(|source| RollbackError::Io { path: copy.clone(), source })?;
    fs::write(&copy, &text).map_err(|source| RollbackError::Io { path: copy.clone(), source })?;
    let value = eval::eval_file(runner, env, &copy, "rollback-maw", &hash_bytes(text.as_bytes()))?;
    Ok(MawState::from_value(&value)?)
}

// the package a recorded pin is for: xbps "foot-1.28_1" -> foot, "bat@0.26" -> bat
fn pin_base(backend: &str, pin: &str) -> String {
    match backend {
        "xbps" => split_pkgver(pin).map_or(pin.to_string(), |(name, _)| name),
        _ => spec_base(pin).to_string(),
    }
}

// one backend's part of a rollback: versions to install, removals, and versions nowhere to be found
type BackendPlan = (Vec<Target>, Vec<Target>, Vec<(Target, String)>);

// the packages part of a rollback for one backend
fn plan_backend(env: &Env, runner: &dyn Runner, src: &SrcPkgs, backend: &dyn Backend, generation: &Generation, then: &MawState, now: &MawState) -> Result<BackendPlan, RollbackError> {
    let name = backend.name();
    let target = |spec: &str| Target { backend: name.into(), spec: spec.into() };
    let installed = backend.list()?;
    let pins = generation.packages.get(name).cloned().unwrap_or_default();
    let (then_specs, now_specs) = (declared(then, name), declared(now, name));

    // declared now but not then, and installed: removed
    let is_installed = |spec: &str| installed.iter().any(|pkg| pkg.source == spec_base(spec));
    let removes = now_specs.iter().filter(|spec| !then_specs.iter().any(|then| spec_base(then) == spec_base(spec)) && is_installed(spec)).map(|spec| target(spec)).collect();

    // declared then: its recorded version, when that isn't what's installed and the pin names a version at all
    let wanted: Vec<String> = then_specs
        .iter()
        .filter_map(|spec| pins.iter().find(|pin| pin_base(name, pin) == spec_base(spec)))
        .filter(|pin| pin_base(name, pin) != **pin)
        .filter(|pin| !installed.iter().any(|pkg| backend.pin(pkg) == **pin))
        .cloned()
        .collect();

    // xbps versions come from the cache, an earlier source build, or the repo; cargo and go fetch any version themselves
    let (versions, kept): (Vec<String>, Vec<String>) = wanted.into_iter().partition(|pin| name != "xbps" || available(env, runner, src, pin));
    let kept = kept.into_iter().map(|pin| (target(&pin_base(name, &pin)), format!("{pin} isn't in the cache, binpkgs, or the repo"))).collect();
    Ok((versions.iter().map(|pin| target(pin)).collect(), removes, kept))
}

// an xbps version the cache kept, xbps-src built earlier, or the repo's current one
fn available(env: &Env, runner: &dyn Runner, src: &SrcPkgs, pkgver: &str) -> bool {
    let xbps = Xbps::new(runner, env);
    xbps.cached(pkgver).is_some() || src.built(pkgver).is_some() || split_pkgver(pkgver).is_some_and(|(name, _)| xbps.info(&name).ok().flatten().is_some_and(|pkg| xbps.pin(&pkg) == pkgver))
}

// everything a rollback to a generation would change in packages, without changing it
pub fn plan(env: &Env, runner: &dyn Runner, repo: &Repo, number: Option<u32>) -> Result<Plan, RollbackError> {
    let generation = target(env, number)?;
    let then = state_at(env, runner, repo, &generation.commit)?;
    let (_, now) = edit::load(env, runner, repo)?;
    let src = packages::srcpkgs(env, runner, repo)?;

    // every backend's plan, merged
    let per_backend = NAMES
        .iter()
        .filter_map(|name| backend::for_name(name, runner, env))
        .map(|backend| plan_backend(env, runner, &src, backend.as_ref(), &generation, &then, &now))
        .collect::<Result<Vec<_>, _>>()?;
    let (mut versions, mut removes, mut kept) = (Vec::new(), Vec::new(), Vec::new());
    per_backend.into_iter().for_each(|(backend_versions, backend_removes, backend_kept)| {
        versions.extend(backend_versions);
        removes.extend(backend_removes);
        kept.extend(backend_kept);
    });

    // held: xbps versions this rollback installs that aren't the repo's newest
    let xbps = Xbps::new(runner, env);
    let behind = |pin: &str| split_pkgver(pin).is_some_and(|(name, _)| xbps.info(&name).ok().flatten().is_none_or(|pkg| xbps.pin(&pkg) != pin));
    let holds = versions.iter().filter(|target| target.backend == "xbps" && behind(&target.spec)).filter_map(|target| split_pkgver(&target.spec)).map(|(name, _)| name).collect();
    Ok(Plan { generation, versions, removes, kept, holds })
}

fn held_file(env: &Env) -> PathBuf {
    env.state_dir.join("held")
}

// the xbps packages maw is holding back
pub fn held(env: &Env) -> Result<Vec<String>, RollbackError> {
    Ok(load_json(&held_file(env))?)
}

// releases every hold rollback placed, so the next sync upgrades those packages; returns what was released.
// packages built with patches stay held, since an upgrade would put void's unpatched build back
pub fn release(env: &Env, runner: &dyn Runner) -> Result<Vec<String>, RollbackError> {
    let patched = srcpkgs::patched(env);
    let names: Vec<String> = held(env)?.into_iter().filter(|name| !patched.contains(name)).collect();
    if !names.is_empty() {
        Xbps::new(runner, env).hold(&names, false)?;
        save_json(&held_file(env), &Vec::<String>::new())?;
    }
    Ok(names)
}

// restores the repo to the generation's tree, then removes, installs versions, and updates holds; activating is up to the caller
pub fn rollback(env: &Env, runner: &dyn Runner, repo: &Repo, plan: &Plan) -> Result<(), RollbackError> {
    if !git(runner, repo, &["status", "--porcelain"])?.trim().is_empty() {
        return Err(RollbackError::Dirty(repo.root.display().to_string()));
    }
    git(runner, repo, &["restore", &format!("--source={}", plan.generation.commit), "--staged", "--worktree", "--", ":/"])?;

    by_backend(&plan.removes).into_iter().try_for_each(|(name, specs)| backend::for_name(&name, runner, env).unwrap().remove(&specs))?;

    // old holds come off first, since a held package can't change version
    let xbps = Xbps::new(runner, env);
    release(env, runner)?;
    let src = packages::srcpkgs(env, runner, repo)?;
    by_backend(&plan.versions).into_iter().try_for_each(|(name, specs)| match name.as_str() {
        "xbps" => install_xbps_versions(&xbps, &src, &specs),
        _ => backend::for_name(&name, runner, env).unwrap().install(&specs),
    })?;
    if !plan.holds.is_empty() {
        xbps.hold(&plan.holds, true)?;
        save_json(&held_file(env), &plan.holds)?;
    }
    Ok(())
}

// xbps versions that only an earlier source build has come from binpkgs, as do patched packages, whose builds share
// void's versions; the rest from the cache or the repo
fn install_xbps_versions(xbps: &Xbps, src: &SrcPkgs, pkgvers: &[String]) -> Result<(), BackendError> {
    let patched = |pkgver: &String| split_pkgver(pkgver).is_some_and(|(name, _)| src.patches(&name));
    let (built, others): (Vec<String>, Vec<String>) = pkgvers.iter().cloned().partition(|pkgver| src.built(pkgver).is_some() && (xbps.cached(pkgver).is_none() || patched(pkgver)));
    if !built.is_empty() {
        xbps.install_from(&src.binpkgs(), &built, true)?;
    }
    if !others.is_empty() {
        xbps.install_versions(&others)?;
    }
    Ok(())
}

// specs grouped by backend
fn by_backend(targets: &[Target]) -> BTreeMap<String, Vec<String>> {
    targets.iter().fold(BTreeMap::new(), |mut groups, target| {
        groups.entry(target.backend.clone()).or_insert_with(Vec::new).push(target.spec.clone());
        groups
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, fixture, write};

    // a fixture with a real git repo; installed xbps is bash-5.2_1, the repo's newest is bash-1.0_1
    fn setup() -> Fixture {
        let fixture = fixture(&[]);
        git(&fixture.runner, &fixture.repo, &["init", "-q"]).unwrap();
        fixture
    }

    // commits maw.nix declaring these xbps packages and logs a generation recording these pins
    fn generation(fixture: &Fixture, number: u32, declared: &str, pins: &[&str]) {
        let text = format!("{{\n  packages = {{\n    xbps = [ {declared} ];\n  }};\n  services = {{ }};\n  paths = {{ }};\n}}\n");
        fs::write(fixture.repo.maw_file(), text).unwrap();
        git(&fixture.runner, &fixture.repo, &["add", "-A"]).unwrap();
        git(&fixture.runner, &fixture.repo, &["commit", "-q", "--allow-empty", "-m", &format!("generation {number}")]).unwrap();
        let commit = git(&fixture.runner, &fixture.repo, &["rev-parse", "HEAD"]).unwrap().trim().to_string();

        let packages = BTreeMap::from([("xbps".to_string(), pins.iter().map(|pin| pin.to_string()).collect())]);
        let entry = Generation { number, time: "t".into(), commit, message: format!("generation {number}"), packages };
        let log = fixture.env.state_dir.join("generations");
        let before = fs::read_to_string(&log).unwrap_or_default();
        write(&log, &format!("{before}{}\n", serde_json::to_string(&entry).unwrap()));
    }

    fn cache(fixture: &Fixture, pkgver: &str) {
        write(&fixture.env.sysroot.join("var/cache/xbps").join(format!("{pkgver}.x86_64.xbps")), "");
    }

    fn xbps(spec: &str) -> Target {
        Target { backend: "xbps".into(), spec: spec.into() }
    }

    #[test]
    fn cached_old_versions_are_installed_and_held() {
        let fixture = setup();
        cache(&fixture, "bash-5.1_1");
        generation(&fixture, 1, r#""bash""#, &["bash-5.1_1"]);
        generation(&fixture, 2, r#""bash""#, &["bash-5.2_1"]);

        let plan = plan(&fixture.env, &fixture.runner, &fixture.repo, None).unwrap();
        assert_eq!(plan.generation.number, 1);
        assert_eq!((plan.versions, plan.holds), (vec![xbps("bash-5.1_1")], vec!["bash".to_string()]));
    }

    #[test]
    fn versions_nowhere_to_be_found_are_kept() {
        let fixture = setup();
        generation(&fixture, 1, r#""bash""#, &["bash-5.0_1"]);
        generation(&fixture, 2, r#""bash""#, &["bash-5.2_1"]);

        let plan = plan(&fixture.env, &fixture.runner, &fixture.repo, Some(1)).unwrap();
        assert!(plan.versions.is_empty());
        assert_eq!(plan.kept, [(xbps("bash"), "bash-5.0_1 isn't in the cache, binpkgs, or the repo".to_string())]);
    }

    #[test]
    fn packages_declared_since_are_removed() {
        let fixture = setup();
        generation(&fixture, 1, "", &[]);
        generation(&fixture, 2, r#""bash""#, &["bash-5.2_1"]);
        assert_eq!(plan(&fixture.env, &fixture.runner, &fixture.repo, Some(1)).unwrap().removes, [xbps("bash")]);
    }

    #[test]
    fn rollback_restores_the_repo_installs_versions_and_holds_until_released() {
        let fixture = setup();
        cache(&fixture, "bash-5.1_1");
        generation(&fixture, 1, r#""bash""#, &["bash-5.1_1"]);
        generation(&fixture, 2, r#""bash" "foot""#, &["bash-5.2_1"]);

        let plan = plan(&fixture.env, &fixture.runner, &fixture.repo, Some(1)).unwrap();
        rollback(&fixture.env, &fixture.runner, &fixture.repo, &plan).unwrap();
        assert!(!fs::read_to_string(fixture.repo.maw_file()).unwrap().contains("foot"));

        let calls = fixture.runner.calls.borrow().clone();
        assert!(calls.iter().any(|call| call.starts_with("xbps-install") && call.ends_with("-fy bash-5.1_1")));
        assert!(calls.iter().any(|call| call.starts_with("xbps-pkgdb") && call.ends_with("-m hold bash")));
        assert_eq!(held(&fixture.env).unwrap(), ["bash"]);

        assert_eq!(release(&fixture.env, &fixture.runner).unwrap(), ["bash"]);
        assert!(held(&fixture.env).unwrap().is_empty());
    }

    #[test]
    fn uncommitted_changes_stop_a_rollback() {
        let fixture = setup();
        generation(&fixture, 1, "", &[]);
        generation(&fixture, 2, "", &[]);
        fs::write(fixture.repo.config_file(), "{ unsaved = 1; }").unwrap();

        let plan = plan(&fixture.env, &fixture.runner, &fixture.repo, None).unwrap();
        assert!(matches!(rollback(&fixture.env, &fixture.runner, &fixture.repo, &plan), Err(RollbackError::Dirty(_))));
    }

    #[test]
    fn a_single_generation_has_nothing_before_it() {
        let fixture = setup();
        generation(&fixture, 1, "", &[]);
        assert!(matches!(plan(&fixture.env, &fixture.runner, &fixture.repo, None), Err(RollbackError::NothingEarlier)));
        assert!(matches!(plan(&fixture.env, &fixture.runner, &fixture.repo, Some(9)), Err(RollbackError::NoGeneration(9))));
    }
}
