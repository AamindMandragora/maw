use crate::activate::Ask;
use crate::backend::cargo::Cargo;
use crate::backend::xbps::Xbps;
use crate::backend::{self, Backend, BackendError, NAMES, Pkg, spec_base, spec_program};
use crate::edit::{self, EditError};
use crate::env::Env;
use crate::registry::Registry;
use crate::repo::Repo;
use crate::runner::Runner;
use crate::state::{MawState, StateError};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum PackagesError {
    #[error("no package {0}; `maw search {0}` to look for it")]
    NotFound(String),
    #[error("{0} isn't in xbps; install it from crates.io with `maw install cargo:{0}`")]
    Unconfirmed(String),
    #[error("{0} is a library crate, not a program; add it to a project with `cargo add {0}`")]
    Library(String),
    #[error("{0} is neither installed nor declared")]
    Unknown(String),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error(transparent)]
    Edit(#[from] EditError),
    #[error(transparent)]
    State(#[from] StateError),
}

// a package in one backend, as maw.nix declares it: "foot", "bat@0.24", a git url, a go path
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Target {
    pub backend: String,
    pub spec: String,
}

impl std::fmt::Display for Target {
    // xbps packages by name alone, the rest with the prefix that selects their backend
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.backend == "xbps" { write!(formatter, "{}", self.spec) } else { write!(formatter, "{}:{}", self.backend, self.spec) }
    }
}

// what an install or remove does: packages changed, maw.nix entries changed, modules created
#[derive(Debug, Default, PartialEq)]
pub struct Change {
    pub packages: Vec<Target>,
    pub recorded: Vec<Target>,
    pub scaffolded: Vec<String>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.recorded.is_empty() && self.scaffolded.is_empty()
    }
}

// the specs a maw.nix declares for a backend
pub fn declared(state: &MawState, backend: &str) -> Vec<String> {
    state.packages.get(backend).cloned().unwrap_or_default()
}

// what the packages commands work from: backends, maw.nix, the registry, and each backend's installed sources
struct Context<'a> {
    backends: Vec<Box<dyn Backend + 'a>>,
    state: MawState,
    registry: Registry,
    installed: BTreeMap<String, HashSet<String>>,
}

impl<'a> Context<'a> {
    // asks every backend what it has installed, once
    fn load(env: &Env, runner: &'a dyn Runner, repo: &Repo) -> Result<Self, PackagesError> {
        let (registry, state) = edit::load(env, runner, repo)?;
        let backends: Vec<Box<dyn Backend + 'a>> = NAMES.iter().filter_map(|name| backend::for_name(name, runner, env)).collect();
        let installed = backends
            .iter()
            .map(|backend| Ok((backend.name().to_string(), backend.list()?.into_iter().flat_map(|pkg| [pkg.source, pkg.name]).collect())))
            .collect::<Result<_, BackendError>>()?;
        Ok(Context { backends, state, registry, installed })
    }

    fn backend(&self, name: &str) -> &dyn Backend {
        self.backends.iter().find(|backend| backend.name() == name).map(|backend| backend.as_ref()).unwrap()
    }

    fn is_installed(&self, target: &Target) -> bool {
        self.installed.get(&target.backend).is_some_and(|sources| sources.contains(spec_base(&target.spec)))
    }

    // the declared spec in this backend that a request names, by its base or its program name
    fn declared_as(&self, backend: &str, request: &str) -> Option<String> {
        let wanted = spec_base(request);
        declared(&self.state, backend).into_iter().find(|spec| spec_base(spec) == wanted || spec_program(spec) == wanted)
    }

    // the backend a bare request belongs to: already declared, xbps, then crates.io once the user agrees
    fn resolve(&self, request: &str, ask: Option<&dyn Ask>) -> Result<Target, PackagesError> {
        let target = |backend: &str, spec: &str| Target { backend: backend.into(), spec: spec.into() };
        if let Some((backend, spec)) = request.split_once(':').filter(|(backend, _)| NAMES.contains(backend)) {
            return Ok(target(backend, spec));
        }
        if let Some((backend, spec)) = NAMES.iter().find_map(|backend| Some((*backend, self.declared_as(backend, request)?))) {
            return Ok(target(backend, &spec));
        }
        if self.is_installed(&target("xbps", request)) || self.backend("xbps").info(request)?.is_some() {
            return Ok(target("xbps", request));
        }

        // crates.io only with a yes; without a terminal the user has to say cargo: themselves
        let Some(crate_found) = self.backend("cargo").info(request)? else {
            return Err(PackagesError::NotFound(request.into()));
        };
        if is_library(&crate_found) {
            return Err(PackagesError::Library(request.into()));
        }
        let question = format!("{request} isn't in xbps; install crate {} {} from crates.io? [Y/n] ", crate_found.name, crate_found.version);
        match ask.map(|ask| ask.ask(&question)) {
            None => Ok(target("cargo", request)),
            Some(Some(answer)) if !answer.trim().to_lowercase().starts_with('n') => Ok(target("cargo", request)),
            _ => Err(PackagesError::Unconfirmed(request.into())),
        }
    }

    // the target a remove request means: prefixed, declared, or installed somewhere
    fn find(&self, request: &str) -> Result<Target, PackagesError> {
        let target = |backend: &str, spec: &str| Target { backend: backend.into(), spec: spec.into() };
        if let Some((backend, spec)) = request.split_once(':').filter(|(backend, _)| NAMES.contains(backend)) {
            return Ok(target(backend, &self.declared_as(backend, spec).unwrap_or(spec.into())));
        }
        let declared = NAMES.iter().find_map(|backend| Some(target(backend, &self.declared_as(backend, request)?)));
        let installed = || NAMES.iter().map(|backend| target(backend, request)).find(|candidate| self.is_installed(candidate));
        declared.or_else(installed).ok_or_else(|| PackagesError::Unknown(request.into()))
    }
}

// what installing would do: resolve each request, install the missing, record the undeclared, scaffold known programs
pub fn plan_install(env: &Env, runner: &dyn Runner, repo: &Repo, requests: &[String], ask: Option<&dyn Ask>) -> Result<Change, PackagesError> {
    let context = Context::load(env, runner, repo)?;
    let targets = requests.iter().map(|request| context.resolve(request, ask)).collect::<Result<Vec<_>, _>>()?;
    let packages: Vec<Target> = targets.iter().filter(|target| !context.is_installed(target)).cloned().collect();

    // anything not installed yet has to exist where it's going to come from, and be a program
    packages.iter().try_for_each(|target| match context.backend(&target.backend).info(&target.spec)? {
        None => Err(PackagesError::NotFound(target.spec.clone())),
        Some(pkg) if is_library(&pkg) => Err(PackagesError::Library(target.spec.clone())),
        Some(_) => Ok(()),
    })?;

    // a program gets a module only when the registry knows it and the repo has nothing for it yet
    let scaffolded = targets
        .iter()
        .map(|target| spec_program(&target.spec))
        .filter(|program| context.registry.has(program) && !repo.module_file(program).exists() && !repo.static_dir().join(program).exists())
        .collect();
    let recorded = targets.iter().filter(|target| !declared(&context.state, &target.backend).contains(&target.spec)).cloned().collect();
    Ok(Change { packages, recorded, scaffolded })
}

// a package known to build no programs, which nothing can install
fn is_library(pkg: &Pkg) -> bool {
    pkg.programs.as_ref().is_some_and(|programs| programs.is_empty())
}

// installs, records in maw.nix, and scaffolds modules, as planned; activating afterwards is up to the caller
pub fn install(env: &Env, runner: &dyn Runner, repo: &Repo, change: &Change) -> Result<(), PackagesError> {
    grouped(&change.packages).into_iter().try_for_each(|(name, specs)| {
        let backend = backend::for_name(&name, runner, env).unwrap();
        backend.install(&specs)
    })?;

    let (_, mut state) = edit::load(env, runner, repo)?;
    if !change.recorded.is_empty() {
        change.recorded.iter().for_each(|target| state.packages.entry(target.backend.clone()).or_default().push(target.spec.clone()));
        state.write(&repo.maw_file())?;
    }
    change.scaffolded.iter().try_for_each(|program| edit::new_module(env, runner, repo, program, None).map(|_| ()))?;
    Ok(())
}

// specs per backend, in backend order
fn grouped(targets: &[Target]) -> BTreeMap<String, Vec<String>> {
    targets.iter().fold(BTreeMap::new(), |mut groups, target| {
        groups.entry(target.backend.clone()).or_insert_with(Vec::new).push(target.spec.clone());
        groups
    })
}

// what removing would do: remove the installed, drop the declared; modules are kept
pub fn plan_remove(env: &Env, runner: &dyn Runner, repo: &Repo, requests: &[String]) -> Result<Change, PackagesError> {
    let context = Context::load(env, runner, repo)?;
    let targets = requests.iter().map(|request| context.find(request)).collect::<Result<Vec<_>, _>>()?;
    let packages = targets.iter().filter(|target| context.is_installed(target)).cloned().collect();
    let recorded = targets.iter().filter(|target| declared(&context.state, &target.backend).contains(&target.spec)).cloned().collect();
    Ok(Change { packages, recorded, scaffolded: Vec::new() })
}

// removes the packages and drops them from maw.nix, as planned
pub fn remove(env: &Env, runner: &dyn Runner, repo: &Repo, change: &Change) -> Result<(), PackagesError> {
    grouped(&change.packages).into_iter().try_for_each(|(name, specs)| backend::for_name(&name, runner, env).unwrap().remove(&specs))?;
    if change.recorded.is_empty() {
        return Ok(());
    }

    // keep every declared spec that isn't being dropped, and drop lists left empty
    let (_, mut state) = edit::load(env, runner, repo)?;
    state.packages = state
        .packages
        .into_iter()
        .map(|(backend, specs)| {
            let kept: Vec<String> = specs.into_iter().filter(|spec| !change.recorded.contains(&Target { backend: backend.clone(), spec: spec.clone() })).collect();
            (backend, kept)
        })
        .filter(|(_, specs)| !specs.is_empty())
        .collect();
    state.write(&repo.maw_file())?;
    Ok(())
}

// upgrades unpinned cargo and go packages; xbps upgrades through its own sync
pub fn upgrade(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Vec<Target>, PackagesError> {
    let (_, state) = edit::load(env, runner, repo)?;
    let unpinned: Vec<Target> = ["cargo", "go"]
        .iter()
        .flat_map(|backend| declared(&state, backend).into_iter().map(|spec| Target { backend: backend.to_string(), spec }))
        .filter(|target| spec_base(&target.spec) == target.spec)
        .collect();
    grouped(&unpinned).into_iter().try_for_each(|(name, specs)| backend::for_name(&name, runner, env).unwrap().install(&specs))?;
    Ok(unpinned)
}

// one package in `maw query`: installed, declared, or both
#[derive(Debug, PartialEq)]
pub struct Row {
    pub target: Target,
    pub name: String,
    pub version: Option<String>,
    pub declared: bool,
}

// everything installed by hand or declared, per backend; declared-but-missing rows have no version
pub fn overview(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Vec<Row>, PackagesError> {
    let (_, state) = edit::load(env, runner, repo)?;
    let per_backend = NAMES
        .iter()
        .filter_map(|name| backend::for_name(name, runner, env))
        .map(|backend| {
            let specs = declared(&state, backend.name());
            let installed = backend.list()?;
            let target = |spec: &str| Target { backend: backend.name().into(), spec: spec.into() };
            let declared_spec = |source: &str| specs.iter().find(|spec| spec_base(spec) == source).cloned();

            // installed rows by hand or by declaration, then declared specs nothing matched
            let present = installed.iter().filter(|pkg| pkg.manual || declared_spec(&pkg.source).is_some()).map(|pkg| Row {
                target: target(&declared_spec(&pkg.source).unwrap_or(pkg.source.clone())),
                name: pkg.name.clone(),
                version: Some(pkg.version.clone()),
                declared: declared_spec(&pkg.source).is_some(),
            });
            let missing = specs.iter().filter(|spec| !installed.iter().any(|pkg| pkg.source == spec_base(spec)));
            let missing = missing.map(|spec| Row { target: target(spec), name: spec.clone(), version: None, declared: true });
            Ok(present.chain(missing).collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>, BackendError>>()?;
    Ok(per_backend.into_iter().flatten().collect())
}

// repo matches, each with whether it's installed: xbps, then crates.io only if xbps has none; a prefix picks one backend
pub fn search(env: &Env, runner: &dyn Runner, term: &str) -> Result<Vec<(Target, Pkg, bool)>, PackagesError> {
    if let Some((name, term)) = term.split_once(':').filter(|(name, _)| NAMES.contains(name)) {
        return Ok(search_in(backend::for_name(name, runner, env).unwrap().as_ref(), term)?);
    }
    let found = search_in(&Xbps::new(runner, env), term)?;
    if !found.is_empty() {
        return Ok(found);
    }
    Ok(search_in(&Cargo::new(runner, env), term)?)
}

// one backend's matches, marked installed by source
fn search_in(backend: &dyn Backend, term: &str) -> Result<Vec<(Target, Pkg, bool)>, BackendError> {
    let installed: HashSet<String> = backend.list()?.into_iter().map(|pkg| pkg.source).collect();
    let found = backend.search(term)?.into_iter().map(|pkg| {
        let target = Target { backend: backend.name().into(), spec: pkg.source.clone() };
        let is_installed = installed.contains(&pkg.source);
        (target, pkg, is_installed)
    });
    Ok(found.collect())
}

// what `maw info` shows: the package, where it comes from, and maw's view of it
pub struct Description {
    pub target: Target,
    pub pkg: Pkg,
    pub installed: Option<String>,
    pub declared: bool,
}

// finds a request among declared and installed packages first, then in the repos
pub fn describe(env: &Env, runner: &dyn Runner, repo: &Repo, request: &str) -> Result<Description, PackagesError> {
    let context = Context::load(env, runner, repo)?;
    let target = context.find(request).or_else(|_| context.resolve(request, None))?;
    let backend = context.backend(&target.backend);

    let installed = backend.list()?.into_iter().find(|pkg| pkg.source == spec_base(&target.spec) || pkg.name == target.spec);
    let declared = declared(&context.state, &target.backend).contains(&target.spec);

    // an undeclared package found by its installed name is looked up by its real source, like a git url
    let spec = if declared { target.spec } else { installed.as_ref().map_or(target.spec, |pkg| pkg.source.clone()) };
    let target = Target { spec, ..target };

    // go and git crates have no repo entry to describe, so what's installed says more than the placeholder
    let pkg = match (installed.clone(), backend.info(&target.spec)?) {
        (Some(installed), Some(repo)) if repo.description.is_empty() => installed,
        (_, Some(repo)) => repo,
        (Some(installed), None) => installed,
        (None, None) => return Err(PackagesError::NotFound(request.into())),
    };
    Ok(Description { target, pkg, installed: installed.map(|pkg| pkg.version), declared })
}

// bin dirs of backends that just installed something and aren't on the given PATH
pub fn off_path(env: &Env, runner: &dyn Runner, change: &Change, path_var: &str) -> Vec<PathBuf> {
    let on_path: Vec<PathBuf> = std::env::split_paths(path_var).collect();
    let backends: BTreeSet<&String> = change.packages.iter().map(|target| &target.backend).collect();
    backends
        .into_iter()
        .filter_map(|name| backend::for_name(name, runner, env)?.bin_dir())
        .filter(|dir| !on_path.contains(dir))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, fixture};
    use std::fs;

    struct Answer(&'static str);

    impl Ask for Answer {
        fn ask(&self, _: &str) -> Option<String> {
            Some(self.0.into())
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    fn target(backend: &str, spec: &str) -> Target {
        Target { backend: backend.into(), spec: spec.into() }
    }

    fn plan(fixture: &Fixture, requests: &[&str], ask: Option<&dyn Ask>) -> Result<Change, PackagesError> {
        plan_install(&fixture.env, &fixture.runner, &fixture.repo, &names(requests), ask)
    }

    fn apply(fixture: &Fixture, requests: &[&str]) {
        let change = plan(fixture, requests, None).unwrap();
        install(&fixture.env, &fixture.runner, &fixture.repo, &change).unwrap();
    }

    fn calls(fixture: &Fixture, program: &str) -> Vec<String> {
        fixture.runner.calls.borrow().iter().filter(|call| call.starts_with(program)).cloned().collect()
    }

    fn maw_nix(fixture: &Fixture) -> String {
        fs::read_to_string(fixture.repo.maw_file()).unwrap()
    }

    #[test]
    fn plan_installs_missing_records_undeclared_and_scaffolds_known_programs() {
        let fixture = fixture(&[]);
        let change = plan(&fixture, &["foot", "bash"], None).unwrap();
        assert_eq!(change.packages, [target("xbps", "foot")]);
        assert_eq!(change.recorded, [target("xbps", "foot"), target("xbps", "bash")]);
        assert_eq!(change.scaffolded, names(&["foot", "bash"]));
    }

    #[test]
    fn install_runs_xbps_writes_maw_nix_and_creates_the_module() {
        let fixture = fixture(&[]);
        apply(&fixture, &["foot"]);
        assert_eq!(calls(&fixture, "xbps-install"), [format!("xbps-install -r {} -y foot", fixture.env.sysroot.display())]);
        assert!(maw_nix(&fixture).contains(r#"xbps = [ "foot" ];"#));
        assert!(fixture.repo.module_file("foot").is_file());
    }

    #[test]
    fn existing_modules_are_left_alone() {
        let fixture = fixture(&["foot"]);
        assert!(plan(&fixture, &["foot"], None).unwrap().scaffolded.is_empty());
    }

    #[test]
    fn names_nowhere_are_not_found() {
        let fixture = fixture(&[]);
        assert!(matches!(plan(&fixture, &["foot", "nope"], None), Err(PackagesError::NotFound(name)) if name == "nope"));
    }

    #[test]
    fn crates_io_fallback_asks_first() {
        let fixture = fixture(&[]);
        assert_eq!(plan(&fixture, &["bat"], Some(&Answer("y"))).unwrap().packages, [target("cargo", "bat")]);
        assert!(matches!(plan(&fixture, &["bat"], Some(&Answer("n"))), Err(PackagesError::Unconfirmed(_))));
    }

    #[test]
    fn library_crates_are_refused_before_asking() {
        let fixture = fixture(&[]);
        assert!(matches!(plan(&fixture, &["libfoo"], Some(&Answer("y"))), Err(PackagesError::Library(_))));
        assert!(matches!(plan(&fixture, &["cargo:libfoo"], None), Err(PackagesError::Library(_))));
        assert!(plan(&fixture, &["cargo:bat"], None).is_ok());
    }

    #[test]
    fn prefixes_skip_resolution() {
        let fixture = fixture(&[]);
        let change = plan(&fixture, &["cargo:ripgrep@1.0", "go:github.com/jesseduffield/lazygit", "cargo:https://github.com/vitali87/croft.git"], None).unwrap();
        assert_eq!(change.packages.iter().map(|target| target.backend.as_str()).collect::<Vec<_>>(), ["cargo", "go", "cargo"]);
        assert!(!fixture.runner.calls.borrow().iter().any(|call| call.starts_with("xbps-query") && call.contains(" -R ")));
    }

    #[test]
    fn cargo_and_go_installs_record_under_their_backend() {
        let fixture = fixture(&[]);
        apply(&fixture, &["cargo:bat@1.0", "go:github.com/jesseduffield/lazygit"]);
        assert_eq!(calls(&fixture, "cargo install"), ["cargo install --list", "cargo install --locked bat@1.0"]);
        assert_eq!(calls(&fixture, "go install"), ["go install github.com/jesseduffield/lazygit@latest"]);
        assert!(maw_nix(&fixture).contains(r#"cargo = [ "bat@1.0" ];"#) && maw_nix(&fixture).contains(r#"go = [ "github.com/jesseduffield/lazygit" ];"#));
    }

    #[test]
    fn declared_packages_resolve_to_their_backend() {
        let fixture = fixture(&[]);
        apply(&fixture, &["cargo:bat"]);
        assert_eq!(plan(&fixture, &["bat"], None).unwrap().recorded, []);
    }

    #[test]
    fn remove_finds_the_backend_and_drops_the_record() {
        let fixture = fixture(&[]);
        apply(&fixture, &["foot", "cargo:bat", "go:github.com/jesseduffield/lazygit"]);

        let change = plan_remove(&fixture.env, &fixture.runner, &fixture.repo, &names(&["bat", "lazygit"])).unwrap();
        assert_eq!(change.recorded, [target("cargo", "bat"), target("go", "github.com/jesseduffield/lazygit")]);
        remove(&fixture.env, &fixture.runner, &fixture.repo, &change).unwrap();
        assert!(!maw_nix(&fixture).contains("cargo") && !maw_nix(&fixture).contains("go ="));
        assert!(maw_nix(&fixture).contains(r#"xbps = [ "foot" ];"#));
    }

    #[test]
    fn removing_the_unknown_fails() {
        let fixture = fixture(&[]);
        let result = plan_remove(&fixture.env, &fixture.runner, &fixture.repo, &names(&["foot"]));
        assert!(matches!(result, Err(PackagesError::Unknown(_))));
    }

    #[test]
    fn upgrade_reinstalls_only_unpinned_cargo_and_go() {
        let fixture = fixture(&[]);
        apply(&fixture, &["foot", "cargo:bat", "cargo:ripgrep@1.0"]);
        fixture.runner.calls.borrow_mut().clear();

        assert_eq!(upgrade(&fixture.env, &fixture.runner, &fixture.repo).unwrap(), [target("cargo", "bat")]);
        assert_eq!(calls(&fixture, "cargo install --locked"), ["cargo install --locked bat"]);
    }

    #[test]
    fn overview_flags_missing_declared_packages() {
        let fixture = fixture(&[]);
        apply(&fixture, &["cargo:bat"]);
        let rows = overview(&fixture.env, &fixture.runner, &fixture.repo).unwrap();
        assert!(rows.contains(&Row { target: target("xbps", "bash"), name: "bash".into(), version: Some("5.2_1".into()), declared: false }));
        assert!(rows.contains(&Row { target: target("cargo", "bat"), name: "bat".into(), version: None, declared: true }));
    }

    #[test]
    fn search_falls_back_to_crates_io_only_when_xbps_has_nothing() {
        let fixture = fixture(&[]);
        let backends = |term: &str| -> Vec<String> {
            search(&fixture.env, &fixture.runner, term).unwrap().into_iter().map(|(target, ..)| target.to_string()).collect()
        };
        assert_eq!(backends("foot"), ["foot"]);
        assert_eq!(backends("bat"), ["cargo:bat"]);
        assert_eq!(backends("cargo:ripgrep"), ["cargo:ripgrep"]);
    }

    #[test]
    fn bin_dirs_off_path_are_reported() {
        let fixture = fixture(&[]);
        let change = Change { packages: vec![target("cargo", "bat"), target("xbps", "foot")], ..Change::default() };
        let cargo_bin = fixture.env.home.join(".cargo/bin");
        assert_eq!(off_path(&fixture.env, &fixture.runner, &change, "/usr/bin"), std::slice::from_ref(&cargo_bin));
        assert!(off_path(&fixture.env, &fixture.runner, &change, &format!("/usr/bin:{}", cargo_bin.display())).is_empty());
    }
}
