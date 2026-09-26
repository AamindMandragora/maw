use crate::backend::srcpkgs;
use crate::build::index_file;
use crate::env::Env;
use crate::generations;
use crate::help::TOPICS;
use crate::index::Index;
use crate::inputs::load_json;
use crate::packages::Target;
use crate::repo::Repo;
use clap_complete::CompletionCandidate;
use clap_complete::env::{Bash, EnvCompleter, Fish, Zsh};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;

// tab completion for names that live in the dotfiles; everything here reads files and never runs nix, so it stays instant

// candidates starting with what's typed so far, each with an optional note
fn matching(current: &OsStr, names: impl IntoIterator<Item = (String, Option<String>)>) -> Vec<CompletionCandidate> {
    let typed = current.to_string_lossy();
    names
        .into_iter()
        .filter(|(name, _)| name.starts_with(typed.as_ref()))
        .map(|(name, note)| CompletionCandidate::new(name).help(note.map(Into::into)))
        .collect()
}

fn repo(env: &Env) -> Option<Repo> {
    Repo::locate(env).ok()
}

// the index activation keeps in out/, or an empty one before the first activation
fn index(repo: &Repo) -> Index {
    load_json(&index_file(repo)).unwrap_or_default()
}

// modules and static dirs, for `maw edit`
pub fn module_names(env: &Env) -> Vec<String> {
    let Some(repo) = repo(env) else { return Vec::new() };
    let modules = repo.module_names().unwrap_or_default();
    let static_dirs = fs::read_dir(repo.static_dir()).into_iter().flatten().filter_map(|entry| {
        let entry = entry.ok()?;
        entry.path().is_dir().then(|| entry.file_name().to_string_lossy().into_owned())
    });
    modules.into_iter().chain(static_dirs).collect::<BTreeSet<_>>().into_iter().chain(["config".to_string()]).collect()
}

// declared packages, the way `maw remove` and `maw info` take them
pub fn package_names(env: &Env) -> Vec<String> {
    let Some(repo) = repo(env) else { return Vec::new() };
    let state = index(&repo).state.here(&env.host);
    let targets = state.packages.iter().flat_map(|(backend, specs)| specs.iter().map(|spec| Target { backend: backend.clone(), spec: spec.clone() }.to_string()));
    targets.collect()
}

// declared services and ones modules define
pub fn service_names(env: &Env) -> Vec<String> {
    let Some(repo) = repo(env) else { return Vec::new() };
    let index = index(&repo);
    let listed = index.state.here(&env.host).services.into_values().flatten();
    let defined = index.files.iter().filter_map(|entry| entry.service.as_ref().map(|(name, _, _)| name.clone()));
    listed.chain(defined).collect::<BTreeSet<_>>().into_iter().collect()
}

// source packages in srcpkgs/, templates or patches, for `maw src build` and `maw src update`
pub fn template_names(env: &Env) -> Vec<String> {
    let Some(repo) = repo(env) else { return Vec::new() };
    let dirs = fs::read_dir(repo.srcpkgs_dir()).into_iter().flatten().filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()));
    dirs.filter(|name| srcpkgs::is_source(&repo.srcpkgs_dir(), name)).collect::<BTreeSet<_>>().into_iter().collect()
}

// wallpapers in static/wallpapers/, plus random, for `maw wallpaper`
pub fn wallpaper_names(env: &Env) -> Vec<String> {
    let Some(repo) = repo(env) else { return Vec::new() };
    crate::theme::wallpapers(&repo).into_iter().chain(["random".to_string()]).collect()
}

// generation numbers, newest first, with their messages as notes
pub fn generation_numbers(env: &Env) -> Vec<(String, Option<String>)> {
    let all = generations::load(env).unwrap_or_default();
    all.iter().rev().map(|generation| (generation.number.to_string(), Some(generation.message.clone()))).collect()
}

fn plain(names: Vec<String>) -> Vec<(String, Option<String>)> {
    names.into_iter().map(|name| (name, None)).collect()
}

// the completers clap calls, reading the real environment
pub fn modules(current: &OsStr) -> Vec<CompletionCandidate> {
    matching(current, plain(module_names(&Env::from_process())))
}

pub fn packages(current: &OsStr) -> Vec<CompletionCandidate> {
    matching(current, plain(package_names(&Env::from_process())))
}

pub fn services(current: &OsStr) -> Vec<CompletionCandidate> {
    matching(current, plain(service_names(&Env::from_process())))
}

pub fn templates(current: &OsStr) -> Vec<CompletionCandidate> {
    matching(current, plain(template_names(&Env::from_process())))
}

pub fn wallpapers(current: &OsStr) -> Vec<CompletionCandidate> {
    matching(current, plain(wallpaper_names(&Env::from_process())))
}

pub fn generations(current: &OsStr) -> Vec<CompletionCandidate> {
    matching(current, generation_numbers(&Env::from_process()))
}

// help topics with their summaries
pub fn topics(current: &OsStr) -> Vec<CompletionCandidate> {
    matching(current, TOPICS.iter().map(|topic| (topic.name.to_string(), Some(topic.summary.to_string()))))
}

// the scripts shells load: each asks `maw` itself what completes, through the COMPLETE variable
pub fn scripts() -> Vec<(String, String)> {
    let shells: [(&str, &dyn EnvCompleter); 3] = [("maw.bash", &Bash), ("_maw", &Zsh), ("maw.fish", &Fish)];
    shells
        .iter()
        .map(|(file, shell)| {
            let mut script = Vec::new();
            shell.write_registration("COMPLETE", "maw", "maw", "maw", &mut script).unwrap();
            (file.to_string(), String::from_utf8(script).unwrap())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activate::{Ask, Options, activate};
    use crate::testing::{fixture, write};

    struct Nobody;

    impl Ask for Nobody {
        fn ask(&self, _: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn names_come_from_the_repo_and_its_index() {
        let fixture = fixture(&["foot", "usersvc"]);
        write(&fixture.repo.static_dir().join("nvim/init.lua"), "");
        write(&fixture.repo.srcpkgs_dir().join("hello/template"), "pkgname=hello\n");
        fs::write(fixture.repo.maw_file(), "{\n  packages = {\n    cargo = [ \"bat\" ];\n  };\n  services = { };\n  paths = { };\n}\n").unwrap();
        activate(&fixture.env, &fixture.runner, &fixture.repo, &Nobody, Options::default()).unwrap();

        assert_eq!(module_names(&fixture.env), ["foot", "nvim", "usersvc", "config"]);
        assert_eq!(package_names(&fixture.env), ["cargo:bat"]);
        assert_eq!(service_names(&fixture.env), ["usersvc"]);
        assert_eq!(template_names(&fixture.env), ["hello"]);
    }

    #[test]
    fn candidates_match_what_is_typed() {
        let names = vec![("foot".to_string(), None), ("fuzzel".to_string(), Some("launcher".to_string())), ("niri".to_string(), None)];
        let found: Vec<String> = matching(OsStr::new("f"), names).iter().map(|candidate| candidate.get_value().to_string_lossy().into_owned()).collect();
        assert_eq!(found, ["foot", "fuzzel"]);
    }

    #[test]
    fn scripts_exist_for_three_shells() {
        let scripts = scripts();
        assert_eq!(scripts.iter().map(|(file, _)| file.as_str()).collect::<Vec<_>>(), ["maw.bash", "_maw", "maw.fish"]);
        assert!(scripts.iter().all(|(_, script)| script.contains("COMPLETE")));
    }
}
