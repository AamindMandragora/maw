use crate::backend::{Backend, BackendError};
use crate::edit::{self, EditError};
use crate::env::Env;
use crate::repo::Repo;
use crate::runner::Runner;
use crate::state::{MawState, StateError};
use std::collections::HashSet;

#[derive(Debug, thiserror::Error)]
pub enum PackagesError {
    #[error("no package {0}; `maw search {0}` to look for it")]
    NotFound(String),
    #[error("{0} is neither installed nor declared")]
    Unknown(String),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error(transparent)]
    Edit(#[from] EditError),
    #[error(transparent)]
    State(#[from] StateError),
}

// what an install or remove does: packages changed, maw.nix entries changed, modules created
#[derive(Debug, Default, PartialEq)]
pub struct Change {
    pub packages: Vec<String>,
    pub recorded: Vec<String>,
    pub scaffolded: Vec<String>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty() && self.recorded.is_empty() && self.scaffolded.is_empty()
    }
}

// the names a maw.nix declares for a backend
pub fn declared(state: &MawState, backend: &str) -> Vec<String> {
    state.packages.get(backend).cloned().unwrap_or_default()
}

fn installed(backend: &dyn Backend) -> Result<HashSet<String>, BackendError> {
    Ok(backend.list()?.into_iter().map(|pkg| pkg.name).collect())
}

// what installing names would do: install the missing, record the undeclared, scaffold modules the registry knows
pub fn plan_install(env: &Env, runner: &dyn Runner, repo: &Repo, backend: &dyn Backend, names: &[String]) -> Result<Change, PackagesError> {
    let (registry, state) = edit::load(env, runner, repo)?;
    let installed = installed(backend)?;
    let declared = declared(&state, backend.name());
    let packages: Vec<String> = names.iter().filter(|name| !installed.contains(*name)).cloned().collect();

    // anything not installed yet has to exist in the repo
    if let Some(unknown) = first_unknown(backend, &packages)? {
        return Err(PackagesError::NotFound(unknown.clone()));
    }

    // a program gets a module only when the registry knows it and the repo has nothing for it yet
    let scaffolded = names
        .iter()
        .map(|name| name.to_lowercase())
        .filter(|program| registry.has(program) && !repo.module_file(program).exists() && !repo.static_dir().join(program).exists())
        .collect();
    let recorded = names.iter().filter(|name| !declared.contains(name)).cloned().collect();
    Ok(Change { packages, recorded, scaffolded })
}

// the first name the repo has no package for
fn first_unknown<'n>(backend: &dyn Backend, names: &'n [String]) -> Result<Option<&'n String>, BackendError> {
    for name in names {
        if backend.info(name)?.is_none() {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

// installs, records in maw.nix, and scaffolds modules; activating afterwards is up to the caller
pub fn install(env: &Env, runner: &dyn Runner, repo: &Repo, backend: &dyn Backend, names: &[String]) -> Result<Change, PackagesError> {
    let change = plan_install(env, runner, repo, backend, names)?;
    if !change.packages.is_empty() {
        backend.install(&change.packages)?;
    }

    let (_, mut state) = edit::load(env, runner, repo)?;
    if !change.recorded.is_empty() {
        state.packages.entry(backend.name().into()).or_default().extend(change.recorded.iter().cloned());
        state.write(&repo.maw_file())?;
    }
    change.scaffolded.iter().try_for_each(|program| edit::new_module(env, runner, repo, program, None).map(|_| ()))?;
    Ok(change)
}

// what removing names would do: remove the installed, drop the declared; modules are kept
pub fn plan_remove(env: &Env, runner: &dyn Runner, repo: &Repo, backend: &dyn Backend, names: &[String]) -> Result<Change, PackagesError> {
    let (_, state) = edit::load(env, runner, repo)?;
    let installed = installed(backend)?;
    let declared = declared(&state, backend.name());

    if let Some(unknown) = names.iter().find(|name| !installed.contains(*name) && !declared.contains(name)) {
        return Err(PackagesError::Unknown(unknown.clone()));
    }
    let packages = names.iter().filter(|name| installed.contains(*name)).cloned().collect();
    let recorded = names.iter().filter(|name| declared.contains(name)).cloned().collect();
    Ok(Change { packages, recorded, scaffolded: Vec::new() })
}

// removes the packages and drops them from maw.nix
pub fn remove(env: &Env, runner: &dyn Runner, repo: &Repo, backend: &dyn Backend, names: &[String]) -> Result<Change, PackagesError> {
    let change = plan_remove(env, runner, repo, backend, names)?;
    if !change.packages.is_empty() {
        backend.remove(&change.packages)?;
    }

    if !change.recorded.is_empty() {
        let (_, mut state) = edit::load(env, runner, repo)?;
        let kept: Vec<String> = declared(&state, backend.name()).into_iter().filter(|name| !change.recorded.contains(name)).collect();
        match kept.is_empty() {
            true => state.packages.remove(backend.name()),
            false => state.packages.insert(backend.name().into(), kept),
        };
        state.write(&repo.maw_file())?;
    }
    Ok(change)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::xbps::Xbps;
    use crate::testing::{Fixture, fixture};
    use std::fs;

    fn xbps(fixture: &Fixture) -> Xbps<'_> {
        Xbps::new(&fixture.runner, &fixture.env)
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    fn installs(fixture: &Fixture) -> Vec<String> {
        fixture.runner.calls.borrow().iter().filter(|call| call.starts_with("xbps-install")).cloned().collect()
    }

    #[test]
    fn plan_installs_missing_records_undeclared_and_scaffolds_known_programs() {
        let fixture = fixture(&[]);
        let change = plan_install(&fixture.env, &fixture.runner, &fixture.repo, &xbps(&fixture), &names(&["foot", "bash"])).unwrap();
        assert_eq!(change, Change { packages: names(&["foot"]), recorded: names(&["foot", "bash"]), scaffolded: names(&["foot", "bash"]) });
    }

    #[test]
    fn install_runs_xbps_writes_maw_nix_and_creates_the_module() {
        let fixture = fixture(&[]);
        install(&fixture.env, &fixture.runner, &fixture.repo, &xbps(&fixture), &names(&["foot"])).unwrap();

        let root = fixture.env.sysroot.display();
        assert_eq!(installs(&fixture), [format!("xbps-install -r {root} -y foot")]);
        assert!(fs::read_to_string(fixture.repo.maw_file()).unwrap().contains(r#"xbps = [ "foot" ];"#));
        assert!(fixture.repo.module_file("foot").is_file());
    }

    #[test]
    fn existing_modules_are_left_alone() {
        let fixture = fixture(&["foot"]);
        let change = plan_install(&fixture.env, &fixture.runner, &fixture.repo, &xbps(&fixture), &names(&["foot"])).unwrap();
        assert!(change.scaffolded.is_empty());
    }

    #[test]
    fn unknown_packages_fail_before_anything_happens() {
        let fixture = fixture(&[]);
        let result = install(&fixture.env, &fixture.runner, &fixture.repo, &xbps(&fixture), &names(&["foot", "nope"]));
        assert!(matches!(result, Err(PackagesError::NotFound(name)) if name == "nope"));
        assert!(installs(&fixture).is_empty());
    }

    #[test]
    fn remove_uninstalls_and_drops_the_record() {
        let fixture = fixture(&[]);
        install(&fixture.env, &fixture.runner, &fixture.repo, &xbps(&fixture), &names(&["bash", "foot"])).unwrap();

        let change = remove(&fixture.env, &fixture.runner, &fixture.repo, &xbps(&fixture), &names(&["bash"])).unwrap();
        assert_eq!((change.packages, change.recorded), (names(&["bash"]), names(&["bash"])));
        assert!(fs::read_to_string(fixture.repo.maw_file()).unwrap().contains(r#"xbps = [ "foot" ];"#));
        assert!(fixture.runner.calls.borrow().iter().any(|call| call.ends_with("-Ry bash")));
    }

    #[test]
    fn removing_the_unknown_fails() {
        let fixture = fixture(&[]);
        let result = plan_remove(&fixture.env, &fixture.runner, &fixture.repo, &xbps(&fixture), &names(&["foot"]));
        assert!(matches!(result, Err(PackagesError::Unknown(_))));
    }
}
