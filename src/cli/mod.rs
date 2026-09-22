use crate::build;
use crate::env::Env;
use crate::repo::Repo;
use crate::runner::{Runner, SystemRunner};
use anyhow::Result;
use clap::{Parser, Subcommand};
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
    Build,
}

pub fn main() -> Result<()> {
    let cli = Cli::parse();
    let env = Env::from_process();
    let runner = SystemRunner;

    match cli.command {
        Command::Init { path } => init(&env, &runner, &path),
        Command::Build => build(&env, &runner),
    }
}

// scaffolds the repo and lists what it created
fn init(env: &Env, runner: &dyn Runner, path: &Path) -> Result<()> {
    let (repo, created) = Repo::init(env, runner, path)?;
    created.iter().for_each(|path| println!("create {}", env.pretty(path)));
    println!("dotfiles at {}", env.pretty(&repo.root));
    Ok(())
}

// builds and lists evaluated modules and changed out/ files
fn build(env: &Env, runner: &dyn Runner) -> Result<()> {
    let repo = Repo::locate(env)?;
    let report = build::build(env, runner, &repo)?;
    let relative = |path: &PathBuf| path.strip_prefix(&repo.root).unwrap_or(path).display().to_string();

    report.evaluated.iter().for_each(|name| println!("eval {name}"));
    report.written.iter().for_each(|path| println!("write {}", relative(path)));
    report.removed.iter().for_each(|path| println!("remove {}", relative(path)));

    if report.evaluated.is_empty() && report.written.is_empty() && report.removed.is_empty() {
        println!("up to date");
    }
    Ok(())
}
