use super::app::{Row, Tab};
use crate::activate::walk;
use crate::cli;
use crate::env::Env;
use crate::generations;
use crate::init::runit::Runit;
use crate::init::{InitBackend, Scope};
use crate::packages;
use crate::repo::Repo;
use crate::runner::Runner;
use crate::services;
use crate::style;
use anyhow::Result;
use std::fs;

// a tab's rows, loaded fresh; errors become a row so the tab says what went wrong
pub fn load(env: &Env, runner: &dyn Runner, tab: Tab) -> Vec<Row> {
    let rows = Repo::locate(env).map_err(anyhow::Error::from).and_then(|repo| match tab {
        Tab::Packages => packages_rows(env, runner, &repo),
        Tab::Modules => modules_rows(&repo),
        Tab::Services => services_rows(env, runner, &repo),
        Tab::Status => Ok(cli::status_lines(env, runner, &repo)?.into_iter().map(|line| Row::text(line.clone()).toned(style::status_tone(&line))).collect()),
        Tab::Diff => Ok(text_rows(&cli::diff_text(env, runner, &repo)?, "no changes")),
        Tab::History => history_rows(env),
        Tab::Git => git_rows(runner, &repo),
    });
    rows.unwrap_or_else(|error| vec![Row::text(format!("error: {error:#}"))])
}

fn text_rows(text: &str, empty: &str) -> Vec<Row> {
    if text.trim().is_empty() { vec![Row::text(empty)] } else { text.lines().map(Row::text).collect() }
}

// installed-by-hand and declared packages, keyed the way `maw install` and `maw remove` take them
fn packages_rows(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Vec<Row>> {
    let mut rows = packages::overview(env, runner, repo)?;
    rows.sort_by_key(|row| (row.target.backend != "xbps", row.name.to_lowercase()));
    let note = |row: &packages::Row| match (&row.version, row.declared) {
        (None, _) => "missing",
        (Some(_), false) => "undeclared",
        _ => "",
    };
    Ok(rows
        .iter()
        .map(|row| {
            let cells = vec![row.name.clone(), row.version.clone().unwrap_or("-".into()), row.target.backend.clone(), note(row).into()];
            Row::new(cells, row.target.to_string()).toned(style::label_tone(note(row)))
        })
        .collect())
}

// repo search results, keyed for installing
pub fn find_rows(env: &Env, runner: &dyn Runner, term: &str) -> Vec<Row> {
    let found = match packages::search(env, runner, term) {
        Ok(found) => found,
        Err(error) => return vec![Row::text(format!("error: {error}"))],
    };
    let unreachable = found.unreachable.iter().map(|source| Row::text(format!("{source} didn't answer")));
    if found.hits.is_empty() {
        return std::iter::once(Row::text(format!("nothing matches {term}"))).chain(unreachable).collect();
    }

    // installed ones say so; aur and nixpkgs ones are drafted rather than installed
    let status = |target: &packages::Target, installed: bool| match installed {
        true => "installed",
        false if packages::DRAFTS.contains(&target.backend.as_str()) => "draft",
        false => "",
    };
    let hits = found.hits.iter().map(|(target, pkg, installed)| Row::new(vec![target.to_string(), pkg.version.clone(), status(target, *installed).into(), pkg.description.clone()], target.to_string()));
    hits.chain(unreachable).collect()
}

// modules/<name>.nix and static/<name>/, each once, with what the repo holds for it
fn modules_rows(repo: &Repo) -> Result<Vec<Row>> {
    let modules = repo.module_names()?;
    let static_dirs: Vec<String> = fs::read_dir(repo.static_dir())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().filter(|entry| entry.path().is_dir()).map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect();
    let mut names: Vec<String> = modules.iter().chain(&static_dirs).cloned().collect();
    names.sort();
    names.dedup();
    let kind = |name: &String| match (modules.contains(name), static_dirs.contains(name)) {
        (true, true) => "module + static",
        (true, false) => "module",
        _ => "static",
    };
    Ok(names.iter().map(|name| Row::new(vec![name.clone(), kind(name).into()], name.clone())).collect())
}

// declared and enabled services, keyed "<scope>:<name>"
fn services_rows(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Vec<Row>> {
    let rows = services::list(env, runner, repo)?;
    let note = |row: &services::Row| match (row.declared, row.enabled) {
        (true, false) => "disabled",
        (false, true) => "undeclared",
        _ => "",
    };
    Ok(rows
        .iter()
        .map(|row| {
            let state = row.state.clone().unwrap_or(if row.enabled { "?".into() } else { "-".into() });
            Row::new(vec![row.name.clone(), row.scope.to_string(), state, note(row).into()], format!("{}:{}", row.scope, row.name)).toned(style::label_tone(note(row)))
        })
        .collect())
}

// newest generation first, keyed by number
fn history_rows(env: &Env) -> Result<Vec<Row>> {
    let all = generations::load(env)?;
    if all.is_empty() {
        return Ok(vec![Row::text("no generations yet")]);
    }
    Ok(all
        .iter()
        .rev()
        .map(|generation| {
            let short = generation.commit.get(..7).unwrap_or("-------").to_string();
            Row::new(vec![generation.number.to_string(), generation.time.clone(), short, generation.message.clone()], generation.number.to_string())
        })
        .collect())
}

// git status, then the recent log
fn git_rows(runner: &dyn Runner, repo: &Repo) -> Result<Vec<Row>> {
    let git = |args: &[&str]| {
        let args: Vec<String> = ["-C".to_string(), repo.root.display().to_string()].into_iter().chain(args.iter().map(|arg| arg.to_string())).collect();
        runner.run("git", &args)
    };
    let status = git(&["status", "--short", "--branch"])?;
    let log = git(&["log", "--oneline", "-20"]).unwrap_or_default();
    Ok(status.lines().chain([""]).chain(log.lines()).map(Row::text).collect())
}

// what the side pane shows for a row: a module's rendered or static file, a service's log
pub fn preview(env: &Env, tab: Tab, key: &str) -> Vec<String> {
    let Ok(repo) = Repo::locate(env) else { return Vec::new() };
    match tab {
        Tab::Modules => {
            let files: Vec<_> = [repo.out_dir().join(key), repo.static_dir().join(key)].iter().flat_map(|dir| walk(dir).unwrap_or_default()).collect();
            let Some(first) = files.first() else { return vec!["nothing rendered yet".into()] };
            let text = fs::read_to_string(first).unwrap_or_else(|_| "(binary file)".into());
            std::iter::once(format!("{} ({} file{})", env.pretty(first), files.len(), if files.len() == 1 { "" } else { "s" }))
                .chain([String::new()])
                .chain(text.lines().map(String::from))
                .collect()
        }
        Tab::Services => {
            let (scope, name) = key.split_once(':').unwrap_or(("system", key));
            let file = Runit.log_file(env, Scope::parse(scope), name);
            let text = fs::read_to_string(&file).unwrap_or_else(|_| format!("no log at {}", env.pretty(&file)));
            let lines: Vec<String> = text.lines().map(String::from).collect();
            lines[lines.len().saturating_sub(200)..].to_vec()
        }
        _ => Vec::new(),
    }
}
