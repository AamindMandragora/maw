use crate::activate::{self, ActivateError, Activation, Ask, Options, Step};
use crate::build::{self, BuildError, Report};
use crate::edit;
use crate::env::Env;
use crate::status;
use crate::repo::Repo;
use crate::runner::{Runner, SystemRunner};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "maw", version, about = "declarative system manager for void linux")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "create a dotfiles repo, or adopt an existing one")]
    Init { path: PathBuf },
    #[command(about = "render changed modules into out/")]
    Build {
        #[arg(long, help = "overwrite out/ files edited in place, backing them up")]
        force: bool,
    },
    #[command(about = "build, then link everything into place")]
    Activate {
        #[arg(long, help = "print the plan without changing anything")]
        dry_run: bool,
        #[arg(long, help = "replace drifted files, backing them up")]
        force: bool,
    },
    #[command(about = "open a module, static dir, or config.nix (`config`) in $EDITOR, then activate")]
    Edit { name: String },
    #[command(about = "create modules/<name>.nix, importing any live config, then edit it")]
    New {
        name: String,
        #[arg(long, help = "css, ini, json, kdl, keyValue, raw, shell, or toml; defaults to the registry's")]
        format: Option<String>,
    },
    #[command(about = "copy a file or dir into static/, then activate so it's linked back in place")]
    Add {
        path: PathBuf,
        #[arg(help = "registry name to file it under; inferred from the path when omitted")]
        name: Option<String>,
    },
    #[command(about = "show what activating would change in each live file")]
    Diff,
    #[command(about = "list files out of sync with the repo")]
    Status,
}

pub fn main() -> Result<()> {
    let cli = Cli::parse();
    let env = Env::from_process();
    let runner = SystemRunner;

    match cli.command {
        Command::Init { path } => init(&env, &runner, &path),
        Command::Build { force } => build(&env, &runner, force),
        Command::Activate { dry_run, force } => activate(&env, &runner, Options { dry_run, force }),
        Command::Edit { name } => edit(&env, &runner, &name),
        Command::New { name, format } => new(&env, &runner, &name, format.as_deref()),
        Command::Add { path, name } => add(&env, &runner, &path, name.as_deref()),
        Command::Diff => diff(&env, &runner),
        Command::Status => status(&env, &runner),
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
fn activate(env: &Env, runner: &dyn Runner, options: Options) -> Result<()> {
    let repo = Repo::locate(env)?;
    let activation = activate::activate(env, runner, &repo, &Terminal, options)?;
    report_activation(env, &repo, &activation, options.dry_run)
}

fn report_activation(env: &Env, repo: &Repo, activation: &Activation, dry_run: bool) -> Result<()> {
    print_activation(env, repo, activation, dry_run);
    if activation.build.is_empty() && activation.steps.is_empty() && activation.answered.is_empty() {
        println!("up to date");
    }
    Ok(())
}

// opens a file in the editor, then activates; on an evaluation error, offers to reopen it
fn edit_then_activate(env: &Env, runner: &dyn Runner, repo: &Repo, path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into());
    loop {
        edit::open_editor(runner, &editor, path)?;
        match activate::activate(env, runner, repo, &Terminal, Options::default()) {
            Err(ActivateError::Build(BuildError::Eval(error))) => {
                eprintln!("error: {error}");
                let again = Terminal.ask("edit again? [Y/n] ").is_some_and(|answer| !answer.trim().to_lowercase().starts_with('n'));
                if !again {
                    return Err(error.into());
                }
            }
            result => return report_activation(env, repo, &result?, false),
        }
    }
}

fn edit(env: &Env, runner: &dyn Runner, name: &str) -> Result<()> {
    let repo = Repo::locate(env)?;
    let target = edit::edit_target(&repo, name)?;
    edit_then_activate(env, runner, &repo, &target)
}

// scaffolds the module, then edits it like `maw edit`
fn new(env: &Env, runner: &dyn Runner, name: &str, format: Option<&str>) -> Result<()> {
    let repo = Repo::locate(env)?;
    let module = edit::new_module(env, runner, &repo, name, format)?;
    println!("create {}", env.pretty(&module));
    edit_then_activate(env, runner, &repo, &module)
}

// copies into static/, then activates so the original is backed up and linked
fn add(env: &Env, runner: &dyn Runner, path: &Path, name: Option<&str>) -> Result<()> {
    let repo = Repo::locate(env)?;
    let copies = edit::add(env, runner, &repo, path, name)?;
    copies.iter().for_each(|(file, copy)| println!("copy {} -> {}", env.pretty(file), relative(&repo, copy)));
    let activation = activate::activate(env, runner, &repo, &Terminal, Options::default())?;
    report_activation(env, &repo, &activation, false)
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
    let steps = status::status(env, runner, &repo)?;
    let line = |label: &str, path: &PathBuf| println!("{label:<9}{}", env.pretty(path));

    steps.iter().for_each(|step| match step {
        Step::Link { destination, backup: false } => line("new", destination),
        Step::Link { destination, backup: true } => line("blocked", destination),
        Step::Relink { destination } => line("moved", destination),
        Step::Update { destination } => line("changed", destination),
        Step::Unlink { destination } => line("stale", destination),
        Step::Replaced { destination } => line("replaced", destination),
        Step::Edited { destination } => line("edited", destination),
        Step::Unplaced { file } => line("unplaced", &PathBuf::from("static").join(file)),
    });
    if steps.is_empty() {
        println!("clean");
    }
    Ok(())
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
        Step::Relink { destination } => println!("relink {}", env.pretty(destination)),
        Step::Update { destination } => println!("update {}", env.pretty(destination)),
        Step::Unlink { destination } => println!("unlink {}", env.pretty(destination)),
        Step::Replaced { destination } => println!("drift {}: replaced by another file; --force to relink", env.pretty(destination)),
        Step::Edited { destination } => println!("drift {}: edited in place; --force to overwrite", env.pretty(destination)),
        Step::Unplaced { file } => println!("skip static/{}: no destination; activate in a terminal to choose one", file.display()),
    });

    if dry_run && !activation.steps.is_empty() {
        println!("dry run, nothing linked");
    }
}
