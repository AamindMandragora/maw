use crate::activate::{self, ActivateError, Options, Step, links_to};
use crate::adopt::{self, AdoptError, Candidate};
use crate::backend::{self, BackendError, NAMES};
use crate::build::Report;
use crate::env::Env;
use crate::repo::Repo;
use crate::runner::Runner;
use similar::TextDiff;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::fs;
use std::path::PathBuf;

// a destination whose content activating would change: what's there now, and what maw would leave
#[derive(Debug, PartialEq)]
pub struct FileDiff {
    pub destination: PathBuf,
    pub live: Vec<u8>,
    pub wanted: Vec<u8>,
}

impl FileDiff {
    // a unified diff between live and wanted, or None when either side isn't text
    pub fn unified(&self, label: &str) -> Option<String> {
        let live = std::str::from_utf8(&self.live).ok()?;
        let wanted = std::str::from_utf8(&self.wanted).ok()?;
        let diff = TextDiff::from_lines(live, wanted);
        Some(diff.unified_diff().context_radius(3).header(&format!("{label} (live)"), &format!("{label} (maw)")).to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StatusError {
    #[error(transparent)]
    Activate(#[from] ActivateError),
    #[error(transparent)]
    Adopt(#[from] AdoptError),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error(transparent)]
    Packages(#[from] crate::packages::PackagesError),
}

// everything out of sync: pending steps, things maw.nix doesn't know about, and config for programs that aren't there
#[derive(Debug, Default)]
pub struct Full {
    pub steps: Vec<Step>,
    pub undeclared: Vec<Candidate>,
    pub orphans: Vec<String>,
    // source packages behind their template, as (name, installed, template)
    pub outdated: Vec<(String, String, String)>,
}

impl Full {
    pub fn is_clean(&self) -> bool {
        self.steps.is_empty() && self.undeclared.is_empty() && self.orphans.is_empty() && self.outdated.is_empty()
    }
}

// the full drift report; path_var decides which commands count as installed
pub fn report(env: &Env, runner: &dyn Runner, repo: &Repo, path_var: &str) -> Result<Full, StatusError> {
    let planned = activate::plan_activation(env, runner, repo, Options { dry_run: true, ..Options::default() })?;
    let orphans = orphans(env, runner, repo, &planned.build, path_var)?;
    let undeclared = adopt::candidates(env, runner, repo)?;
    let outdated = crate::packages::outdated(env, runner, repo, &planned.build.state)?;
    Ok(Full { steps: pending(planned), undeclared, orphans, outdated })
}

// modules and static dirs whose program isn't there: no installed package by that name, and no such command on PATH
fn orphans(env: &Env, runner: &dyn Runner, repo: &Repo, build: &Report, path_var: &str) -> Result<Vec<String>, BackendError> {
    let installed: HashSet<String> = NAMES
        .iter()
        .filter_map(|name| backend::for_name(name, runner, env))
        .map(|backend| Ok(backend.list()?.into_iter().flat_map(|pkg| [pkg.name.to_lowercase(), pkg.source.to_lowercase()])))
        .collect::<Result<Vec<_>, BackendError>>()?
        .into_iter()
        .flatten()
        .collect();
    let on_path = |name: &str| std::env::split_paths(path_var).any(|dir| fs::metadata(dir.join(name)).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0));

    // programs rendered by modules plus static/<name>/ dirs, minus data dirs like fonts and wallpapers
    let rendered = build.outputs.iter().filter(|output| output.service.is_none() && !output.name.starts_with('.')).map(|output| output.name.clone());
    let static_dirs = fs::read_dir(repo.static_dir()).into_iter().flatten().filter_map(|entry| {
        let entry = entry.ok()?;
        entry.file_type().ok()?.is_dir().then(|| entry.file_name().to_string_lossy().into_owned())
    });
    let is_data = |name: &str| {
        let entry = build.registry.entry(name);
        entry.dir.is_some() && entry.files.is_empty()
    };
    let names: BTreeSet<String> = rendered.chain(static_dirs).filter(|name| !is_data(name)).collect();
    Ok(names.into_iter().filter(|name| !installed.contains(&name.to_lowercase()) && !on_path(name)).collect())
}

// every pending step, planned without building out/ or touching links
pub fn status(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Vec<Step>, ActivateError> {
    Ok(pending(activate::plan_activation(env, runner, repo, Options { dry_run: true, ..Options::default() })?))
}

// a plan's steps, minus updates that are already live
fn pending(planned: activate::Planned) -> Vec<Step> {
    let sources: HashMap<&PathBuf, &PathBuf> = planned.wanted.iter().map(|file| (&file.destination, &file.source)).collect();

    // an update is pending only while out/ lags the module; static edits are live through the link already
    let lagging = |destination: &PathBuf| sources.get(destination).is_some_and(|source| planned.build.written.contains(source));
    planned.steps.iter().filter(|step| !matches!(step, Step::Update { destination } if !lagging(destination))).cloned().collect()
}

// what `activate --force` would change in each live file, including links it would remove
pub fn diff(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Vec<FileDiff>, ActivateError> {
    let planned = activate::plan_activation(env, runner, repo, Options { dry_run: true, ..Options::default() })?;
    let rendered: HashMap<&PathBuf, &String> = planned.build.outputs.iter().map(|output| (&output.out, &output.content)).collect();
    let read = |path: &PathBuf| fs::read(path).unwrap_or_default();

    let removals = planned.steps.iter().filter_map(|step| match step {
        Step::Unlink { destination } | Step::Delete { destination } => Some(FileDiff { destination: destination.clone(), live: read(destination), wanted: Vec::new() }),
        _ => None,
    });

    // a static file behind its own link is the same file, so it's skipped unread
    let changes = planned.wanted.iter().chain(&planned.copies).filter_map(|file| {
        let render = rendered.get(&file.source);
        if render.is_none() && links_to(&file.destination, &file.source) {
            return None;
        }
        let wanted = render.map_or_else(|| read(&file.source), |content| content.as_bytes().to_vec());
        let live = read(&file.destination);
        (live != wanted).then(|| FileDiff { destination: file.destination.clone(), live, wanted })
    });
    Ok(removals.chain(changes).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activate::{Ask, activate};
    use crate::testing::{fixture, write};

    struct Nobody;

    impl Ask for Nobody {
        fn ask(&self, _: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn status_lists_pending_links_without_making_them() {
        let fixture = fixture(&["foot"]);
        write(&fixture.env.home.join(".config/foot/foot.ini"), "hand written\n");

        let steps = status(&fixture.env, &fixture.runner, &fixture.repo).unwrap();
        let destination = fixture.env.home.join(".config/foot/foot.ini");
        assert_eq!(steps, [Step::Link { destination, backup: true }]);
        assert!(!fixture.repo.out_dir().join("foot").exists());
    }

    #[test]
    fn static_edits_are_not_pending() {
        let fixture = fixture(&["foot"]);
        write(&fixture.repo.static_dir().join("nvim/init.lua"), "-- lua\n");
        activate(&fixture.env, &fixture.runner, &fixture.repo, &Nobody, Options::default()).unwrap();
        write(&fixture.repo.static_dir().join("nvim/init.lua"), "-- edited\n");
        fs::write(fixture.repo.module_file("foot"), "# edited").unwrap();

        let foot = fixture.env.home.join(".config/foot/foot.ini");
        assert!(status(&fixture.env, &fixture.runner, &fixture.repo).unwrap().is_empty());
        fs::write(fixture.repo.out_dir().join("foot/foot.ini"), "stale\n").unwrap();
        assert_eq!(status(&fixture.env, &fixture.runner, &fixture.repo).unwrap(), [Step::Edited { destination: foot }]);
    }

    #[test]
    fn orphans_are_programs_with_neither_a_package_nor_a_command() {
        let fixture = fixture(&["foot", "bash", "tool"]);
        write(&fixture.repo.static_dir().join("wallpapers/sunset.png"), "");
        let bin = fixture.dir.path().join("bin");
        write(&bin.join("tool"), "");
        fs::set_permissions(bin.join("tool"), fs::Permissions::from_mode(0o755)).unwrap();

        // bash is an installed package, tool a command on PATH, wallpapers a data dir; foot is nowhere
        let full = report(&fixture.env, &fixture.runner, &fixture.repo, &bin.display().to_string()).unwrap();
        assert_eq!(full.orphans, ["foot"]);
        assert_eq!(full.undeclared.len(), 1);
    }

    #[test]
    fn diff_compares_live_content_with_what_maw_would_place() {
        let fixture = fixture(&["foot"]);
        write(&fixture.env.home.join(".config/foot/foot.ini"), "hand written\n");

        let diffs = diff(&fixture.env, &fixture.runner, &fixture.repo).unwrap();
        assert_eq!((diffs[0].live.as_slice(), diffs[0].wanted.as_slice()), (&b"hand written\n"[..], &b"foot v1\n"[..]));
        assert!(diffs[0].unified("foot.ini").unwrap().contains("-hand written\n+foot v1\n"));
    }

    #[test]
    fn nothing_to_diff_after_activating() {
        let fixture = fixture(&["foot"]);
        write(&fixture.repo.static_dir().join("nvim/init.lua"), "-- lua\n");
        activate(&fixture.env, &fixture.runner, &fixture.repo, &Nobody, Options::default()).unwrap();

        assert!(diff(&fixture.env, &fixture.runner, &fixture.repo).unwrap().is_empty());
        assert!(status(&fixture.env, &fixture.runner, &fixture.repo).unwrap().is_empty());
    }

    #[test]
    fn removed_module_diffs_to_nothing() {
        let fixture = fixture(&["foot"]);
        activate(&fixture.env, &fixture.runner, &fixture.repo, &Nobody, Options::default()).unwrap();
        fs::remove_file(fixture.repo.module_file("foot")).unwrap();

        let diffs = diff(&fixture.env, &fixture.runner, &fixture.repo).unwrap();
        assert_eq!((diffs[0].live.as_slice(), diffs[0].wanted.len()), (&b"foot v1\n"[..], 0));
    }
}
