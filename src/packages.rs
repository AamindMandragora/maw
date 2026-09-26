use crate::activate::Ask;
use crate::backend::flatpak::{self, Flatpak};
use crate::backend::srcpkgs::SrcPkgs;
use crate::build::{self, BuildError};
use crate::backend::xbps::Xbps;
use crate::backend::{self, Backend, BackendError, NAMES, Pkg, newer, spec_base, spec_program};
use crate::edit::{self, EditError};
use crate::env::Env;
use crate::registry::Registry;
use crate::repo::Repo;
use crate::runner::{RunError, Runner};
use crate::scaffold::{aur, nixos};
use crate::state::{MawState, StateError};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum PackagesError {
    #[error("no package {0}; `maw search {0}` to look for it")]
    NotFound(String),
    #[error("{name} isn't in xbps; `maw install {}` to install it from there", options.join("` or `maw install "))]
    Unconfirmed { name: String, options: Vec<String> },
    #[error("{name} isn't in any source maw installs from; draft a template with `maw src new {name} {flag}`")]
    Draftable { name: String, flag: String, options: Vec<(Target, Pkg)> },
    #[error("{name} is a library, not a program; add it to a project with `{add} {name}`")]
    Library { name: String, add: &'static str },
    #[error("{program} isn't installed; `maw install {package}` first")]
    MissingTool { program: &'static str, package: &'static str },
    #[error("{0} is neither installed nor declared")]
    Unknown(String),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error(transparent)]
    Edit(#[from] EditError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error("no srcpkgs/{0}/template; create one with `maw src new {0}`")]
    NoTemplate(String),
}

// the source builds for this repo: its srcpkgs/ templates, built in the configured void-packages clone
pub fn srcpkgs<'a>(env: &Env, runner: &'a dyn Runner, repo: &Repo) -> Result<SrcPkgs<'a>, PackagesError> {
    let settings = build::settings(env, runner, repo)?;
    Ok(SrcPkgs::new(runner, env, &repo.srcpkgs_dir(), &settings.void_packages(env)))
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
    // record for this machine only
    pub here: bool,
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
    env: Env,
    runner: &'a dyn Runner,
    backends: Vec<Box<dyn Backend + 'a>>,
    state: MawState,
    registry: Registry,
    installed: BTreeMap<String, HashSet<String>>,
    src: SrcPkgs<'a>,
}

impl<'a> Context<'a> {
    // asks every backend what it has installed, once
    fn load(env: &Env, runner: &'a dyn Runner, repo: &Repo) -> Result<Self, PackagesError> {
        let (registry, state) = edit::load(env, runner, repo)?;
        let state = state.here(&env.host);
        let backends: Vec<Box<dyn Backend + 'a>> = NAMES.iter().filter_map(|name| backend::for_name(name, runner, env)).collect();
        let installed = backends
            .iter()
            .map(|backend| Ok((backend.name().to_string(), backend.list()?.into_iter().flat_map(|pkg| [pkg.source, pkg.name]).collect())))
            .collect::<Result<_, BackendError>>()?;
        Ok(Context { env: env.clone(), runner, backends, state, registry, installed, src: srcpkgs(env, runner, repo)? })
    }

    fn backend(&self, name: &str) -> &dyn Backend {
        self.backends.iter().find(|backend| backend.name() == name).map(|backend| backend.as_ref()).unwrap()
    }

    fn is_installed(&self, target: &Target) -> bool {
        self.installed.get(&target.backend).is_some_and(|sources| sources.contains(spec_base(&target.spec)))
    }

    // the declared spec in this backend that a request names, by its base or its program name (a flatpak's short name)
    fn declared_as(&self, backend: &str, request: &str) -> Option<String> {
        let wanted = spec_base(request);
        let program = |spec: &str| if backend == "flatpak" { flatpak::short_name(spec) } else { spec_program(spec) };
        declared(&self.state, backend).into_iter().find(|spec| spec_base(spec) == wanted || program(spec) == wanted)
    }

    // the backend a bare request belongs to: already declared, a flatpak app id, a srcpkgs template (built for xbps), xbps, then crates.io once the user agrees
    fn resolve(&self, request: &str, ask: Option<&dyn Ask>) -> Result<Target, PackagesError> {
        let target = |backend: &str, spec: &str| Target { backend: backend.into(), spec: spec.into() };
        if let Some((backend, spec)) = request.split_once(':').filter(|(backend, _)| NAMES.contains(backend)) {
            return Ok(target(backend, spec));
        }
        if let Some((backend, spec)) = NAMES.iter().find_map(|backend| Some((*backend, self.declared_as(backend, request)?))) {
            return Ok(target(backend, &spec));
        }
        if let Some((source, name)) = request.split_once(':').filter(|(source, _)| DRAFTS.contains(source)) {
            return Err(self.draftable(name, &[source]).unwrap_or_else(|| PackagesError::NotFound(request.into())));
        }
        if flatpak::is_app_id(request) {
            return match self.backend("flatpak").info(request)? {
                Some(_) => Ok(target("flatpak", request)),
                None => Err(PackagesError::NotFound(request.into())),
            };
        }
        if self.src.has(request) || self.is_installed(&target("xbps", request)) || self.backend("xbps").info(request)?.is_some() {
            return Ok(target("xbps", request));
        }

        // elsewhere, only with a yes; a dry run (no one to ask) shows the first
        let candidates = self.candidates(request)?;
        let Some(first) = candidates.first() else {
            return Err(self.draftable(request, &DRAFTS).unwrap_or_else(|| PackagesError::NotFound(request.into())));
        };
        let Some(ask) = ask else {
            return Ok(first.0.clone());
        };
        let refused = || PackagesError::Unconfirmed { name: request.into(), options: candidates.iter().map(|(target, _)| target.to_string()).collect() };
        let answer = ask.ask(&choice_question(request, "isn't in xbps; install", &candidates)).ok_or_else(refused)?;
        pick(&answer, &candidates).map(|(target, _)| target.clone()).ok_or_else(refused)
    }

    // exact matches for a name outside xbps, in source order; libraries are left out, and an error if they're all there is
    fn candidates(&self, name: &str) -> Result<Vec<(Target, Pkg)>, PackagesError> {
        let found = |backend: &str| -> Option<Pkg> {
            match backend {
                "flatpak" => self.backend(backend).search(name).ok()?.into_iter().find(|pkg| flatpak::short_name(&pkg.source) == name.to_lowercase()),
                "cargo" | "uv" | "npm" => self.backend(backend).info(name).ok().flatten(),
                _ => None,
            }
        };
        let matches: Vec<(Target, Pkg)> = NAMES.iter().filter_map(|backend| found(backend).map(|pkg| (Target { backend: backend.to_string(), spec: pkg.source.clone() }, pkg))).collect();
        let (libraries, programs): (Vec<_>, Vec<_>) = matches.into_iter().partition(|(_, pkg)| is_library(pkg));
        match (programs.is_empty(), libraries.first()) {
            (true, Some((target, _))) => Err(library(&target.backend, name)),
            _ => Ok(programs),
        }
    }

    // the upstreams among these that a template for this name could be drafted from, as an error to offer them
    fn draftable(&self, name: &str, sources: &[&str]) -> Option<PackagesError> {
        let exact = |source: &&str| -> Option<Pkg> {
            match *source {
                "aur" => aur::find(self.runner, name),
                _ => nixos::search(&self.env, self.runner, name).ok()?.into_iter().find(|pkg| pkg.name == name),
            }
        };
        let options: Vec<(Target, Pkg)> = sources.iter().filter_map(|source| exact(source).map(|pkg| (Target { backend: source.to_string(), spec: pkg.source.clone() }, pkg))).collect();
        let flag = options.first().map(|(target, _)| draft_flag(name, target))?;
        Some(PackagesError::Draftable { name: name.into(), flag, options })
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

// the src new flag that drafts a name from an upstream, naming the upstream package only when it differs
pub fn draft_flag(name: &str, target: &Target) -> String {
    let flag = if target.backend == "aur" { "--from-aur" } else { "--from-nix" };
    if target.spec == name { flag.to_string() } else { format!("{flag} {}", target.spec) }
}

// a numbered choice among targets, or a yes/no when there's one
pub fn choice_question(name: &str, verb: &str, options: &[(Target, Pkg)]) -> String {
    if let [(target, pkg)] = options {
        return format!("{name} {verb} {target} {}? [Y/n] ", pkg.version);
    }
    let lines: String = options.iter().enumerate().map(|(index, (target, pkg))| format!("  {}  {target} {}  {}\n", index + 1, pkg.version, pkg.description)).collect();
    format!("{name} {verb} one of:\n{lines}which? [1] ")
}

// the option an answer picks: blank or yes is the first, a number picks one, no or anything else is none
pub fn pick<'o>(answer: &str, options: &'o [(Target, Pkg)]) -> Option<&'o (Target, Pkg)> {
    match answer.trim().to_lowercase().as_str() {
        "" | "y" | "yes" => options.first(),
        number => options.get(number.parse::<usize>().ok()?.checked_sub(1)?),
    }
}

// the error for a package that builds no program, with how to use it instead
fn library(backend: &str, name: &str) -> PackagesError {
    let add = if backend == "npm" { "npm install" } else { "cargo add" };
    PackagesError::Library { name: name.into(), add }
}

// the first backend whose tool isn't installed and isn't being installed from xbps alongside
fn missing_tool(runner: &dyn Runner, context: &Context, packages: &[Target]) -> Option<PackagesError> {
    let backends: BTreeSet<&str> = packages.iter().map(|target| target.backend.as_str()).collect();
    let coming = |package: &str| packages.iter().any(|target| target.backend == "xbps" && target.spec == package);
    let tools = backends.into_iter().filter_map(|name| context.backend(name).tool());
    tools
        .filter(|(program, package)| !coming(package) && matches!(runner.run(program, &["--version".into()]), Err(RunError::Spawn { .. })))
        .map(|(program, package)| PackagesError::MissingTool { program, package })
        .next()
}

// what installing would do: resolve each request, install the missing, record the undeclared, scaffold known programs
pub fn plan_install(env: &Env, runner: &dyn Runner, repo: &Repo, requests: &[String], ask: Option<&dyn Ask>) -> Result<Change, PackagesError> {
    let context = Context::load(env, runner, repo)?;
    let targets = requests.iter().map(|request| context.resolve(request, ask)).collect::<Result<Vec<_>, _>>()?;
    let packages: Vec<Target> = targets.iter().filter(|target| !context.is_installed(target)).cloned().collect();
    if let Some(error) = missing_tool(runner, &context, &packages) {
        return Err(error);
    }

    // anything not installed yet has to exist where it's going to come from, and be a program; a template is its own source
    let from_repos = packages.iter().filter(|target| !(target.backend == "xbps" && context.src.has(&target.spec)));
    from_repos.into_iter().try_for_each(|target| match context.backend(&target.backend).info(&target.spec)? {
        None => Err(PackagesError::NotFound(target.spec.clone())),
        Some(pkg) if is_library(&pkg) => Err(library(&target.backend, &target.spec)),
        Some(_) => Ok(()),
    })?;

    // a program gets a module only when the registry knows it and the repo has nothing for it yet
    let scaffolded = targets
        .iter()
        .map(|target| spec_program(&target.spec))
        .filter(|program| context.registry.has(program) && !repo.module_file(program).exists() && !repo.static_dir().join(program).exists())
        .collect();
    let recorded = targets.iter().filter(|target| !declared(&context.state, &target.backend).contains(&target.spec)).cloned().collect();
    Ok(Change { packages, recorded, scaffolded, here: false })
}

// a package known to build no programs, which nothing can install
fn is_library(pkg: &Pkg) -> bool {
    pkg.programs.as_ref().is_some_and(|programs| programs.is_empty())
}

// installs (building srcpkgs templates first), records in maw.nix, and scaffolds modules, as planned; activating afterwards is up to the caller
pub fn install(env: &Env, runner: &dyn Runner, repo: &Repo, change: &Change) -> Result<(), PackagesError> {
    install_targets(env, runner, repo, &change.packages)?;

    let (_, mut state) = edit::load(env, runner, repo)?;
    if !change.recorded.is_empty() {
        // for every machine, or with here this one only
        let lists = if change.here { &mut state.own(&env.host).packages } else { &mut state.packages };
        change.recorded.iter().for_each(|target| lists.entry(target.backend.clone()).or_default().push(target.spec.clone()));
        state.write(&repo.maw_file())?;
    }
    change.scaffolded.iter().try_for_each(|program| edit::new_module(env, runner, repo, program, None).map(|_| ()))?;
    Ok(())
}

// installs targets per backend; xbps names with a srcpkgs template are built and installed from binpkgs instead
pub fn install_targets(env: &Env, runner: &dyn Runner, repo: &Repo, targets: &[Target]) -> Result<(), PackagesError> {
    install_with(env, runner, &srcpkgs(env, runner, repo)?, targets)
}

// install_targets with the source builds already located, so nothing needs nix to find the clone
pub fn install_with(env: &Env, runner: &dyn Runner, src: &SrcPkgs, targets: &[Target]) -> Result<(), PackagesError> {
    let (built, fetched): (Vec<Target>, Vec<Target>) = targets.iter().cloned().partition(|target| target.backend == "xbps" && src.has(&target.spec));
    if !built.is_empty() {
        src.install(&built.into_iter().map(|target| target.spec).collect::<Vec<_>>(), false)?;
    }
    Ok(grouped(&fetched).into_iter().try_for_each(|(name, specs)| backend::for_name(&name, runner, env).unwrap().install(&specs))?)
}

// rebuilds a srcpkgs template; an installed package is reinstalled at the new build. Returns whether it was
pub fn build_source(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str) -> Result<bool, PackagesError> {
    let src = srcpkgs(env, runner, repo)?;
    if !src.has(name) {
        return Err(PackagesError::NoTemplate(name.into()));
    }
    let installed = Xbps::new(runner, env).list()?.iter().any(|pkg| pkg.name == name);
    match installed {
        true => src.install(&[name.to_string()], true)?,
        false => src.build(&[name.to_string()])?,
    }
    Ok(installed)
}

// declared source packages a build would change, as (name, installed, what a build makes): a template at another
// version, or patches not yet built into void's package or behind void's newer release
pub fn outdated(env: &Env, runner: &dyn Runner, repo: &Repo, state: &MawState) -> Result<Vec<(String, String, String)>, PackagesError> {
    let src = srcpkgs(env, runner, repo)?;
    let xbps = Xbps::new(runner, env);
    let installed = xbps.list()?;
    let stale = declared(state, "xbps").into_iter().filter_map(|name| {
        let current = installed.iter().find(|pkg| pkg.name == name)?.version.clone();
        if !src.patches(&name) {
            let template = src.version(&name)?;
            return (current != template).then_some((name, current, template));
        }
        let shipped = xbps.info(&name).ok().flatten().map_or(current.clone(), |pkg| pkg.version);
        (!src.is_ours(&name) || newer(&shipped, &current)).then(|| (name, current, format!("{shipped} + patches")))
    });
    Ok(stale.collect())
}

// specs per backend, in NAMES order, so xbps goes first and can bring the tools the others need
fn grouped(targets: &[Target]) -> Vec<(String, Vec<String>)> {
    let groups = targets.iter().fold(BTreeMap::new(), |mut groups: BTreeMap<String, Vec<String>>, target| {
        groups.entry(target.backend.clone()).or_default().push(target.spec.clone());
        groups
    });
    let mut ordered: Vec<(String, Vec<String>)> = groups.into_iter().collect();
    ordered.sort_by_key(|(name, _)| NAMES.iter().position(|known| known == name));
    ordered
}

// what removing would do: remove the installed, drop the declared; modules are kept
pub fn plan_remove(env: &Env, runner: &dyn Runner, repo: &Repo, requests: &[String]) -> Result<Change, PackagesError> {
    let context = Context::load(env, runner, repo)?;
    let targets = requests.iter().map(|request| context.find(request)).collect::<Result<Vec<_>, _>>()?;
    let packages = targets.iter().filter(|target| context.is_installed(target)).cloned().collect();
    let recorded = targets.iter().filter(|target| declared(&context.state, &target.backend).contains(&target.spec)).cloned().collect();
    Ok(Change { packages, recorded, scaffolded: Vec::new(), here: false })
}

// removes the packages and drops them from maw.nix, as planned
pub fn remove(env: &Env, runner: &dyn Runner, repo: &Repo, change: &Change) -> Result<(), PackagesError> {
    grouped(&change.packages).into_iter().try_for_each(|(name, specs)| backend::for_name(&name, runner, env).unwrap().remove(&specs))?;
    if change.recorded.is_empty() {
        return Ok(());
    }

    // dropped from the shared lists and this machine's own
    let (_, mut state) = edit::load(env, runner, repo)?;
    change.recorded.iter().for_each(|target| state.forget(&env.host, false, &target.backend, &target.spec));
    state.write(&repo.maw_file())?;
    Ok(())
}

// upgrades every unpinned package but xbps's, which upgrade through its own sync
pub fn upgrade(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Vec<Target>, PackagesError> {
    let (_, state) = edit::load(env, runner, repo)?;
    let state = state.here(&env.host);
    let unpinned: Vec<Target> = ["flatpak", "cargo", "go", "uv", "npm"]
        .iter()
        .flat_map(|backend| declared(&state, backend).into_iter().map(|spec| Target { backend: backend.to_string(), spec }))
        .filter(|target| spec_base(&target.spec) == target.spec)
        .collect();

    // flatpak updates in place; the rest upgrade by installing again
    grouped(&unpinned).into_iter().try_for_each(|(name, specs)| match name.as_str() {
        "flatpak" => Flatpak::new(runner, env).update(&specs),
        _ => backend::for_name(&name, runner, env).unwrap().install(&specs),
    })?;
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
    let state = state.here(&env.host);
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

// what search found: matches with whether each is installed, and sources that didn't answer
#[derive(Debug, Default)]
pub struct Found {
    pub hits: Vec<(Target, Pkg, bool)>,
    pub unreachable: Vec<String>,
}

// upstreams maw drafts templates from, searched when nothing installable matches
pub const DRAFTS: [&str; 2] = ["aur", "nixpkgs"];

// matches in every source maw installs from, xbps first; when there are none, in the aur and nixpkgs for drafting.
// a prefix searches one source. Sources whose tool is missing or that fail are skipped
pub fn search(env: &Env, runner: &dyn Runner, term: &str) -> Result<Found, PackagesError> {
    match term.split_once(':') {
        Some((name, term)) if NAMES.contains(&name) => return Ok(Found { hits: search_in(backend::for_name(name, runner, env).unwrap().as_ref(), term)?, ..Found::default() }),
        Some((name, term)) if DRAFTS.contains(&name) => return Ok(search_drafts(env, runner, term, &[name])),
        _ => {}
    }

    // xbps in full, the others' best ten each
    let xbps = search_in(&Xbps::new(runner, env), term)?;
    let others = NAMES[1..].iter().filter_map(|name| backend::for_name(name, runner, env)).flat_map(|backend| search_in(backend.as_ref(), term).unwrap_or_default().into_iter().take(10));
    let hits: Vec<(Target, Pkg, bool)> = xbps.into_iter().chain(others).collect();
    if !hits.is_empty() {
        return Ok(Found { hits, ..Found::default() });
    }
    Ok(search_drafts(env, runner, term, &DRAFTS))
}

// matches in the aur and nixpkgs, never installed; the ones that didn't answer are named
fn search_drafts(env: &Env, runner: &dyn Runner, term: &str, sources: &[&str]) -> Found {
    let searched = sources.iter().map(|source| {
        let found = match *source {
            "aur" => aur::search(runner, term),
            _ => nixos::search(env, runner, term),
        };
        (source, found)
    });
    searched.fold(Found::default(), |mut found, (source, result)| {
        match result {
            Ok(pkgs) => found.hits.extend(pkgs.into_iter().map(|pkg| (Target { backend: source.to_string(), spec: pkg.source.clone() }, pkg, false))),
            Err(_) => found.unreachable.push(source.to_string()),
        }
        found
    })
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
    use crate::runner::fake::FakeRunner;
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
    fn app_ids_are_flatpaks() {
        let fixture = fixture(&[]);
        assert_eq!(plan(&fixture, &["com.slack.Slack"], None).unwrap().packages, [target("flatpak", "com.slack.Slack")]);
    }

    #[test]
    fn a_missing_tool_stops_the_plan_unless_xbps_brings_it() {
        let mut fixture = fixture(&[]);
        fixture.runner = FakeRunner::fallible(|program, args| match program {
            "uv" => Err(RunError::Spawn { program: "uv".into(), source: std::io::ErrorKind::NotFound.into() }),
            "curl" => Ok(r#"{"info":{"name":"ruff","version":"0.6.9"}}"#.into()),
            "xbps-query" if args.last().is_some_and(|name| name == "uv") => Ok("pkgver: uv-0.12.9_1\n".into()),
            _ => crate::testing::fake(program, args),
        });
        assert!(matches!(plan(&fixture, &["uv:ruff"], None), Err(PackagesError::MissingTool { program: "uv", package: "uv" })));

        // uv from xbps in the same install is fine, and goes first
        let change = plan(&fixture, &["uv:ruff", "xbps:uv"], None).unwrap();
        let order: Vec<String> = grouped(&change.packages).into_iter().map(|(backend, _)| backend).collect();
        assert_eq!(order, ["xbps", "uv"]);
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
        assert!(matches!(plan(&fixture, &["bat"], Some(&Answer("n"))), Err(PackagesError::Unconfirmed { .. })));
    }

    #[test]
    fn several_sources_are_a_numbered_choice() {
        let option = |backend: &str| (target(backend, "prettier"), Pkg { version: "1.0".into(), description: "formats".into(), ..Pkg::default() });
        let options = [option("npm"), option("cargo")];
        assert_eq!(choice_question("prettier", "isn't in xbps; install", &options), "prettier isn't in xbps; install one of:\n  1  npm:prettier 1.0  formats\n  2  cargo:prettier 1.0  formats\nwhich? [1] ");
        assert_eq!(choice_question("prettier", "isn't in xbps; install", &options[..1]), "prettier isn't in xbps; install npm:prettier 1.0? [Y/n] ");
        assert_eq!((pick("", &options).unwrap().0.backend.as_str(), pick("2", &options).unwrap().0.backend.as_str()), ("npm", "cargo"));
        assert!(pick("n", &options).is_none() && pick("3", &options).is_none());
    }

    #[test]
    fn library_crates_are_refused_before_asking() {
        let fixture = fixture(&[]);
        assert!(matches!(plan(&fixture, &["libfoo"], Some(&Answer("y"))), Err(PackagesError::Library { .. })));
        assert!(matches!(plan(&fixture, &["cargo:libfoo"], None), Err(PackagesError::Library { add: "cargo add", .. })));
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
    fn search_looks_everywhere_and_drafts_only_when_nothing_installs() {
        let fixture = fixture(&[]);
        let backends = |term: &str| -> Vec<String> {
            search(&fixture.env, &fixture.runner, term).unwrap().hits.into_iter().map(|(target, ..)| target.to_string()).collect()
        };
        assert_eq!(backends("foot"), ["foot"]);
        assert_eq!(backends("bat"), ["cargo:bat"]);
        assert_eq!(backends("cargo:ripgrep"), ["cargo:ripgrep"]);

        // nothing installable: the aur and nixpkgs are asked, and one that doesn't answer is named
        let found = search(&fixture.env, &fixture.runner, "nothing").unwrap();
        assert!(found.hits.is_empty());
        assert_eq!(found.unreachable, ["nixpkgs"]);
    }

    // a srcpkgs template in the repo and a clone that's already set up, so nothing is ever really cloned
    fn template(fixture: &Fixture, name: &str, version: &str) -> PathBuf {
        let clone = fixture.env.home.join(".local/share/maw/void-packages");
        crate::testing::write(&clone.join("xbps-src"), "");
        let (version, revision) = version.split_once('_').unwrap();
        crate::testing::write(&fixture.repo.srcpkgs_dir().join(name).join("template"), &format!("pkgname={name}\nversion={version}\nrevision={revision}\n"));
        clone
    }

    #[test]
    fn a_template_makes_a_bare_name_a_source_build() {
        let fixture = fixture(&[]);
        let clone = template(&fixture, "hello", "0.1_1");
        apply(&fixture, &["hello"]);

        let xbps_src = clone.join("xbps-src").display().to_string();
        assert!(fixture.runner.calls.borrow().contains(&format!("{xbps_src} pkg hello")));
        let root = fixture.env.sysroot.display();
        assert_eq!(calls(&fixture, "xbps-install"), [format!("xbps-install -r {root} -R {} -y hello", clone.join("hostdir/binpkgs").display())]);
        assert!(maw_nix(&fixture).contains(r#"xbps = [ "hello" ];"#));
    }

    #[test]
    fn a_declared_source_package_registers_the_local_repo() {
        let fixture = fixture(&[]);
        let clone = template(&fixture, "hello", "0.1_1");
        let conf = fixture.env.sysroot.join("etc/xbps.d/10-maw-local.conf");
        apply(&fixture, &["foot"]);
        crate::activate::activate(&fixture.env, &fixture.runner, &fixture.repo, &Answer("n"), Default::default()).unwrap();
        assert!(!conf.exists());

        apply(&fixture, &["hello"]);
        crate::activate::activate(&fixture.env, &fixture.runner, &fixture.repo, &Answer("n"), Default::default()).unwrap();
        assert_eq!(fs::read_to_string(&conf).unwrap(), format!("# written by maw: packages built from srcpkgs/\nrepository={}\n", clone.join("hostdir/binpkgs").display()));
    }

    #[test]
    fn templates_ahead_of_what_is_installed_are_outdated() {
        let fixture = fixture(&[]);
        template(&fixture, "bash", "5.3_1");
        apply(&fixture, &["bash"]);
        let (_, state) = edit::load(&fixture.env, &fixture.runner, &fixture.repo).unwrap();
        let stale = outdated(&fixture.env, &fixture.runner, &fixture.repo, &state).unwrap();
        assert_eq!(stale, [("bash".to_string(), "5.2_1".to_string(), "5.3_1".to_string())]);
    }

    #[test]
    fn patches_are_outdated_until_built_into_the_installed_package() {
        let fixture = fixture(&[]);
        crate::testing::write(&fixture.repo.srcpkgs_dir().join("bash/patches/fix.patch"), "");
        let mut state = MawState::default();
        state.packages.insert("xbps".into(), vec!["bash".into()]);

        // bash is installed from void's repos, so the patches aren't in it yet
        let stale = outdated(&fixture.env, &fixture.runner, &fixture.repo, &state).unwrap();
        assert_eq!(stale, [("bash".to_string(), "5.2_1".to_string(), "1.0_1 + patches".to_string())]);
    }

    #[test]
    fn building_upgrades_only_what_is_installed() {
        let fixture = fixture(&[]);
        template(&fixture, "bash", "5.3_1");
        template(&fixture, "hello", "0.1_1");
        assert!(build_source(&fixture.env, &fixture.runner, &fixture.repo, "bash").unwrap());
        assert!(!build_source(&fixture.env, &fixture.runner, &fixture.repo, "hello").unwrap());
        assert_eq!(calls(&fixture, "xbps-install").len(), 1);
        assert!(calls(&fixture, "xbps-install")[0].ends_with("-fy bash"));
        assert!(matches!(build_source(&fixture.env, &fixture.runner, &fixture.repo, "nope"), Err(PackagesError::NoTemplate(_))));
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
