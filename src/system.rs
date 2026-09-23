use crate::activate::{ActivateError, Step, Wanted};
use crate::backup;
use crate::build::{Output, Report};
use crate::env::Env;
use crate::init::runit::Runit;
use crate::init::{InitBackend, Scope};
use crate::inputs::hash_bytes;
use crate::runner::{Runner, as_root};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

// services by scope and name
pub type Services = BTreeSet<(Scope, String)>;

// what maw has copied as root, with content hashes, and the services it enabled
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub copied: BTreeMap<PathBuf, String>,
    #[serde(default)]
    pub services: Services,
}

// root files to copy or delete, and the record of copies they leave behind
pub fn plan_copies(record: &Record, copies: &[Wanted], force: bool) -> (Vec<Step>, BTreeMap<PathBuf, String>) {
    let wanted: HashSet<&PathBuf> = copies.iter().map(|file| &file.destination).collect();
    let live_hash = |path: &Path| fs::read(path).ok().map(|bytes| hash_bytes(&bytes));

    // copies no longer wanted are deleted, unless someone changed them since
    let deletes = record
        .copied
        .iter()
        .filter(|(destination, hash)| !wanted.contains(destination) && live_hash(destination).as_ref() == Some(*hash))
        .map(|(destination, _)| Step::Delete { destination: destination.clone() });

    let decisions: Vec<(Option<Step>, &Wanted)> = copies.iter().map(|file| (decide_copy(record.copied.get(&file.destination), file, force), file)).collect();

    // edited copies keep their old record so they stay drifted until forced
    let copied = decisions
        .iter()
        .filter_map(|(step, file)| match step {
            Some(Step::Edited { .. }) => Some((file.destination.clone(), record.copied.get(&file.destination)?.clone())),
            _ => Some((file.destination.clone(), file.hash.clone())),
        })
        .collect();
    (deletes.chain(decisions.into_iter().filter_map(|(step, _)| step)).collect(), copied)
}

// a root file is copied when missing or still what maw last put there; anything else is drift or needs a backup
fn decide_copy(previous: Option<&String>, file: &Wanted, force: bool) -> Option<Step> {
    let destination = file.destination.clone();
    let exists = fs::symlink_metadata(&file.destination).is_ok();
    let live = fs::read(&file.destination).ok().map(|bytes| hash_bytes(&bytes));

    if !exists {
        Some(Step::Copy { destination, backup: false })
    } else if live.as_ref() == Some(&file.hash) {
        None
    } else if live.is_some() && live.as_ref() == previous {
        Some(Step::Copy { destination, backup: false })
    } else if previous.is_some() && !force {
        Some(Step::Edited { destination })
    } else {
        Some(Step::Copy { destination, backup: true })
    }
}

// every service that should be enabled: maw.nix's lists plus module services left enabled
pub fn declared_services(build: &Report) -> Services {
    let listed = build.state.services.iter().flat_map(|(scope, names)| names.iter().map(|name| (Scope::parse(scope), name.clone())));
    let defined = build.outputs.iter().filter_map(|output| output.service.as_ref()).filter(|service| service.enable);
    listed.chain(defined.map(|service| (service.scope, service.name.clone()))).collect()
}

// services to enable, disable, purge, and restart, and the set maw has enabled afterwards; placed is every file maw put down last time
pub fn plan_services(env: &Env, build: &Report, record: &Record, changed: &HashSet<PathBuf>, placed: &HashSet<PathBuf>) -> Result<(Vec<Step>, Services), ActivateError> {
    let declared = declared_services(build);
    let is_enabled = |(scope, name): &(Scope, String)| fs::symlink_metadata(Runit.enabled_link(env, *scope, name)).is_ok();

    // a declared service needs a definition, already on disk or about to be written
    if let Some((scope, name)) = declared.iter().find(|(scope, name)| !Runit.definition(env, *scope, name).exists() && rendered(build, *scope, name).next().is_none()) {
        return Err(ActivateError::UnknownService { scope: *scope, name: name.clone() });
    }

    let step = |make: fn(Scope, String) -> Step| move |(scope, name): &(Scope, String)| make(*scope, name.clone());
    let dropped: Vec<&(Scope, String)> = record.services.difference(&declared).collect();
    let disables = dropped.iter().copied().filter(|service| is_enabled(service)).map(step(|scope, name| Step::Disable { scope, name }));

    // a dropped service whose files maw wrote leaves only runit's state behind, so its dir goes too; stock ones stay
    let wrote = |(scope, name): &(Scope, String)| {
        let definition = Runit.definition(env, *scope, name);
        rendered(build, *scope, name).next().is_none() && placed.iter().any(|path| path.starts_with(&definition)) && definition.exists()
    };
    let purges = dropped.iter().copied().filter(|service| wrote(service)).map(step(|scope, name| Step::Purge { scope, name }));
    let enables = declared.iter().filter(|service| !is_enabled(service)).map(step(|scope, name| Step::Enable { scope, name }));

    // running services whose files change get restarted; newly enabled ones start on their own
    let restarts = declared
        .iter()
        .filter(|service| is_enabled(service) && rendered(build, service.0, &service.1).any(|output| changed.contains(&output.destination)))
        .map(step(|scope, name| Step::Restart { scope, name }));

    let steps = disables.chain(purges).chain(enables).chain(restarts).collect();
    Ok((steps, declared))
}

// the files a service is made of
fn rendered<'b>(build: &'b Report, scope: Scope, name: &'b str) -> impl Iterator<Item = &'b Output> + 'b {
    build.outputs.iter().filter(move |output| output.service.as_ref().is_some_and(|service| service.scope == scope && service.name == name))
}

// carries out root copies and service changes, in step order; returns the backups made
pub fn apply(env: &Env, runner: &dyn Runner, steps: &[Step], copies: &[Wanted]) -> Result<Vec<(PathBuf, PathBuf)>, ActivateError> {
    let sources: BTreeMap<&PathBuf, &PathBuf> = copies.iter().map(|file| (&file.destination, &file.source)).collect();
    let root = |program: &str, args: &[&Path]| {
        let args: Vec<String> = args.iter().map(|arg| arg.display().to_string()).collect();
        as_root(runner, &env.sysroot, program, &args)
    };

    // each step yields the backup it made, if any
    let backups = steps
        .iter()
        .map(|step| -> Result<Option<(PathBuf, PathBuf)>, ActivateError> {
            match step {
                Step::Copy { destination, backup } => {
                    let saved = backup.then(|| backup_as_root(env, runner, destination)).transpose()?;
                    copy_as_root(env, runner, sources[destination], destination)?;
                    Ok(saved.map(|saved| (destination.clone(), saved)))
                }
                Step::Delete { destination } => root("rm", &[Path::new("-f"), destination]).map(|_| None).map_err(Into::into),
                Step::Enable { scope, name } => enable(env, runner, *scope, name).map(|_| None),
                Step::Disable { scope, name } => disable(env, runner, *scope, name).map(|_| None),
                Step::Purge { scope, name } => purge(env, runner, *scope, name).map(|_| None),
                Step::Restart { scope, name } => Ok(Runit.control(env, runner, *scope, name, "restart").map(|_| None)?),
                _ => Ok(None),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(backups.into_iter().flatten().collect())
}

// saves a root file into the backup mirror (which the user owns) with cp -p; returns where it went
fn backup_as_root(env: &Env, runner: &dyn Runner, path: &Path) -> Result<PathBuf, ActivateError> {
    let target = backup::next_target(env, path);
    fs::create_dir_all(target.parent().unwrap()).map_err(|source| ActivateError::Io { path: target.clone(), source })?;
    as_root(runner, &env.sysroot, "cp", &["-p".into(), path.display().to_string(), target.display().to_string()])?;
    Ok(target)
}

// installs a file as root with its source's mode, creating parent dirs
fn copy_as_root(env: &Env, runner: &dyn Runner, source: &Path, destination: &Path) -> Result<(), ActivateError> {
    let mode = fs::metadata(source).map_err(|error| ActivateError::Io { path: source.into(), source: error })?.permissions().mode() & 0o777;
    let args = ["-D".to_string(), "-m".into(), format!("{mode:o}"), source.display().to_string(), destination.display().to_string()];
    Ok(as_root(runner, &env.sysroot, "install", &args)?)
}

// deletes a service's definition dir: as root for system services, directly for user ones
fn purge(env: &Env, runner: &dyn Runner, scope: Scope, name: &str) -> Result<(), ActivateError> {
    let definition = Runit.definition(env, scope, name);
    match scope {
        Scope::System => Ok(as_root(runner, &env.sysroot, "rm", &["-rf".into(), definition.display().to_string()])?),
        Scope::User => fs::remove_dir_all(&definition).map_err(|source| ActivateError::Io { path: definition, source }),
    }
}

// links a service into the supervised dir: as root for system services, directly for user ones
fn enable(env: &Env, runner: &dyn Runner, scope: Scope, name: &str) -> Result<(), ActivateError> {
    let (definition, link) = (Runit.definition(env, scope, name), Runit.enabled_link(env, scope, name));
    let io = |source| ActivateError::Io { path: link.clone(), source };
    match scope {
        Scope::System => {
            // /var/service always exists on void; a fresh scratch root needs it made
            let parent = link.parent().unwrap();
            if !parent.exists() {
                as_root(runner, &env.sysroot, "mkdir", &["-p".into(), parent.display().to_string()])?;
            }
            Ok(as_root(runner, &env.sysroot, "ln", &["-s".into(), definition.display().to_string(), link.display().to_string()])?)
        }
        Scope::User => {
            fs::create_dir_all(link.parent().unwrap()).map_err(io)?;
            symlink(&definition, &link).map_err(io)
        }
    }
}

// stops a service, then removes its link; a service that won't stop is unlinked anyway, and runit stops it
pub fn disable(env: &Env, runner: &dyn Runner, scope: Scope, name: &str) -> Result<(), ActivateError> {
    let _ = Runit.control(env, runner, scope, name, "down");
    let link = Runit.enabled_link(env, scope, name);
    match scope {
        Scope::System => Ok(as_root(runner, &env.sysroot, "rm", &[link.display().to_string()])?),
        Scope::User => fs::remove_file(&link).map_err(|source| ActivateError::Io { path: link, source }),
    }
}

#[cfg(test)]
mod tests {
    use crate::activate::{Activation, Ask, Options, Step, activate};
    use crate::init::Scope;
    use crate::testing::{Fixture, fixture, write};
    use std::fs;
    use std::path::PathBuf;

    struct Nobody;

    impl Ask for Nobody {
        fn ask(&self, _: &str) -> Option<String> {
            None
        }
    }

    fn run(fixture: &Fixture, force: bool) -> Activation {
        activate(&fixture.env, &fixture.runner, &fixture.repo, &Nobody, Options { dry_run: false, force }).unwrap()
    }

    fn greetd(fixture: &Fixture) -> PathBuf {
        fixture.env.sysroot.join("etc/greetd/config.toml")
    }

    fn sv_calls(fixture: &Fixture) -> Vec<String> {
        fixture.runner.calls.borrow().iter().filter(|call| call.starts_with("sv ")).cloned().collect()
    }

    fn set_services(fixture: &Fixture, system: &str) {
        let text = format!("{{\n  packages = {{ }};\n  services = {{\n    system = [ {system} ];\n  }};\n  paths = {{ }};\n}}\n");
        fs::write(fixture.repo.maw_file(), text).unwrap();
    }

    #[test]
    fn root_files_are_copied_once() {
        let fixture = fixture(&["greetd"]);
        assert_eq!(run(&fixture, false).steps, [Step::Copy { destination: greetd(&fixture), backup: false }]);
        assert_eq!(fs::read_to_string(greetd(&fixture)).unwrap(), "greetd v1\n");
        assert!(!fs::symlink_metadata(greetd(&fixture)).unwrap().is_symlink());
        assert!(run(&fixture, false).steps.is_empty());
    }

    #[test]
    fn a_different_root_file_is_backed_up_before_the_first_copy() {
        let fixture = fixture(&["greetd"]);
        write(&greetd(&fixture), "distro default\n");
        let activation = run(&fixture, false);

        assert_eq!(activation.steps, [Step::Copy { destination: greetd(&fixture), backup: true }]);
        assert_eq!(fs::read_to_string(&activation.backups[0].1).unwrap(), "distro default\n");
        assert!(activation.backups[0].1.starts_with(fixture.env.backup_dir().join("system")));
    }

    #[test]
    fn edited_root_files_are_drift_until_forced() {
        let fixture = fixture(&["greetd"]);
        run(&fixture, false);
        fs::write(greetd(&fixture), "hand edit\n").unwrap();

        assert_eq!(run(&fixture, false).steps, [Step::Edited { destination: greetd(&fixture) }]);
        assert_eq!(fs::read_to_string(greetd(&fixture)).unwrap(), "hand edit\n");
        assert_eq!(run(&fixture, true).backups.len(), 1);
        assert_eq!(fs::read_to_string(greetd(&fixture)).unwrap(), "greetd v1\n");
    }

    #[test]
    fn root_files_nothing_declares_are_deleted() {
        let fixture = fixture(&["greetd"]);
        run(&fixture, false);
        fs::remove_file(fixture.repo.module_file("greetd")).unwrap();
        assert_eq!(run(&fixture, false).steps, [Step::Delete { destination: greetd(&fixture) }]);
        assert!(!greetd(&fixture).exists());
    }

    #[test]
    fn user_services_are_linked_enabled_and_restarted_on_change() {
        let fixture = fixture(&["usersvc"]);
        let first = run(&fixture, false);
        assert!(first.steps.contains(&Step::Enable { scope: Scope::User, name: "usersvc".into() }));

        let definition = fixture.env.home.join(".config/sv/usersvc");
        assert!(fs::read_to_string(definition.join("run")).unwrap().contains("exec sleep 1000"));
        assert!(definition.join("log/run").is_file());
        assert_eq!(fs::read_link(fixture.env.home.join(".config/service/usersvc")).unwrap(), definition);
        assert!(run(&fixture, false).steps.is_empty());

        // an edited service is restarted, not re-enabled
        fs::write(fixture.repo.module_file("usersvc"), "v2").unwrap();
        let changed = run(&fixture, false);
        assert!(changed.steps.contains(&Step::Restart { scope: Scope::User, name: "usersvc".into() }));
        assert!(!changed.steps.iter().any(|step| matches!(step, Step::Enable { .. })));
        assert_eq!(sv_calls(&fixture), [format!("sv restart {}", fixture.env.home.join(".config/service/usersvc").display())]);
    }

    #[test]
    fn system_services_are_copied_and_linked_as_root() {
        let fixture = fixture(&["syssvc"]);
        run(&fixture, false);
        let definition = fixture.env.sysroot.join("etc/sv/syssvc");
        assert!(fs::metadata(definition.join("run")).unwrap().permissions().mode() & 0o111 != 0);
        assert_eq!(fs::read_link(fixture.env.sysroot.join("var/service/syssvc")).unwrap(), definition);
    }

    #[test]
    fn removed_services_are_stopped_and_unlinked() {
        let fixture = fixture(&["usersvc"]);
        run(&fixture, false);
        fs::remove_file(fixture.repo.module_file("usersvc")).unwrap();

        let activation = run(&fixture, false);
        assert!(activation.steps.contains(&Step::Disable { scope: Scope::User, name: "usersvc".into() }));
        assert!(activation.steps.contains(&Step::Purge { scope: Scope::User, name: "usersvc".into() }));
        assert!(sv_calls(&fixture)[0].starts_with("sv down"));
        assert!(fs::symlink_metadata(fixture.env.home.join(".config/service/usersvc")).is_err());
        assert!(!fixture.env.home.join(".config/sv/usersvc").exists());
    }

    #[test]
    fn declared_stock_services_need_a_definition() {
        let fixture = fixture(&[]);
        set_services(&fixture, r#""nope""#);
        let result = activate(&fixture.env, &fixture.runner, &fixture.repo, &Nobody, Options::default());
        assert!(matches!(result, Err(crate::activate::ActivateError::UnknownService { .. })));

        // a stock service whose definition exists is just linked
        fs::create_dir_all(fixture.env.sysroot.join("etc/sv/dbus")).unwrap();
        set_services(&fixture, r#""dbus""#);
        assert_eq!(run(&fixture, false).steps, [Step::Enable { scope: Scope::System, name: "dbus".into() }]);

        // dropping a stock service disables it but never deletes its definition
        set_services(&fixture, "");
        assert_eq!(run(&fixture, false).steps, [Step::Disable { scope: Scope::System, name: "dbus".into() }]);
        assert!(fixture.env.sysroot.join("etc/sv/dbus").exists());
    }

    use std::os::unix::fs::PermissionsExt;
}
