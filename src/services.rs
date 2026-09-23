use crate::activate::ActivateError;
use crate::build::{self, BuildError, Options, Report};
use crate::env::Env;
use crate::init::runit::Runit;
use crate::init::{InitBackend, Scope};
use crate::repo::Repo;
use crate::runner::{RunError, Runner};
use crate::state::StateError;
use crate::system;
use std::collections::BTreeSet;
use std::fs;

#[derive(Debug, thiserror::Error)]
pub enum ServicesError {
    #[error("no service {0}")]
    Unknown(String),
    #[error("a module enables {0} with lib.service; set `enable = false` there instead")]
    ModuleEnabled(String),
    #[error(transparent)]
    Activate(#[from] ActivateError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Run(#[from] RunError),
}

// one service in `maw sv list`
#[derive(Debug, PartialEq)]
pub struct Row {
    pub scope: Scope,
    pub name: String,
    pub declared: bool,
    pub enabled: bool,
    pub state: Option<String>,
}

// modules and maw.nix, evaluated without writing out/
fn load(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Report, ServicesError> {
    Ok(build::build(env, runner, repo, Options { dry_run: true, force: false })?)
}

// every service linked into a supervised dir
fn linked(env: &Env) -> BTreeSet<(Scope, String)> {
    [Scope::System, Scope::User]
        .into_iter()
        .flat_map(|scope| {
            let dir = Runit.enabled_link(env, scope, "_").parent().unwrap().to_path_buf();
            let names = fs::read_dir(dir).into_iter().flatten().filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()));
            names.map(move |name| (scope, name))
        })
        .collect()
}

// every declared or enabled service, with its state where it can be read
pub fn list(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Vec<Row>, ServicesError> {
    let declared = system::declared_services(&load(env, runner, repo)?);
    let enabled = linked(env);
    let rows = declared.union(&enabled).map(|(scope, name)| Row {
        scope: *scope,
        name: name.clone(),
        declared: declared.contains(&(*scope, name.clone())),
        enabled: enabled.contains(&(*scope, name.clone())),
        state: Runit.state(env, runner, *scope, name),
    });
    Ok(rows.collect())
}

// the scope a name means: the one asked for, else what declares it, what's linked, or where a definition is
fn scope_of(env: &Env, build: &Report, name: &str, wanted: Option<Scope>) -> Result<Scope, ServicesError> {
    let scopes = [Scope::System, Scope::User];
    let declared = system::declared_services(build);
    wanted
        .or_else(|| scopes.into_iter().find(|scope| declared.contains(&(*scope, name.to_string()))))
        .or_else(|| scopes.into_iter().find(|scope| fs::symlink_metadata(Runit.enabled_link(env, *scope, name)).is_ok()))
        .or_else(|| scopes.into_iter().find(|scope| Runit.definition(env, *scope, name).exists()))
        .ok_or_else(|| ServicesError::Unknown(name.into()))
}

// whether a module's lib.service defines this service and leaves it enabled
fn module_enables(build: &Report, scope: Scope, name: &str) -> bool {
    build.outputs.iter().filter_map(|output| output.service.as_ref()).any(|service| service.scope == scope && service.name == name && service.enable)
}

// records a service as enabled in maw.nix; activating links it. Returns its scope
pub fn enable(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str, scope: Option<Scope>) -> Result<Scope, ServicesError> {
    let mut build = load(env, runner, repo)?;
    let scope = scope_of(env, &build, name, scope)?;
    let from_module = module_enables(&build, scope, name);
    if !Runit.definition(env, scope, name).exists() && !from_module {
        return Err(ServicesError::Unknown(name.into()));
    }

    // module-defined services are enabled by their module, so only others go into maw.nix
    let listed = build.state.services.entry(scope.to_string()).or_default();
    if !from_module && !listed.contains(&name.to_string()) {
        listed.push(name.into());
        build.state.write(&repo.maw_file())?;
    }
    Ok(scope)
}

// stops and unlinks a service now and drops it from maw.nix; true if maw.nix changed
pub fn disable(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str, scope: Option<Scope>) -> Result<(Scope, bool), ServicesError> {
    let mut build = load(env, runner, repo)?;
    let scope = scope_of(env, &build, name, scope)?;
    if module_enables(&build, scope, name) {
        return Err(ServicesError::ModuleEnabled(name.into()));
    }

    if fs::symlink_metadata(Runit.enabled_link(env, scope, name)).is_ok() {
        system::disable(env, runner, scope, name)?;
    }
    let listed = build.state.services.entry(scope.to_string()).or_default();
    let recorded = listed.contains(&name.to_string());
    listed.retain(|listed| listed != name);
    build.state.services.retain(|_, names| !names.is_empty());
    if recorded {
        build.state.write(&repo.maw_file())?;
    }
    Ok((scope, recorded))
}

// runs sv status, restart, or another action on a service
pub fn control(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str, scope: Option<Scope>, action: &str) -> Result<(), ServicesError> {
    let scope = scope_of(env, &load(env, runner, repo)?, name, scope)?;
    Ok(Runit.control(env, runner, scope, name, action)?)
}

// follows a service's log on the terminal
pub fn log(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str, scope: Option<Scope>) -> Result<(), ServicesError> {
    let scope = scope_of(env, &load(env, runner, repo)?, name, scope)?;
    let file = Runit.log_file(env, scope, name).display().to_string();
    Ok(runner.interactive("tail", &["-n".into(), "50".into(), "-F".into(), file])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activate::{Ask, activate};
    use crate::testing::{Fixture, fixture};

    struct Nobody;

    impl Ask for Nobody {
        fn ask(&self, _: &str) -> Option<String> {
            None
        }
    }

    fn activate_now(fixture: &Fixture) {
        activate(&fixture.env, &fixture.runner, &fixture.repo, &Nobody, Options::default()).unwrap();
    }

    fn stock(fixture: &Fixture, name: &str) {
        fs::create_dir_all(fixture.env.sysroot.join("etc/sv").join(name)).unwrap();
    }

    #[test]
    fn enabling_a_stock_service_records_it_and_activation_links_it() {
        let fixture = fixture(&[]);
        stock(&fixture, "dbus");
        assert_eq!(enable(&fixture.env, &fixture.runner, &fixture.repo, "dbus", None).unwrap(), Scope::System);
        assert!(fs::read_to_string(fixture.repo.maw_file()).unwrap().contains(r#"system = [ "dbus" ];"#));

        activate_now(&fixture);
        assert!(fs::symlink_metadata(fixture.env.sysroot.join("var/service/dbus")).is_ok());
    }

    #[test]
    fn disabling_unlinks_now_and_drops_the_record() {
        let fixture = fixture(&[]);
        stock(&fixture, "dbus");
        enable(&fixture.env, &fixture.runner, &fixture.repo, "dbus", None).unwrap();
        activate_now(&fixture);

        assert_eq!(disable(&fixture.env, &fixture.runner, &fixture.repo, "dbus", None).unwrap(), (Scope::System, true));
        assert!(fs::symlink_metadata(fixture.env.sysroot.join("var/service/dbus")).is_err());
        assert!(!fs::read_to_string(fixture.repo.maw_file()).unwrap().contains("dbus"));
    }

    #[test]
    fn module_enabled_services_are_disabled_in_their_module() {
        let fixture = fixture(&["usersvc"]);
        activate_now(&fixture);
        assert!(matches!(disable(&fixture.env, &fixture.runner, &fixture.repo, "usersvc", None), Err(ServicesError::ModuleEnabled(_))));
    }

    #[test]
    fn unknown_services_are_an_error() {
        let fixture = fixture(&[]);
        assert!(matches!(enable(&fixture.env, &fixture.runner, &fixture.repo, "nope", None), Err(ServicesError::Unknown(_))));
    }

    #[test]
    fn list_shows_declared_and_undeclared_links() {
        let fixture = fixture(&["usersvc"]);
        activate_now(&fixture);
        stock(&fixture, "agetty");
        fs::create_dir_all(fixture.env.sysroot.join("var/service")).unwrap();
        std::os::unix::fs::symlink(fixture.env.sysroot.join("etc/sv/agetty"), fixture.env.sysroot.join("var/service/agetty")).unwrap();

        let rows = list(&fixture.env, &fixture.runner, &fixture.repo).unwrap();
        let summary: Vec<(Scope, &str, bool, bool)> = rows.iter().map(|row| (row.scope, row.name.as_str(), row.declared, row.enabled)).collect();
        assert_eq!(summary, [(Scope::System, "agetty", false, true), (Scope::User, "usersvc", true, true)]);
    }
}
