use crate::activate::{self, ActivateError, Activation, Ask, Options, Step};
use crate::adopt::{self, Candidate};
use crate::backend::xbps::Xbps;
use crate::backend::SystemBackend;
use crate::build::{self, BuildError, Report};
use crate::edit;
use crate::env::Env;
use crate::generations;
use crate::help;
use crate::init::runit::Runit;
use crate::init::{InitBackend, Scope};
use crate::packages::{self, Change};
use crate::registry;
use crate::repo::Repo;
use crate::rollback;
use crate::services;
use crate::runner::{Runner, SystemRunner};
use crate::status;
use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "maw", version, about = "declarative system manager for void linux", disable_help_subcommand = true, after_help = "see: maw help")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "create a dotfiles repo, or adopt an existing one", after_help = "see: maw help setting up")]
    Init {
        #[arg(help = "where the repo is, or should be")]
        path: PathBuf,
    },
    #[command(about = "render changed modules into out/", after_help = "see: maw help building")]
    Build {
        #[arg(long, help = "overwrite out/ files edited in place, backing them up")]
        force: bool,
    },
    #[command(about = "build, then link everything into place", after_help = "see: maw help activating")]
    Activate {
        #[arg(long, help = "print the plan without changing anything")]
        dry_run: bool,
        #[arg(long, help = "replace drifted files, backing them up")]
        force: bool,
        #[arg(long, help = "don't commit or record a generation this time")]
        no_commit: bool,
    },
    #[command(about = "open a module, static dir, or config.nix (`config`) in $EDITOR, then activate", after_help = "see: maw help editing")]
    Edit {
        #[arg(help = "a module or static/ name, or `config`")]
        name: String,
        #[arg(long, help = "only edit; activate later")]
        no_activate: bool,
    },
    #[command(about = "create modules/<name>.nix, importing any live config, then edit it", after_help = "see: maw help writing a module")]
    New {
        #[arg(help = "the program, as the registry names it")]
        name: String,
        #[arg(long, help = "css, ini, json, kdl, keyValue, raw, shell, or toml; defaults to the registry's")]
        format: Option<String>,
        #[arg(long, help = "only create and edit; activate later")]
        no_activate: bool,
    },
    #[command(about = "copy a file or dir into static/, then activate so it's linked back in place", after_help = "see: maw help adding verbatim files")]
    Add {
        #[arg(help = "file or dir to copy")]
        path: PathBuf,
        #[arg(help = "registry name to file it under; inferred from the path when omitted")]
        name: Option<String>,
        #[arg(long, help = "only copy; activate later")]
        no_activate: bool,
    },
    #[command(about = "show what activating would change in each live file", after_help = "see: maw help checking")]
    Diff,
    #[command(about = "everything out of sync: files, packages, services, and config for missing programs", after_help = "see: maw help checking")]
    Status,
    #[command(about = "record installed packages and enabled services maw.nix doesn't know about", after_help = "see: maw help adopting")]
    Adopt {
        #[arg(long, help = "print the checklist without opening it")]
        dry_run: bool,
    },
    #[command(about = "install packages, record them in maw.nix, scaffold their modules, then activate", after_help = "see: maw help installing")]
    Install {
        #[arg(required = true, help = "names, or cargo:<crate|git url>, go:<path>, xbps:<name>; @version pins")]
        packages: Vec<String>,
        #[arg(long, help = "print what would change without changing anything")]
        dry_run: bool,
    },
    #[command(about = "remove packages and drop them from maw.nix; their modules stay", after_help = "see: maw help removing")]
    Remove {
        #[arg(required = true)]
        packages: Vec<String>,
        #[arg(long, help = "print what would change without changing anything")]
        dry_run: bool,
    },
    #[command(about = "list packages you installed, or show one", after_help = "see: maw help looking things up")]
    Query { package: Option<String> },
    #[command(about = "search xbps, or crates.io when xbps has nothing", after_help = "see: maw help looking things up")]
    Search {
        #[arg(help = "a term, or cargo:<term> to search crates.io directly")]
        term: String,
    },
    #[command(about = "a package's details, and whether maw manages it", after_help = "see: maw help looking things up")]
    Info { package: String },
    #[command(about = "upgrade the system, then every unpinned cargo and go package", after_help = "see: maw help updating")]
    Sync {
        #[arg(long, help = "first release the packages a rollback held back")]
        release: bool,
    },
    #[command(about = "restore a generation's repo and package versions, as a new generation", after_help = "see: maw help rolling back")]
    Rollback {
        #[arg(help = "the generation number from `maw generations`; the one before the latest if left out")]
        generation: Option<u32>,
        #[arg(long, help = "print what would change without changing anything")]
        dry_run: bool,
    },
    #[command(about = "services: list, enable, disable, status, restart, log", after_help = "see: maw help services")]
    Sv {
        #[command(subcommand)]
        action: SvAction,
    },
    #[command(about = "list generations: each activation that changed something", after_help = "see: maw help generations")]
    Generations,
    #[command(about = "commit every change in the repo", after_help = "see: maw help generations")]
    Commit {
        #[arg(short, long, help = "the message; without it git opens your editor")]
        message: Option<String>,
    },
    #[command(about = "push the repo to its remote", after_help = "see: maw help sharing")]
    Push,
    #[command(about = "pull the repo from its remote, then activate", after_help = "see: maw help sharing")]
    Pull,
    #[command(about = "read the docs: a topic, a command, or any section by its heading")]
    Help {
        #[arg(help = "usage, modules, formats, a command, or a heading like `drift`")]
        query: Vec<String>,
    },
}

#[derive(Subcommand)]
enum SvAction {
    #[command(about = "declared and enabled services, and whether they run")]
    List,
    #[command(about = "record a service in maw.nix and link it into place")]
    Enable(ServiceName),
    #[command(about = "stop and unlink a service, and drop it from maw.nix")]
    Disable(ServiceName),
    #[command(about = "sv status")]
    Status(ServiceName),
    #[command(about = "sv restart")]
    Restart(ServiceName),
    #[command(about = "follow a service's log")]
    Log(ServiceName),
}

#[derive(clap::Args)]
struct ServiceName {
    name: String,
    #[arg(long, conflicts_with = "system", help = "the user service run by your session")]
    user: bool,
    #[arg(long, help = "the system service run from boot")]
    system: bool,
}

impl ServiceName {
    // --user or --system picks the scope; otherwise maw works it out
    fn scope(&self) -> Option<Scope> {
        match (self.user, self.system) {
            (true, _) => Some(Scope::User),
            (_, true) => Some(Scope::System),
            _ => None,
        }
    }
}

pub fn main() -> Result<()> {
    let cli = Cli::parse();
    let env = Env::from_process();
    let runner = SystemRunner;

    match cli.command {
        Command::Init { path } => init(&env, &runner, &path),
        Command::Build { force } => build(&env, &runner, force),
        Command::Activate { dry_run, force, no_commit } => activate(&env, &runner, Options { dry_run, force }, !no_commit),
        Command::Generations => list_generations(&env),
        Command::Commit { message } => {
            let repo = Repo::locate(&env)?;
            if !generations::commit(&runner, &repo, message.as_deref())? {
                println!("nothing to commit");
            }
            Ok(())
        }
        Command::Push => Ok(generations::push(&runner, &Repo::locate(&env)?)?),
        Command::Pull => {
            let repo = Repo::locate(&env)?;
            let moved = generations::pull(&runner, &repo)?;
            activate_after(&env, &runner, &repo, if moved { vec!["pull".into()] } else { Vec::new() })
        }
        Command::Edit { name, no_activate } => edit(&env, &runner, &name, !no_activate),
        Command::New { name, format, no_activate } => new(&env, &runner, &name, format.as_deref(), !no_activate),
        Command::Add { path, name, no_activate } => add(&env, &runner, &path, name.as_deref(), !no_activate),
        Command::Diff => diff(&env, &runner),
        Command::Status => status(&env, &runner),
        Command::Adopt { dry_run } => adopt(&env, &runner, dry_run),
        Command::Install { packages, dry_run } => install(&env, &runner, &packages, dry_run),
        Command::Remove { packages, dry_run } => remove(&env, &runner, &packages, dry_run),
        Command::Query { package } => query(&env, &runner, package.as_deref()),
        Command::Search { term } => search(&env, &runner, &term),
        Command::Info { package } => info(&env, &runner, &package),
        Command::Sync { release } => sync(&env, &runner, release),
        Command::Rollback { generation, dry_run } => rollback(&env, &runner, generation, dry_run),
        Command::Sv { action } => sv(&env, &runner, action),
        Command::Help { query } => show_help(&runner, &query.join(" ")),
    }
}

// questions on the terminal; no answers when stdin isn't one
struct Terminal;

impl Ask for Terminal {
    fn ask(&self, question: &str) -> Option<String> {
        if !std::io::stdin().is_terminal() {
            return None;
        }
        print!("{question}");
        std::io::stdout().flush().ok()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).ok()?;
        Some(answer)
    }
}

// scaffolds the repo and lists what it created
fn init(env: &Env, runner: &dyn Runner, path: &Path) -> Result<()> {
    let (repo, created) = Repo::init(env, runner, path)?;
    created.iter().for_each(|path| println!("create {}", env.pretty(path)));
    println!("dotfiles at {}", env.pretty(&repo.root));
    Ok(())
}

// builds and lists evaluated modules, changed out/ files, and drift
fn build(env: &Env, runner: &dyn Runner, force: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let report = build::build(env, runner, &repo, build::Options { force, dry_run: false })?;
    print_build(env, &repo, &report);
    report.drifted.iter().for_each(|output| println!("drift {}: edited in place; --force to overwrite", env.pretty(&output.destination)));

    if report.is_empty() {
        println!("up to date");
    }
    Ok(())
}

// activates and prints the build, then each step of the plan
fn activate(env: &Env, runner: &dyn Runner, options: Options, commit: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let activation = activate::activate(env, runner, &repo, &Terminal, options)?;
    finish(env, runner, &repo, &activation, options.dry_run, commit, &[])
}

// prints an activation, then commits and records a generation if anything changed and autoCommit is on;
// done lists what the command did before activating, like `install foot`, which counts as a change
fn finish(env: &Env, runner: &dyn Runner, repo: &Repo, activation: &Activation, dry_run: bool, commit: bool, done: &[String]) -> Result<()> {
    print_activation(env, repo, activation, dry_run);
    if activation.build.is_empty() && activation.steps.is_empty() && activation.answered.is_empty() && done.is_empty() {
        println!("up to date");
    }
    if dry_run || !commit || !activation.build.settings.auto_commit || (!generations::changed(activation) && done.is_empty()) {
        return Ok(());
    }
    let summary: Vec<String> = done.iter().cloned().chain(generations::summary(repo, activation)).collect();
    let generation = generations::record(env, runner, repo, &summary, &Terminal)?;
    let short = generation.commit.get(..7).unwrap_or(&generation.commit);
    println!("generation {} ({short})", generation.number);
    Ok(())
}

// opens a file in the editor, then activates if asked; on an evaluation error, offers to reopen it
fn edit_then_activate(env: &Env, runner: &dyn Runner, repo: &Repo, path: &Path, activate: bool) -> Result<()> {
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
    loop {
        edit::open_editor(runner, &editor, path)?;
        if !activate {
            return Ok(());
        }
        match activate::activate(env, runner, repo, &Terminal, Options::default()) {
            Err(ActivateError::Build(BuildError::Eval(error))) => {
                eprintln!("error: {error}");
                let again = Terminal.ask("edit again? [Y/n] ").is_some_and(|answer| !answer.trim().to_lowercase().starts_with('n'));
                if !again {
                    return Err(error.into());
                }
            }
            result => return finish(env, runner, repo, &result?, false, true, &[]),
        }
    }
}

fn edit(env: &Env, runner: &dyn Runner, name: &str, activate: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let target = edit::edit_target(&repo, name)?;
    edit_then_activate(env, runner, &repo, &target, activate)
}

// scaffolds the module, then edits it like `maw edit`
fn new(env: &Env, runner: &dyn Runner, name: &str, format: Option<&str>, activate: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let module = edit::new_module(env, runner, &repo, name, format)?;
    println!("create {}", env.pretty(&module));
    edit_then_activate(env, runner, &repo, &module, activate)
}

// copies into static/, then activates so the original is backed up and linked
fn add(env: &Env, runner: &dyn Runner, path: &Path, name: Option<&str>, activate: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let copies = edit::add(env, runner, &repo, path, name)?;
    copies.iter().for_each(|(file, copy)| println!("copy {} -> {}", env.pretty(file), relative(&repo, copy)));
    if !activate {
        return Ok(());
    }
    let activation = activate::activate(env, runner, &repo, &Terminal, Options::default())?;
    finish(env, runner, &repo, &activation, false, true, &[])
}

// unified diffs per file, colored on a terminal
fn diff(env: &Env, runner: &dyn Runner) -> Result<()> {
    let repo = Repo::locate(env)?;
    let color = std::io::stdout().is_terminal();
    status::diff(env, runner, &repo)?.iter().for_each(|file| {
        let label = env.pretty(&file.destination);
        match file.unified(&label) {
            Some(text) => print!("{}", if color { colorize(&text) } else { text }),
            None => println!("binary {label} differs"),
        }
    });
    Ok(())
}

// + lines green, - lines red, hunk headers cyan
fn colorize(diff: &str) -> String {
    diff.lines()
        .map(|line| match line.chars().next() {
            Some('+') if !line.starts_with("+++") => format!("\x1b[32m{line}\x1b[0m\n"),
            Some('-') if !line.starts_with("---") => format!("\x1b[31m{line}\x1b[0m\n"),
            Some('@') => format!("\x1b[36m{line}\x1b[0m\n"),
            _ => format!("{line}\n"),
        })
        .collect()
}

// one line per out-of-sync file, like git status --short
fn status(env: &Env, runner: &dyn Runner) -> Result<()> {
    let repo = Repo::locate(env)?;
    let report = status::report(env, runner, &repo, &std::env::var("PATH").unwrap_or_default())?;
    let line = |label: &str, text: String| println!("{label:<11}{text}");
    let path = |path: &PathBuf| env.pretty(path);

    report.steps.iter().for_each(|step| match step {
        Step::Install { backend, package } => line("missing", packages::Target { backend: backend.clone(), spec: package.clone() }.to_string()),
        Step::Link { destination, backup: false } => line("new", path(destination)),
        Step::Link { destination, backup: true } => line("blocked", path(destination)),
        Step::Relink { destination } => line("moved", path(destination)),
        Step::Update { destination } => line("changed", path(destination)),
        Step::Unlink { destination } => line("stale", path(destination)),
        Step::Replaced { destination } => line("replaced", path(destination)),
        Step::Edited { destination } => line("edited", path(destination)),
        Step::Unplaced { file } => line("unplaced", format!("static/{}", file.display())),
        Step::Copy { destination, backup: true } => line("blocked", path(destination)),
        Step::Copy { destination, .. } if destination.exists() => line("changed", path(destination)),
        Step::Copy { destination, .. } => line("new", path(destination)),
        Step::Delete { destination } => line("stale", path(destination)),
        Step::Enable { scope, name } => line("disabled", service_label(*scope, name)),
        Step::Disable { scope, name } => line("stale", service_label(*scope, name)),
        Step::Purge { scope, name } => line("stale", path(&Runit.definition(env, *scope, name))),
        Step::Restart { scope, name } => line("restart", service_label(*scope, name)),
    });

    // things maw.nix doesn't know about, then config for programs that aren't installed
    report.undeclared.iter().for_each(|candidate| line("undeclared", candidate_label(candidate)));
    report.orphans.iter().for_each(|name| {
        let module = repo.module_file(name);
        let source = if module.exists() { relative(&repo, &module) } else { format!("static/{name}/") };
        line("orphan", format!("{source} ({name} isn't installed)"));
    });
    if report.is_clean() {
        println!("clean");
    }
    Ok(())
}

// packages the way maw install takes them, services with their scope
fn candidate_label(candidate: &Candidate) -> String {
    match candidate {
        Candidate::Package { backend, spec } => packages::Target { backend: backend.clone(), spec: spec.clone() }.to_string(),
        Candidate::Service { scope, name } => format!("{name} ({scope} service)"),
    }
}

// lists everything unrecorded in the editor, then records what's kept and ignores the rest
fn adopt(env: &Env, runner: &dyn Runner, dry_run: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let candidates = adopt::candidates(env, runner, &repo)?;
    if candidates.is_empty() {
        println!("nothing to adopt");
        return Ok(());
    }
    let checklist = adopt::checklist(&candidates);
    if dry_run {
        print!("{checklist}");
        return Ok(());
    }

    // the checklist lives in maw's state dir while it's being edited
    let file = env.state_dir.join("adopt");
    std::fs::create_dir_all(&env.state_dir)?;
    std::fs::write(&file, &checklist)?;
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
    edit::open_editor(runner, &editor, &file)?;
    let decisions = adopt::parse(&std::fs::read_to_string(&file)?, &candidates)?;
    std::fs::remove_file(&file)?;

    let (_, state) = edit::load(env, runner, &repo)?;
    adopt::apply(&repo, state, &decisions)?;
    decisions.iter().for_each(|(candidate, keep)| println!("{} {}", if *keep { "record" } else { "ignore" }, candidate_label(candidate)));
    let kept = decisions.iter().filter(|(_, keep)| *keep).count();
    activate_after(env, runner, &repo, vec![format!("adopt {kept}, ignore {}", decisions.len() - kept)])
}

// activates after a command that already changed something, which the generation's summary starts with
fn activate_after(env: &Env, runner: &dyn Runner, repo: &Repo, done: Vec<String>) -> Result<()> {
    let activation = activate::activate(env, runner, repo, &Terminal, Options::default())?;
    finish(env, runner, repo, &activation, false, true, &done)
}

// what an install or remove did, for a generation's summary: packages changed, then ones only (un)recorded
fn change_summary(change: &Change, verb: &str, record_verb: &str) -> Vec<String> {
    let changed = change.packages.iter().map(|target| format!("{verb} {target}"));
    let only_recorded = change.recorded.iter().filter(|target| !change.packages.contains(target)).map(|target| format!("{record_verb} {target}"));
    changed.chain(only_recorded).collect()
}

// prints the plan (asking before any crates.io fallback), then installs, records, scaffolds, and activates
fn install(env: &Env, runner: &dyn Runner, requests: &[String], dry_run: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let ask: Option<&dyn Ask> = if dry_run { None } else { Some(&Terminal) };
    let change = packages::plan_install(env, runner, &repo, requests, ask)?;
    print_change(&change, "install", "record {} in maw.nix");
    if dry_run {
        println!("dry run, nothing changed");
        return Ok(());
    }

    packages::install(env, runner, &repo, &change)?;
    let path_var = std::env::var("PATH").unwrap_or_default();
    packages::off_path(env, runner, &change, &path_var).iter().for_each(|dir| {
        let shown = env.pretty(dir).replacen('~', "$HOME", 1);
        eprintln!("warning: {} isn't on PATH; add `export PATH=\"{shown}:$PATH\"` to your shell profile", env.pretty(dir));
    });
    activate_after(env, runner, &repo, change_summary(&change, "install", "record"))
}

// prints the plan, then removes and drops from maw.nix
fn remove(env: &Env, runner: &dyn Runner, requests: &[String], dry_run: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let change = packages::plan_remove(env, runner, &repo, requests)?;
    print_change(&change, "remove", "drop {} from maw.nix");
    if dry_run {
        println!("dry run, nothing changed");
        return Ok(());
    }
    packages::remove(env, runner, &repo, &change)?;
    activate_after(env, runner, &repo, change_summary(&change, "remove", "drop"))
}

// one line per package, maw.nix entry (record_line has a {} for the name), and new module
fn print_change(change: &Change, verb: &str, record_line: &str) {
    change.packages.iter().for_each(|target| println!("{verb} {target}"));
    change.recorded.iter().for_each(|target| println!("{}", record_line.replace("{}", &target.to_string())));
    change.scaffolded.iter().for_each(|name| println!("create modules/{name}.nix"));
    if change.is_empty() {
        println!("nothing to do");
    }
}

// installed-by-hand and declared packages across backends, flagging any that disagree
fn query(env: &Env, runner: &dyn Runner, package: Option<&str>) -> Result<()> {
    let repo = Repo::locate(env)?;
    let mut rows = packages::overview(env, runner, &repo)?;
    rows.retain(|row| package.is_none_or(|wanted| row.name == wanted || row.target.spec == wanted || row.target.to_string() == wanted));
    rows.sort_by_key(|row| (row.target.backend != "xbps", row.name.to_lowercase()));
    if rows.is_empty() {
        anyhow::bail!("{} isn't installed", package.unwrap_or("nothing"));
    }

    rows.iter().for_each(|row| {
        let backend = if row.target.backend == "xbps" { String::new() } else { format!(" ({})", row.target.backend) };
        let note = match (&row.version, row.declared) {
            (None, _) => "  (missing)",
            (Some(_), false) => "  (undeclared)",
            _ => "",
        };
        println!("{} {}{backend}{note}", row.name, row.version.as_deref().unwrap_or("-"));
    });
    Ok(())
}

// repo matches, marked [*] when installed like xbps does, named the way `maw install` takes them
fn search(env: &Env, runner: &dyn Runner, term: &str) -> Result<()> {
    packages::search(env, runner, term)?.iter().for_each(|(target, pkg, installed)| {
        let mark = if *installed { "*" } else { "-" };
        println!("[{mark}] {target} {}  {}", pkg.version, pkg.description);
    });
    Ok(())
}

// the repo's description of a package plus what maw knows about it
fn info(env: &Env, runner: &dyn Runner, request: &str) -> Result<()> {
    let repo = Repo::locate(env)?;
    let description = packages::describe(env, runner, &repo, request)?;
    let (registry, _) = edit::load(env, runner, &repo)?;
    let (pkg, program) = (&description.pkg, crate::backend::spec_program(&description.target.spec));

    println!("{} {} ({})", pkg.name, pkg.version, description.target.backend);
    [&pkg.description, &pkg.homepage].iter().filter(|text| !text.is_empty()).for_each(|text| println!("  {text}"));
    println!("{:<10}{}", "installed", description.installed.as_deref().unwrap_or("no"));
    println!("{:<10}{}", "declared", if description.declared { description.target.to_string() } else { "no".into() });

    // where its config comes from in the repo, and where the registry puts it
    let config = [repo.module_file(&program), repo.static_dir().join(&program)].into_iter().find(|path| path.exists());
    println!("{:<10}{}", "config", config.as_ref().map_or("none".into(), |path| relative(&repo, path)));
    // destinations only when the registry really knows the program, not its ~/.config guess
    if registry.has(&program) || config.is_some() {
        let entry = registry.entry(&program);
        let specs: Vec<String> = if entry.files.is_empty() { vec![registry.spec(&program, "main")] } else { entry.files.values().cloned().collect() };
        specs.iter().for_each(|spec| println!("{:<10}{}", "goes to", env.pretty(&registry::resolve(env, spec))));
    }
    Ok(())
}

// upgrades the system through xbps, then every unpinned cargo and go package
fn sync(env: &Env, runner: &dyn Runner, release: bool) -> Result<()> {
    if release {
        rollback::release(env, runner)?.iter().for_each(|name| println!("release {name}"));
    }
    Xbps::new(runner, env).sync()?;
    let repo = Repo::locate(env)?;
    packages::upgrade(env, runner, &repo)?.iter().for_each(|target| println!("upgrade {target}"));
    Ok(())
}

// a topic, a command's --help, or a doc section, paged on a terminal
fn show_help(runner: &dyn Runner, query: &str) -> Result<()> {
    let terminal = std::io::stdout().is_terminal();
    let color = terminal && std::env::var_os("NO_COLOR").is_none();
    let Some(text) = help_text(query, color) else {
        anyhow::bail!("no help on {query}; `maw help` lists topics");
    };

    // $PAGER, else less; printing directly if there's no terminal or no pager
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less -FRX".into());
    let mut words = pager.split_whitespace().map(String::from);
    let paged = terminal && words.next().is_some_and(|program| runner.pipe(&program, &words.collect::<Vec<_>>(), &text).is_ok());
    if !paged {
        print!("{text}");
    }
    Ok(())
}

// the topic list for no query, else the first of: topic, command, section
fn help_text(query: &str, color: bool) -> Option<String> {
    if query.is_empty() {
        let topics: String = help::TOPICS.iter().map(|topic| format!("  {:<11}{}\n", topic.name, topic.summary)).collect();
        return Some(format!("topics:\n{topics}\nmaw help <topic | command | heading>, e.g. `maw help drift`\n"));
    }
    let render = |markdown: String| help::render(&markdown, color);
    if help::TOPICS.iter().any(|topic| topic.name == query) {
        return help::find(query).map(render);
    }

    let mut command = Cli::command();
    command.build();
    match command.find_subcommand_mut(query) {
        Some(subcommand) => Some(subcommand.render_long_help().to_string()),
        None => help::find(query).map(render),
    }
}

// runs one sv subcommand
fn sv(env: &Env, runner: &dyn Runner, action: SvAction) -> Result<()> {
    let repo = Repo::locate(env)?;
    match action {
        SvAction::List => sv_list(env, runner, &repo),
        SvAction::Enable(service) => {
            let scope = services::enable(env, runner, &repo, &service.name, service.scope())?;
            println!("record {} in maw.nix", service_label(scope, &service.name));
            let activation = activate::activate(env, runner, &repo, &Terminal, Options::default())?;
            finish(env, runner, &repo, &activation, false, true, &[])
        }
        SvAction::Disable(service) => {
            let (scope, dropped) = services::disable(env, runner, &repo, &service.name, service.scope())?;
            println!("disable {}", service_label(scope, &service.name));
            if dropped {
                println!("drop {} from maw.nix", service_label(scope, &service.name));
            }
            activate_after(env, runner, &repo, vec![format!("disable {}", service.name)])
        }
        SvAction::Status(service) => Ok(services::control(env, runner, &repo, &service.name, service.scope(), "status")?),
        SvAction::Restart(service) => Ok(services::control(env, runner, &repo, &service.name, service.scope(), "restart")?),
        SvAction::Log(service) => Ok(services::log(env, runner, &repo, &service.name, service.scope())?),
    }
}

// one line per service: name, scope, state, and whether maw.nix and the system disagree
fn sv_list(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<()> {
    services::list(env, runner, repo)?.iter().for_each(|row| {
        let note = match (row.declared, row.enabled) {
            (true, false) => "  (disabled)",
            (false, true) => "  (undeclared)",
            _ => "",
        };
        let state = row.state.as_deref().unwrap_or(if row.enabled { "?" } else { "-" });
        println!("{:<24}{:<8}{state}{note}", row.name, row.scope);
    });
    Ok(())
}

// prints the rollback plan, then restores the repo, changes package versions, activates, and records a generation
fn rollback(env: &Env, runner: &dyn Runner, number: Option<u32>, dry_run: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let plan = rollback::plan(env, runner, &repo, number)?;
    let generation = &plan.generation;
    println!("restore generation {} ({})", generation.number, generation.commit.get(..7).unwrap_or(&generation.commit));
    plan.removes.iter().for_each(|target| println!("remove {target}"));
    plan.versions.iter().for_each(|target| println!("install {target}"));
    plan.kept.iter().for_each(|(target, why)| println!("keep {target}: {why}"));
    plan.holds.iter().for_each(|name| println!("hold {name}"));
    if dry_run {
        println!("dry run, nothing changed");
        return Ok(());
    }

    rollback::rollback(env, runner, &repo, &plan)?;
    activate_after(env, runner, &repo, vec![format!("rollback to generation {}", generation.number)])
}

// one line per generation, newest last: number, time, commit, message
fn list_generations(env: &Env) -> Result<()> {
    let all = generations::load(env)?;
    if all.is_empty() {
        println!("no generations yet");
    }
    all.iter().for_each(|generation| {
        let short = generation.commit.get(..7).unwrap_or("-------");
        println!("{:>4}  {}  {short}  {}", generation.number, generation.time, generation.message);
    });
    Ok(())
}

// system services by name, user ones marked
fn service_label(scope: Scope, name: &str) -> String {
    if scope == Scope::User { format!("{name} (user)") } else { name.to_string() }
}

fn relative(repo: &Repo, path: &Path) -> String {
    path.strip_prefix(&repo.root).unwrap_or(path).display().to_string()
}

fn print_build(env: &Env, repo: &Repo, report: &Report) {
    let relative = |path: &PathBuf| relative(repo, path);
    report.evaluated.iter().for_each(|name| println!("eval {name}"));
    report.backups.iter().for_each(|(file, moved)| println!("backup {} -> {}", relative(file), env.pretty(moved)));
    report.written.iter().for_each(|path| println!("write {}", relative(path)));
    report.removed.iter().for_each(|path| println!("remove {}", relative(path)));
}

fn print_activation(env: &Env, repo: &Repo, activation: &Activation, dry_run: bool) {
    activation.answered.iter().for_each(|name| println!("record {name} in maw.nix"));
    print_build(env, repo, &activation.build);

    // a backup's line names where it went, once it has gone somewhere
    let moved_to = |destination: &PathBuf| activation.backups.iter().find(|(file, _)| file == destination).map(|(_, moved)| env.pretty(moved));
    activation.steps.iter().for_each(|step| match step {
        Step::Link { destination, backup: true } => {
            let target = moved_to(destination).map(|moved| format!(" -> {moved}")).unwrap_or_default();
            println!("backup {}{target}", env.pretty(destination));
            println!("link {}", env.pretty(destination));
        }
        Step::Link { destination, .. } => println!("link {}", env.pretty(destination)),
        Step::Install { backend, package } => println!("install {}", packages::Target { backend: backend.clone(), spec: package.clone() }),
        Step::Relink { destination } => println!("relink {}", env.pretty(destination)),
        Step::Update { destination } => println!("update {}", env.pretty(destination)),
        Step::Unlink { destination } => println!("unlink {}", env.pretty(destination)),
        Step::Replaced { destination } => println!("drift {}: replaced by another file; --force to relink", env.pretty(destination)),
        Step::Edited { destination } => println!("drift {}: edited in place; --force to overwrite", env.pretty(destination)),
        Step::Unplaced { file } => println!("skip static/{}: no destination; activate in a terminal to choose one", file.display()),
        Step::Copy { destination, backup: true } => {
            let target = moved_to(destination).map(|moved| format!(" -> {moved}")).unwrap_or_default();
            println!("backup {}{target}", env.pretty(destination));
            println!("copy {}", env.pretty(destination));
        }
        Step::Copy { destination, .. } => println!("copy {}", env.pretty(destination)),
        Step::Delete { destination } => println!("delete {}", env.pretty(destination)),
        Step::Enable { scope, name } => println!("enable {}", service_label(*scope, name)),
        Step::Disable { scope, name } => println!("disable {}", service_label(*scope, name)),
        Step::Purge { scope, name } => println!("remove {}", env.pretty(&Runit.definition(env, *scope, name))),
        Step::Restart { scope, name } => println!("restart {}", service_label(*scope, name)),
    });

    if dry_run && !activation.steps.is_empty() {
        println!("dry run, nothing linked");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_see_pointer_resolves() {
        Cli::command().get_subcommands().filter_map(|subcommand| subcommand.get_after_help()).for_each(|after| {
            let query = after.to_string().trim_start_matches("see: maw help ").to_string();
            assert!(help::find(&query).is_some(), "{query}");
        });
    }

    #[test]
    fn help_resolves_topics_commands_and_sections() {
        assert!(help_text("", false).unwrap().contains("formats"));
        assert!(help_text("formats", false).unwrap().starts_with("Formats"));
        assert!(help_text("add", false).unwrap().contains("Usage: maw add"));
        assert!(help_text("drift", false).unwrap().starts_with("Drift"));
        assert!(help_text("nonsense words", false).is_none());
    }
}
