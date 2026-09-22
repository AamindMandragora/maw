use crate::activate::{self, Activation, Ask, Options, Step};
use crate::build::{self, Report};
use crate::env::Env;
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
}

pub fn main() -> Result<()> {
    let cli = Cli::parse();
    let env = Env::from_process();
    let runner = SystemRunner;

    match cli.command {
        Command::Init { path } => init(&env, &runner, &path),
        Command::Build { force } => build(&env, &runner, force),
        Command::Activate { dry_run, force } => activate(&env, &runner, Options { dry_run, force }),
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
    let report = build::build(env, runner, &repo, force)?;
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
    print_activation(env, &repo, &activation, options.dry_run);

    if activation.build.is_empty() && activation.steps.is_empty() && activation.answered.is_empty() {
        println!("up to date");
    }
    Ok(())
}

fn print_build(env: &Env, repo: &Repo, report: &Report) {
    let relative = |path: &PathBuf| path.strip_prefix(&repo.root).unwrap_or(path).display().to_string();
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
