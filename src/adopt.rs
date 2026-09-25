use crate::backend::{self, BackendError, NAMES, spec_base};
use crate::build::{self, BuildError, Options};
use crate::env::Env;
use crate::init::runit::Runit;
use crate::init::{InitBackend, Scope};
use crate::repo::Repo;
use crate::runner::Runner;
use crate::state::{MawState, StateError};
use crate::system;
use std::collections::BTreeMap;
use std::fs;

#[derive(Debug, thiserror::Error)]
pub enum AdoptError {
    #[error("adopt checklist, line {line}: not keep or skip: {text}")]
    Line { line: usize, text: String },
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error(transparent)]
    State(#[from] StateError),
}

// something installed or enabled that maw.nix doesn't mention: a package in a backend, or a service in a scope
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Candidate {
    Package { backend: String, spec: String },
    Service { scope: Scope, name: String },
}

impl Candidate {
    // the checklist section it's listed under
    fn section(&self) -> String {
        match self {
            Candidate::Package { backend, .. } => format!("packages ({backend})"),
            Candidate::Service { scope, .. } => format!("services ({scope})"),
        }
    }

    // xbps, cargo, go, then system and user services, names alphabetical within each
    fn order(&self) -> (usize, String) {
        let rank = match self {
            Candidate::Package { backend, .. } => NAMES.iter().position(|name| name == backend).unwrap_or(NAMES.len()),
            Candidate::Service { scope, .. } => NAMES.len() + 1 + (*scope == Scope::User) as usize,
        };
        (rank, self.name().to_lowercase())
    }

    pub fn name(&self) -> &str {
        match self {
            Candidate::Package { spec, .. } => spec,
            Candidate::Service { name, .. } => name,
        }
    }
}

// services turnstile puts in every user's service dir itself
const TURNSTILE: [&str; 1] = ["turnstile-ready"];

// true when a maw.nix list (declared or ignored) already names this, by spec or source
fn listed(lists: &BTreeMap<String, Vec<String>>, key: &str, name: &str) -> bool {
    lists.get(key).is_some_and(|names| names.iter().any(|listed| listed == name || spec_base(listed) == name))
}

// everything installed by hand or enabled that maw.nix neither declares nor ignores
pub fn candidates(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Vec<Candidate>, AdoptError> {
    let build = build::build(env, runner, repo, Options { dry_run: true, ..Options::default() })?;
    let state = &build.state;

    // packages installed by hand, per backend
    let packages = NAMES
        .iter()
        .filter_map(|name| backend::for_name(name, runner, env))
        .map(|backend| {
            let name = backend.name().to_string();
            let manual = backend.list()?.into_iter().filter(|pkg| pkg.manual).map(|pkg| pkg.source);
            let unlisted = manual.filter(|source| !listed(&state.packages, &name, source) && !listed(&state.ignored.packages, &name, source));
            Ok(unlisted.map(|spec| Candidate::Package { backend: name.clone(), spec }).collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>, BackendError>>()?;

    // services linked into either supervised dir
    let declared = system::declared_services(&build);
    let services = [Scope::System, Scope::User].into_iter().flat_map(|scope| {
        let dir = Runit.enabled_link(env, scope, "_").parent().unwrap().to_path_buf();
        let names: Vec<String> = fs::read_dir(dir).into_iter().flatten().filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned())).collect();
        let ignored = |name: &String| listed(&state.ignored.services, &scope.to_string(), name) || TURNSTILE.contains(&name.as_str());
        let unlisted: Vec<String> = names.into_iter().filter(|name| !declared.contains(&(scope, name.clone())) && !ignored(name)).collect();
        unlisted.into_iter().map(move |name| Candidate::Service { scope, name })
    });

    let mut all: Vec<Candidate> = packages.into_iter().flatten().chain(services).collect();
    all.sort_by_key(Candidate::order);
    Ok(all)
}

// the file the user edits: sections of `keep <name>` lines
pub fn checklist(candidates: &[Candidate]) -> String {
    let header = "# maw adopt: `keep` records it in maw.nix, `skip` (or deleting the line) ignores it from now on\n";
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    candidates.iter().for_each(|candidate| match sections.last_mut() {
        Some((section, names)) if *section == candidate.section() => names.push(candidate.name()),
        _ => sections.push((candidate.section(), vec![candidate.name()])),
    });
    let body: String = sections.iter().map(|(section, names)| format!("\n# {section}\n{}", names.iter().map(|name| format!("keep {name}\n")).collect::<String>())).collect();
    format!("{header}{body}")
}

// each candidate with whether the edited checklist keeps it; lines the user deleted count as skipped
pub fn parse(text: &str, candidates: &[Candidate]) -> Result<Vec<(Candidate, bool)>, AdoptError> {
    let mut section = String::new();
    let mut kept: Vec<(String, String)> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix("# ") {
            section = header.to_string();
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once(char::is_whitespace) {
            Some(("keep", name)) => kept.push((section.clone(), name.trim().to_string())),
            Some(("skip", _)) => {}
            _ => return Err(AdoptError::Line { line: index + 1, text: line.into() }),
        }
    }
    Ok(candidates.iter().map(|candidate| (candidate.clone(), kept.contains(&(candidate.section(), candidate.name().to_string())))).collect())
}

// records kept candidates as declared and skipped ones as ignored, then writes maw.nix
pub fn apply(repo: &Repo, mut state: MawState, decisions: &[(Candidate, bool)]) -> Result<(), AdoptError> {
    decisions.iter().for_each(|(candidate, keep)| {
        let (lists, key) = match (candidate, keep) {
            (Candidate::Package { backend, .. }, true) => (&mut state.packages, backend.clone()),
            (Candidate::Package { backend, .. }, false) => (&mut state.ignored.packages, backend.clone()),
            (Candidate::Service { scope, .. }, true) => (&mut state.services, scope.to_string()),
            (Candidate::Service { scope, .. }, false) => (&mut state.ignored.services, scope.to_string()),
        };
        lists.entry(key).or_default().push(candidate.name().to_string());
    });
    Ok(state.write(&repo.maw_file())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, fixture};
    use std::os::unix::fs::symlink;

    // bash is installed by hand in the fixture; this adds an enabled system service nothing declares
    fn with_service(fixture: &Fixture, name: &str) {
        let definition = fixture.env.sysroot.join("etc/sv").join(name);
        fs::create_dir_all(&definition).unwrap();
        fs::create_dir_all(fixture.env.sysroot.join("var/service")).unwrap();
        symlink(&definition, fixture.env.sysroot.join("var/service").join(name)).unwrap();
    }

    fn found(fixture: &Fixture) -> Vec<Candidate> {
        candidates(&fixture.env, &fixture.runner, &fixture.repo).unwrap()
    }

    fn maw_nix(fixture: &Fixture) -> String {
        fs::read_to_string(fixture.repo.maw_file()).unwrap()
    }

    #[test]
    fn candidates_are_undeclared_manual_packages_and_enabled_services() {
        let fixture = fixture(&[]);
        with_service(&fixture, "dbus");
        assert_eq!(found(&fixture), [
            Candidate::Package { backend: "xbps".into(), spec: "bash".into() },
            Candidate::Service { scope: Scope::System, name: "dbus".into() },
        ]);
    }

    #[test]
    fn checklist_round_trips_and_deleted_lines_are_skipped() {
        let candidates = vec![
            Candidate::Package { backend: "xbps".into(), spec: "bash".into() },
            Candidate::Package { backend: "xbps".into(), spec: "zig".into() },
            Candidate::Service { scope: Scope::System, name: "dbus".into() },
        ];
        let text = checklist(&candidates);
        assert!(text.contains("# packages (xbps)\nkeep bash\nkeep zig\n\n# services (system)\nkeep dbus\n"));

        let edited = text.replace("keep zig\n", "").replace("keep dbus", "skip dbus");
        let keeps: Vec<bool> = parse(&edited, &candidates).unwrap().into_iter().map(|(_, keep)| keep).collect();
        assert_eq!(keeps, [true, false, false]);
    }

    #[test]
    fn garbage_lines_name_their_line() {
        let result = parse("# packages (xbps)\nmaybe bash\n", &[]);
        assert!(matches!(result, Err(AdoptError::Line { line: 2, .. })));
    }

    #[test]
    fn kept_is_declared_skipped_is_ignored_and_neither_comes_back() {
        let fixture = fixture(&[]);
        with_service(&fixture, "dbus");
        let decisions = parse(&checklist(&found(&fixture)).replace("keep dbus", "skip dbus"), &found(&fixture)).unwrap();
        apply(&fixture.repo, MawState::default(), &decisions).unwrap();

        assert!(maw_nix(&fixture).contains("packages = {\n    xbps = [ \"bash\" ];"));
        assert!(maw_nix(&fixture).contains("ignored = {\n    packages = { };\n    services = {\n      system = [ \"dbus\" ];"));
        assert!(found(&fixture).is_empty());
    }
}
