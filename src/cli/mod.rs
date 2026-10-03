use crate::activate::{self, ActivateError, Activation, Ask, Options, Step};
use crate::adopt::{self, Candidate};
use crate::backend::SystemBackend;
use crate::backend::srcpkgs;
use crate::backend::xbps::Xbps;
use crate::build::{self, BuildError, Report};
use crate::history;
use crate::complete;
use crate::edit;
use crate::env::Env;
use crate::generations;
use crate::help;
use crate::init::runit::Runit;
use crate::init::{InitBackend, Scope};
use crate::packages::{self, Change};
use crate::registry;
use crate::secrets;
use crate::repo::Repo;
use crate::rollback;
use crate::scaffold;
use crate::services;
use crate::runner::{Runner, SystemRunner};
use crate::status;
use crate::style::{self, Tone};
use anyhow::Result;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use clap_complete::ArgValueCompleter;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "maw", version, about = "declarative system manager for void linux", disable_help_subcommand = true, after_help = "see: maw help")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Clone, Debug)]
pub enum Command {
    #[command(about = "create a dotfiles repo, use an existing one, or clone one", after_help = "see: maw help setting up")]
    Init {
        #[arg(help = "a path for a new or existing repo, or a git url to clone")]
        target: String,
        #[arg(help = "where to clone a url to; ~/dotfiles by default")]
        path: Option<PathBuf>,
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
        #[arg(long, help = "skip nix: activate the committed out/<machine>/ from its index")]
        no_build: bool,
    },
    #[command(about = "open a module, static dir, or config.nix (`config`) in $EDITOR, then activate", after_help = "see: maw help editing")]
    Edit {
        #[arg(help = "a module or static/ name, or `config`", add = ArgValueCompleter::new(complete::modules))]
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
        #[arg(long, help = "encrypt it into static/ instead, for every machine's key")]
        secret: bool,
    },
    #[command(about = "show what activating would change in each live file", after_help = "see: maw help checking")]
    Diff,
    #[command(about = "everything out of sync: files, packages, services, and config for missing programs", after_help = "see: maw help checking")]
    Status,
    #[command(about = "check the session for problems that break a desktop quietly, like a missing session bus", after_help = "see: maw help doctor")]
    Doctor,
    #[command(about = "record installed packages and enabled services maw.nix doesn't know about", after_help = "see: maw help adopting")]
    Adopt {
        #[arg(long, help = "print the checklist without opening it")]
        dry_run: bool,
    },
    #[command(about = "install packages, record them in maw.nix, scaffold their modules, then activate", after_help = "see: maw help installing")]
    Install {
        #[arg(required = true, help = "names or flatpak app ids, or cargo:<crate|git url>, go:<path>, uv:<pypi name|git url>, npm:<package>, flatpak:<app id>, xbps:<name>; aur:<name> or nixpkgs:<attr> drafts a template; @version pins")]
        packages: Vec<String>,
        #[arg(long, help = "print what would change without changing anything")]
        dry_run: bool,
        #[arg(long, help = "record them for this machine only")]
        here: bool,
    },
    #[command(about = "remove packages and drop them from maw.nix; their modules stay", after_help = "see: maw help removing")]
    Remove {
        #[arg(required = true, add = ArgValueCompleter::new(complete::packages), help = "declared or installed packages, by name or with their source's prefix")]
        packages: Vec<String>,
        #[arg(long, help = "print what would change without changing anything")]
        dry_run: bool,
    },
    #[command(about = "list packages you installed, or show one", after_help = "see: maw help looking things up")]
    Query {
        #[arg(add = ArgValueCompleter::new(complete::packages), help = "one package; all of them without it")]
        package: Option<String>,
    },
    #[command(about = "search every package source, then the aur and nixpkgs when none has it", after_help = "see: maw help looking things up")]
    Search {
        #[arg(help = "a term, or <source>:<term> to search one: xbps, flatpak, cargo, uv, npm, aur, nixpkgs")]
        term: String,
    },
    #[command(about = "a package's details, and whether maw manages it", after_help = "see: maw help looking things up")]
    Info {
        #[arg(add = ArgValueCompleter::new(complete::packages), help = "a package, installed or not")]
        package: String,
    },
    #[command(about = "upgrade the system, every unpinned package, and source packages", after_help = "see: maw help updating")]
    Sync {
        #[arg(long, help = "first release the packages a rollback held back")]
        release: bool,
    },
    #[command(about = "restore a generation's repo and package versions, as a new generation", after_help = "see: maw help rolling back")]
    Rollback {
        #[arg(help = "the generation number from `maw generations`; the one before the latest if left out", add = ArgValueCompleter::new(complete::generations))]
        generation: Option<u32>,
        #[arg(long, help = "print what would change without changing anything")]
        dry_run: bool,
    },
    #[command(about = "services: list, enable, disable, status, restart, log", after_help = "see: maw help services")]
    Sv {
        #[command(subcommand)]
        action: SvAction,
    },
    #[command(about = "show or set this machine's name", after_help = "see: maw help machines")]
    Host {
        #[arg(help = "a new name for this machine")]
        name: Option<String>,
    },
    #[command(about = "encrypted files in static/: edit one, or re-encrypt all for every machine", after_help = "see: maw help secrets")]
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },
    #[command(about = "set the wallpaper the theme comes from, or show it", after_help = "see: maw help themes")]
    Wallpaper {
        #[arg(add = ArgValueCompleter::new(complete::wallpapers), help = "an image (copied into static/wallpapers/), a name already there, or random")]
        image: Option<String>,
    },
    #[command(about = "list generations: each activation that changed something", after_help = "see: maw help history")]
    Generations,
    #[command(about = "commit every change in the repo", after_help = "see: maw help history")]
    Commit {
        #[arg(short, long, help = "the message; without it git opens your editor")]
        message: Option<String>,
    },
    #[command(about = "push the repo to its remote", after_help = "see: maw help sharing")]
    Push,
    #[command(about = "pull the repo from its remote, then activate", after_help = "see: maw help sharing")]
    Pull,
    #[command(about = "fold generations FROM through TO, both included, into one commit, rewriting the repo's history", after_help = "see: maw help squashing")]
    Squash {
        #[arg(help = "the first generation to fold in")]
        from: u32,
        #[arg(help = "the last generation to fold in")]
        to: u32,
        #[arg(short, long, help = "don't ask first")]
        yes: bool,
    },
    #[command(about = "packages built from your own xbps-src templates in srcpkgs/", after_help = "see: maw help source packages")]
    Src {
        #[command(subcommand)]
        action: SrcAction,
    },
    #[command(about = "read the manual: a chapter, a command, or any section by its heading")]
    Help {
        #[arg(help = "a chapter by number or name, a command, or a heading like `drift`", add = ArgValueCompleter::new(complete::topics))]
        query: Vec<String>,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum SvAction {
    #[command(about = "declared and enabled services, and whether they run", after_help = "see: maw help service commands")]
    List,
    #[command(about = "record a service in maw.nix and link it into place", after_help = "see: maw help service commands")]
    Enable {
        #[command(flatten)]
        service: ServiceName,
        #[arg(long, help = "record it for this machine only")]
        here: bool,
    },
    #[command(about = "stop and unlink a service, and drop it from maw.nix", after_help = "see: maw help service commands")]
    Disable(ServiceName),
    #[command(about = "show a service's state", after_help = "see: maw help service commands")]
    Status(ServiceName),
    #[command(about = "restart a service", after_help = "see: maw help service commands")]
    Restart(ServiceName),
    #[command(about = "follow a service's log", after_help = "see: maw help service commands")]
    Log(ServiceName),
}

#[derive(Subcommand, Clone, Debug)]
pub enum SecretAction {
    #[command(about = "decrypt a file into your editor, then encrypt it back and activate", after_help = "see: maw help secrets")]
    Edit {
        #[arg(help = "the live file, or its static/<name>/<file>.age")]
        file: PathBuf,
    },
    #[command(about = "re-encrypt every file for every machine in hosts/*.pub, after adding one", after_help = "see: maw help secrets")]
    Rekey,
}

#[derive(Subcommand, Clone, Debug)]
pub enum SrcAction {
    #[command(about = "write a srcpkgs/<name>/template, blank or drafted from nixpkgs or the aur, and open it", after_help = "see: maw help source packages")]
    New {
        #[arg(help = "the package's name, and srcpkgs/<name>/")]
        name: String,
        #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "ATTR", help = "draft it from a nixpkgs package; the attribute defaults to the name")]
        from_nix: Option<String>,
        #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "PKG", conflicts_with = "from_nix", help = "draft it from an aur package; the package defaults to the name")]
        from_aur: Option<String>,
    },
    #[command(about = "move a template to its upstream's current version or newest release, then rebuild", after_help = "see: maw help source packages")]
    Update {
        #[arg(add = ArgValueCompleter::new(complete::templates), help = "a template in srcpkgs/")]
        name: String,
    },
    #[command(about = "build a template with xbps-src; an installed package is upgraded to the new build", after_help = "see: maw help source packages")]
    Build {
        #[arg(add = ArgValueCompleter::new(complete::templates), help = "a template in srcpkgs/")]
        name: String,
    },
    #[command(about = "free the build cache: old builds nothing needs and dependencies no build uses", after_help = "see: maw help source packages")]
    Clean,
}

#[derive(clap::Args, Clone, Debug)]
pub struct ServiceName {
    #[arg(add = ArgValueCompleter::new(complete::services), help = "a service, by its name in /etc/sv or ~/.config/sv")]
    pub name: String,
    #[arg(long, conflicts_with = "system", help = "the user service run by your session")]
    pub user: bool,
    #[arg(long, help = "the system service run from boot")]
    pub system: bool,
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

// `maw` alone opens the tui; anything else is a command
pub fn main() -> Result<()> {
    // with COMPLETE set, the shell is asking what completes; this answers and exits
    clap_complete::CompleteEnv::with_factory(command).complete();
    let env = Env::from_process();
    let cli = Cli::from_arg_matches(&command().get_matches()).unwrap_or_else(|error| error.exit());
    match cli.command {
        Some(command) => run(&env, &SystemRunner, command),
        None => crate::tui::run(&env),
    }
}

// answers a question, or None when there's nobody to ask
pub type AskHook = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

// opens a file in the editor and waits for it to close
pub type EditHook = Box<dyn Fn(&Path) -> Result<()> + Send + Sync>;

// the whole command tree, for parsing, completion, and man pages, with maw's lowercase -h and -V
pub fn command() -> clap::Command {
    let version = clap::Arg::new("version").short('V').long("version").action(clap::ArgAction::Version).help("print version");
    lowercase_help(Cli::command()).disable_version_flag(true).arg(version)
}

// a command and all its subcommands with `-h, --help  print help` in place of clap's capitalized one
fn lowercase_help(command: clap::Command) -> clap::Command {
    let help = clap::Arg::new("help").short('h').long("help").action(clap::ArgAction::Help).help("print help");
    command.disable_help_flag(true).arg(help).mut_subcommands(lowercase_help)
}

// what the tui plugs into the cli while it's running: popups for questions, and stepping aside for the editor
pub struct Hooks {
    pub ask: AskHook,
    pub edit: EditHook,
}

static HOOKS: std::sync::OnceLock<Hooks> = std::sync::OnceLock::new();

// installed once, by the tui
pub fn set_hooks(hooks: Hooks) {
    let _ = HOOKS.set(hooks);
}

// the user's editor: $VISUAL, then $EDITOR, then vi
pub fn editor() -> String {
    std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "vi".into())
}

// opens a file in the editor, or asks the tui to while it's running
fn open_in_editor(runner: &dyn Runner, path: &Path) -> Result<()> {
    match HOOKS.get() {
        Some(hooks) => (hooks.edit)(path),
        None => Ok(edit::open_editor(runner, &editor(), path)?),
    }
}

// runs one command; the tui calls this too, so every action is the cli's own code
pub fn run(env: &Env, runner: &dyn Runner, command: Command) -> Result<()> {
    match command {
        Command::Init { target, path } => init(env, runner, &target, path.as_deref()),
        Command::Build { force } => build(env, runner, force),
        Command::Activate { dry_run, force, no_commit, no_build } => activate(env, runner, Options { dry_run, force, no_build }, !no_commit),
        Command::Host { name } => host(env, runner, name.as_deref()),
        Command::Wallpaper { image } => wallpaper(env, runner, image.as_deref()),
        Command::Secret { action } => secret(env, runner, action),
        Command::Generations => list_generations(env),
        Command::Commit { message } => {
            let repo = Repo::locate(env)?;
            match generations::commit(runner, &repo, message.as_deref())? {
                true => auto_push(env, runner, &repo, false),
                false => println!("nothing to commit"),
            }
            Ok(())
        }
        Command::Push => Ok(generations::push(runner, &Repo::locate(env)?)?),
        Command::Pull => {
            let repo = Repo::locate(env)?;
            save_edited_out(env, &repo)?;
            let pulled = history::pull(runner, &repo)?;
            print_pull(&pulled);
            activate_after(env, runner, &repo, if pulled.moved { vec!["pull".into()] } else { Vec::new() })
        }
        Command::Squash { from, to, yes } => squash(env, runner, from, to, yes),
        Command::Edit { name, no_activate } => edit(env, runner, &name, !no_activate),
        Command::New { name, format, no_activate } => new(env, runner, &name, format.as_deref(), !no_activate),
        Command::Add { path, name, no_activate, secret } => add(env, runner, &path, name.as_deref(), !no_activate, secret),
        Command::Diff => diff(env, runner),
        Command::Status => status(env, runner),
        Command::Doctor => doctor(env, runner),
        Command::Adopt { dry_run } => adopt(env, runner, dry_run),
        Command::Install { packages, dry_run, here } => install(env, runner, &packages, dry_run, here),
        Command::Remove { packages, dry_run } => remove(env, runner, &packages, dry_run),
        Command::Query { package } => query(env, runner, package.as_deref()),
        Command::Search { term } => search(env, runner, &term),
        Command::Info { package } => info(env, runner, &package),
        Command::Sync { release } => sync(env, runner, release),
        Command::Rollback { generation, dry_run } => rollback(env, runner, generation, dry_run),
        Command::Sv { action } => sv(env, runner, action),
        Command::Src { action } => src(env, runner, action),
        Command::Help { query } => show_help(runner, &query.join(" ")),
    }
}

// questions on the terminal; no answers when stdin isn't one
struct Terminal;

impl Ask for Terminal {
    fn ask(&self, question: &str) -> Option<String> {
        if let Some(hooks) = HOOKS.get() {
            return (hooks.ask)(question);
        }
        if !std::io::stdin().is_terminal() {
            return None;
        }
        // on stderr, so the question shows even when output is piped; end of input (^D) is no answer, not the default
        eprint!("{question}");
        std::io::stderr().flush().ok()?;
        let mut answer = String::new();
        let read = std::io::stdin().lock().read_line(&mut answer).ok()?;
        (read > 0).then_some(answer)
    }
}

// scaffolds a repo at a path, or clones one from a url and offers to activate it
fn init(env: &Env, runner: &dyn Runner, target: &str, clone_to: Option<&Path>) -> Result<()> {
    let is_url = target.contains("://") || target.starts_with("git@") || target.ends_with(".git");
    if clone_to.is_some() && !is_url {
        anyhow::bail!("a second path is only for cloning: maw init <git url> <path>");
    }
    let (repo, created) = match is_url {
        true => Repo::clone(env, runner, target, &clone_to.map_or_else(|| env.home.join("dotfiles"), Path::to_path_buf))?,
        false => Repo::init(env, runner, Path::new(target))?,
    };
    created.iter().for_each(|path| println!("create {}", env.pretty(path)));
    println!("dotfiles at {}", env.pretty(&repo.root));

    // the machine's name decides what it gets, so it's settled before anything activates
    let env = &name_machine(env, &repo)?;
    let repo = repo.for_host(&env.host);
    if is_url { bootstrap(env, runner, &repo) } else { Ok(()) }
}

// machines the repo has a hosts/<name>.nix for
fn known_hosts(repo: &Repo) -> Vec<String> {
    let files = std::fs::read_dir(repo.root.join("hosts")).into_iter().flatten().filter_map(|entry| entry.ok()).map(|entry| entry.path());
    let mut names: Vec<String> = files.filter(|path| path.extension().is_some_and(|ext| ext == "nix")).filter_map(|path| Some(path.file_stem()?.to_string_lossy().into_owned())).collect();
    names.sort();
    names
}

// asks this machine's name the first time (the hostname by default, with the repo's hosts/ names as hints) and keeps it
fn name_machine(env: &Env, repo: &Repo) -> Result<Env> {
    if env.host_file().exists() {
        return Ok(env.clone());
    }
    let known = known_hosts(repo);
    let hint = if known.is_empty() { String::new() } else { format!(" (hosts/ has {})", known.join(", ")) };
    let answer = Terminal.ask(&format!("this machine's name [{}]{hint}: ", env.host)).map(|answer| answer.trim().to_string());
    let host = answer.filter(|answer| !answer.is_empty()).unwrap_or_else(|| env.host.clone());
    set_host(env, &host)?;
    Ok(Env { host, ..env.clone() })
}

// keeps this machine's name where maw and nix read it; a name is letters, digits, - _ and ., as it names hosts/ files
fn set_host(env: &Env, host: &str) -> Result<()> {
    let valid = !host.is_empty() && !host.starts_with('.') && host.chars().all(|char| char.is_ascii_alphanumeric() || "-_.".contains(char));
    if !valid {
        anyhow::bail!("{host:?} can't name a machine; use letters, digits, - _ and .");
    }
    std::fs::create_dir_all(&env.config_dir)?;
    std::fs::write(env.host_file(), format!("{host}\n"))?;
    Ok(())
}

// shows this machine's name, or renames it and activates for what the new name gets, without a generation
fn host(env: &Env, runner: &dyn Runner, name: Option<&str>) -> Result<()> {
    let repo = Repo::locate(env)?;
    let Some(name) = name else {
        let file = repo.root.join("hosts").join(format!("{}.nix", env.host));
        let note = if file.exists() { format!(", with {}", relative(&repo, &file)) } else { String::new() };
        println!("{}{note}", env.host);
        return Ok(());
    };
    set_host(env, name)?;
    println!("host {name}");
    let renamed = &Env { host: name.to_string(), ..env.clone() };
    let repo = repo.for_host(name);

    // the old name comes back if activating under the new one fails
    match activate::activate(renamed, runner, &repo, &Terminal, Options::default()) {
        Ok(activation) => finish(renamed, runner, &repo, &activation, false, false, &[]),
        Err(error) => {
            set_host(env, &env.host)?;
            Err(error.into())
        }
    }
}

// on a freshly cloned repo: show the plan, then activate if asked; without nix, from the committed index
fn bootstrap(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<()> {
    let no_build = runner.run("nix-instantiate", &["--version".into()]).is_err();
    if no_build {
        println!("no nix, using the committed out/");
    }
    let plan = activate::activate(env, runner, repo, &Terminal, Options { dry_run: true, no_build, ..Options::default() })?;
    print_activation(env, repo, &plan, true);

    let yes = Terminal.ask("activate now? [Y/n] ").is_some_and(|answer| !answer.trim().to_lowercase().starts_with('n'));
    if !yes {
        println!("later: maw activate{}", if no_build { " --no-build" } else { "" });
        return Ok(());
    }
    let activation = activate::activate(env, runner, repo, &Terminal, Options { no_build, ..Options::default() })?;
    finish(env, runner, repo, &activation, false, true, &["bootstrap".into()])
}

// builds and lists evaluated modules, changed out/ files, and drift
fn build(env: &Env, runner: &dyn Runner, force: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let report = build::build(env, runner, &repo, build::Options { force, ..build::Options::default() })?;
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
    activation.failed.iter().for_each(|(scope, name)| {
        let flag = if *scope == Scope::System { " --system" } else { "" };
        eprintln!("{} {} didn't come back up; `maw sv log {name}{flag}` shows why", prefix(Tone::Warn, "warning:"), service_label(*scope, name));
    });
    if activation.build.is_empty() && activation.steps.is_empty() && activation.answered.is_empty() && done.is_empty() {
        println!("up to date");
    }
    if dry_run || !commit || !activation.build.settings.auto_commit {
        return Ok(());
    }
    let by_hand = generations::hand_changes(runner, repo)?;
    if !generations::changed(repo, activation) && done.is_empty() && by_hand.is_empty() {
        return Ok(());
    }
    let summary: Vec<String> = done.iter().cloned().chain(generations::summary(repo, activation)).chain(by_hand).collect();
    let generation = generations::record(env, runner, repo, &summary, &Terminal)?;
    let short = generation.commit.get(..7).unwrap_or(&generation.commit);
    println!("generation {} ({short})", generation.number);
    auto_push(env, runner, repo, false);
    Ok(())
}

// pushes after a commit unless maw.autoPush is off; a push that fails (offline, the remote moved on) is a warning
fn auto_push(env: &Env, runner: &dyn Runner, repo: &Repo, force: bool) {
    if !build::settings(env, runner, repo).map_or(true, |settings| settings.auto_push) {
        return;
    }
    match history::push(runner, repo, force) {
        Ok(true) => println!("push"),
        Ok(false) => {}
        Err(_) => eprintln!("{} couldn't push; `maw pull`, then `maw push`", prefix(Tone::Warn, "warning:")),
    }
}

// what a pull did beyond moving: a rewritten history, this machine's commits put back on top or left aside
fn print_pull(pulled: &history::Pulled) {
    if pulled.rewritten {
        println!("the remote's history was rewritten (a squash)");
    }
    if pulled.replayed > 0 {
        println!("move {} unpushed commit{} on top", pulled.replayed, if pulled.replayed == 1 { "" } else { "s" });
    }
    let Some(backup) = &pulled.backup else { return };
    match pulled.stranded {
        0 => println!("keep the old history on branch {backup}; `git branch -D {backup}` once you don't need it"),
        count => eprintln!("{} {count} unpushed commit{} didn't apply; they're on branch {backup}: `git cherry-pick {backup}~{count}..{backup}`", prefix(Tone::Warn, "warning:"), if count == 1 { "" } else { "s" }),
    }
}

// shows what a squash folds together, asks, squashes, then force-pushes
fn squash(env: &Env, runner: &dyn Runner, from: u32, to: u32, yes: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let plan = history::plan_squash(env, runner, &repo, from, to)?;
    let numbers: Vec<String> = plan.generations.iter().map(u32::to_string).collect();
    println!("squash generations {} ({} commits) into one; {} later commits move on top", numbers.join(" "), plan.commits, plan.after);
    println!("this rewrites history: the remote is force-pushed, and other machines' `maw pull` puts their unpushed commits on top");
    if !yes {
        match Terminal.ask("squash? [y/N] ") {
            None if !std::io::stdin().is_terminal() => anyhow::bail!("no terminal to ask; `maw squash {from} {to} --yes` to squash"),
            Some(answer) if answer.trim().eq_ignore_ascii_case("y") => {}
            _ => anyhow::bail!("not squashed"),
        }
    }
    let backup = history::squash(env, runner, &repo, &plan)?;
    println!("generation {to} is generations {from}-{to}; the old history is on branch {backup}");
    auto_push(env, runner, &repo, true);
    Ok(())
}

// opens a file in the editor, then activates if asked; on an evaluation error, offers to reopen it
fn edit_then_activate(env: &Env, runner: &dyn Runner, repo: &Repo, path: &Path, activate: bool) -> Result<()> {
    loop {
        open_in_editor(runner, path)?;
        if !activate {
            return Ok(());
        }
        match activate::activate(env, runner, repo, &Terminal, Options::default()) {
            Err(ActivateError::Build(BuildError::Eval(error))) => {
                eprintln!("{} {error}", prefix(Tone::Bad, "error:"));
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

// copies (or encrypts) into static/, then activates so the original is backed up and linked
fn add(env: &Env, runner: &dyn Runner, path: &Path, name: Option<&str>, activate: bool, secret: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let key = if secret { Some(secrets::key(env, build::settings(env, runner, &repo)?.secret_key.as_deref())?) } else { None };
    let published = !secrets::recipients(&repo).iter().any(|file| file.file_stem().is_some_and(|stem| *stem == *repo.host));
    let copies = edit::add(env, runner, &repo, path, name, key.as_deref())?;
    if secret && published {
        println!("record hosts/{}.pub", repo.host);
    }
    let verb = if secret { "encrypt" } else { "copy" };
    copies.iter().for_each(|(file, copy)| println!("{verb} {} -> {}", env.pretty(file), relative(&repo, copy)));
    if !activate {
        return Ok(());
    }
    let activation = activate::activate(env, runner, &repo, &Terminal, Options::default())?;
    finish(env, runner, &repo, &activation, false, true, &[])
}

// unified diffs per file, colored on a terminal
fn diff(env: &Env, runner: &dyn Runner) -> Result<()> {
    let repo = Repo::locate(env)?;
    let text = diff_text(env, runner, &repo)?;
    print!("{}", if colored(&std::io::stdout()) { colorize(&text) } else { text });
    Ok(())
}

// every file's unified diff as one text; the diff tab shows the same
pub fn diff_text(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<String> {
    let files = status::diff(env, runner, repo)?;
    let texts = files.iter().map(|file| {
        let label = env.pretty(&file.destination);
        file.unified(&label).unwrap_or_else(|| format!("binary {label} differs\n"))
    });
    Ok(texts.collect())
}

// + lines accent, - lines bad, hunk headers dim
fn colorize(diff: &str) -> String {
    diff.lines().map(|line| format!("{}\n", painted(style::diff_tone(line), line))).collect()
}

// text in a tone, or plain when there's no tone
fn painted(tone: Option<Tone>, text: &str) -> String {
    tone.map_or(text.to_string(), |tone| style::paint(tone, text, false))
}

// whether to paint what goes to a stream: a terminal, and NO_COLOR unset
fn colored(stream: &impl IsTerminal) -> bool {
    stream.is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

// the `error:` or `warning:` prefix, painted when stderr takes color
pub fn prefix(tone: Tone, word: &str) -> String {
    if colored(&std::io::stderr()) { style::paint(tone, word, true) } else { word.to_string() }
}

// one line per out-of-sync file, like git status --short
fn status(env: &Env, runner: &dyn Runner) -> Result<()> {
    let repo = Repo::locate(env)?;
    let color = colored(&std::io::stdout());
    status_lines(env, runner, &repo)?.iter().for_each(|line| println!("{}", if color { painted(style::status_tone(line), line) } else { line.clone() }));
    Ok(())
}

// each problem with its fix under it, or `all good`; outside a repo, PATH isn't checked
fn doctor(env: &Env, runner: &dyn Runner) -> Result<()> {
    let state = Repo::locate(env).ok().and_then(|repo| edit::load(env, runner, &repo).ok()).map(|(_, state)| state.here(&env.host));
    let vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let found = crate::doctor::check(env, runner, &vars, state.as_ref());
    found.iter().for_each(|finding| println!("{} {}
     {}", prefix(Tone::Warn, "warn"), finding.problem, finding.fix));
    if found.is_empty() {
        println!("all good");
    }
    Ok(())
}

// the status report as labeled lines, `clean` when there's nothing; the status tab shows the same lines
pub fn status_lines(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Vec<String>> {
    let report = status::report(env, runner, repo, &std::env::var("PATH").unwrap_or_default())?;
    let line = |label: &str, text: String| format!("{label:<11}{text}");
    let path = |path: &PathBuf| env.pretty(path);

    let steps = report.steps.iter().map(|step| match step {
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
        Step::Setting { key, value } => line("changed", format!("{key} = {value}")),
        Step::SettingEdited { key } => line("edited", key.clone()),
        Step::SettingReset { key } => line("stale", key.clone()),
        Step::SettingsSkipped { reason } => line("skipped", format!("settings: {reason}")),
        Step::Reload { name, .. } => line("reload", name.clone()),
        Step::SecretSkipped { file, reason } => line("skipped", format!("{file}: {reason}")),
        Step::SecretBackedUp { file, backup } => line("edited", format!("{file}: edited copy at {}", path(backup))),
    });

    // source packages behind their template, things maw.nix doesn't know about, then config for programs that aren't installed
    let outdated = report.outdated.iter().map(|(name, installed, template)| line("outdated", format!("{name} {installed} -> {template} (maw src build {name})")));
    let undeclared = report.undeclared.iter().map(|candidate| line("undeclared", candidate_label(candidate)));
    let orphans = report.orphans.iter().map(|name| {
        let module = repo.module_file(name);
        let static_dir = repo.static_dir().join(name);
        let source = match (module.exists(), static_dir.exists()) {
            (true, _) => relative(repo, &module),
            (false, true) => relative(repo, &static_dir) + "/",
            // rendered by a module named something else
            (false, false) => relative(repo, &repo.out_dir().join(name)) + "/",
        };
        line("orphan", format!("{source} ({name} isn't installed)"))
    });
    let lines: Vec<String> = steps.chain(outdated).chain(undeclared).chain(orphans).collect();
    Ok(if lines.is_empty() { vec!["clean".into()] } else { lines })
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
    open_in_editor(runner, &file)?;
    let decisions = adopt::parse(&std::fs::read_to_string(&file)?, &candidates)?;
    std::fs::remove_file(&file)?;

    let (_, state) = edit::load(env, runner, &repo)?;
    adopt::apply(&repo, state, &env.host, &decisions)?;
    decisions.iter().for_each(|(candidate, choice)| {
        let verb = match choice {
            adopt::Choice::Keep => "record",
            adopt::Choice::Here => "record here",
            adopt::Choice::Skip => "ignore",
        };
        println!("{verb} {}", candidate_label(candidate));
    });
    let kept = decisions.iter().filter(|(_, choice)| *choice != adopt::Choice::Skip).count();
    activate_after(env, runner, &repo, vec![format!("adopt {kept}, ignore {}", decisions.len() - kept)])
}

// activates after a command that already changed something, which the generation's summary starts with
fn activate_after(env: &Env, runner: &dyn Runner, repo: &Repo, done: Vec<String>) -> Result<()> {
    let activation = activate::activate(env, runner, repo, &Terminal, Options::default())?;
    finish(env, runner, repo, &activation, false, true, &done)
}

// edits one encrypted file, or re-encrypts all of them for every machine, then activates
fn secret(env: &Env, runner: &dyn Runner, action: SecretAction) -> Result<()> {
    let repo = Repo::locate(env)?;
    let key = secrets::key(env, build::settings(env, runner, &repo)?.secret_key.as_deref())?;
    let published = secrets::ensure_recipient(&repo, &key)?.map(|published| relative(&repo, &published));
    published.iter().for_each(|published| println!("record {published}"));
    match action {
        SecretAction::Edit { file } => {
            // decrypted into a private scratch file, and encrypted back only if it changed
            let encrypted = secrets::find(env, &repo, &file)?;
            let temp = secrets::scratch(env, &encrypted)?;
            if temp.exists() {
                anyhow::bail!("{} holds unsaved edits from last time; move them into the file, then delete it", env.pretty(&temp));
            }
            secrets::decrypt(runner, &repo, &key, &encrypted, &temp)?;
            let before = std::fs::read(&temp)?;
            let edited = open_in_editor(runner, &temp).map(|_| std::fs::read(&temp));
            let changed = matches!(&edited, Ok(Ok(after)) if *after != before);

            // a failure to save keeps the edits in the scratch file rather than losing them
            if changed {
                let saved = secrets::encrypt(runner, &repo, &temp, &encrypted).and_then(|_| secrets::store(env, &repo, &encrypted, &temp, &mut crate::inputs::Inputs::default()));
                if let Err(error) = saved {
                    anyhow::bail!("{error}; your edits are in {}", env.pretty(&temp));
                }
            }
            std::fs::remove_file(&temp)?;
            edited??;
            if !changed {
                println!("no change");
                return Ok(());
            }
            println!("encrypt {}", relative(&repo, &encrypted));
            activate_after(env, runner, &repo, vec![format!("edit {}", relative(&repo, &encrypted))])
        }
        SecretAction::Rekey => {
            let (done, skipped) = secrets::rekey(env, runner, &repo, &key)?;
            done.iter().for_each(|file| println!("rekey {}", relative(&repo, file)));
            skipped.iter().for_each(|error| eprintln!("{} {error}", prefix(Tone::Warn, "warning:")));

            // a generation only when the repo changed: files rekeyed or this machine's key recorded
            let summary: Vec<String> = published.map(|published| format!("record {published}")).into_iter().chain((!done.is_empty()).then(|| format!("rekey {} files", done.len()))).collect();
            if summary.is_empty() {
                return Ok(());
            }
            activate_after(env, runner, &repo, summary)
        }
    }
}

// sets the wallpaper theme from an image, a name in static/wallpapers/, or a random one there, then activates without
// recording a generation; alone, shows the current one
fn wallpaper(env: &Env, runner: &dyn Runner, image: Option<&str>) -> Result<()> {
    let repo = Repo::locate(env)?;
    let describe = |theme: &crate::theme::Theme| format!("{}: color {} of {}, {} {}", theme.image, theme.index + 1, theme.count, theme.settings.mode, theme.colors.get("primary").map_or("", String::as_str));
    let Some(image) = image else {
        match crate::theme::load(env) {
            Some(theme) => println!("{}", describe(&theme)),
            None => println!("no wallpaper theme yet; `maw wallpaper <image>` sets one"),
        }
        return Ok(());
    };

    // a file on disk is copied in; otherwise it names one already in static/wallpapers/
    let name = match image {
        "random" => crate::theme::pick_random(env, &repo)?,
        path if Path::new(path).is_file() => {
            let (name, copied) = crate::theme::adopt(&repo, Path::new(path))?;
            if copied {
                println!("copy static/wallpapers/{name}");
            }
            name
        }
        name => name.to_string(),
    };
    let settings = build::settings(env, runner, &repo)?.theme.unwrap_or_default();
    let theme = crate::theme::set(env, runner, &repo, &name, &settings)?;
    println!("theme {}", describe(&theme));
    let activation = activate::activate(env, runner, &repo, &Terminal, Options::default())?;
    finish(env, runner, &repo, &activation, false, false, &[])
}

// what an install or remove did, for a generation's summary: packages changed, then ones only (un)recorded
fn change_summary(change: &Change, verb: &str, record_verb: &str) -> Vec<String> {
    let changed = change.packages.iter().map(|target| format!("{verb} {target}"));
    let only_recorded = change.recorded.iter().filter(|target| !change.packages.contains(target)).map(|target| format!("{record_verb} {target}"));
    changed.chain(only_recorded).collect()
}

// prints the plan (asking before using another source than xbps), then installs, records, scaffolds, and activates
fn install(env: &Env, runner: &dyn Runner, requests: &[String], dry_run: bool, here: bool) -> Result<()> {
    let repo = Repo::locate(env)?;
    let ask: Option<&dyn Ask> = if dry_run { None } else { Some(&Terminal) };
    let mut change = match packages::plan_install(env, runner, &repo, requests, ask) {
        Err(packages::PackagesError::Draftable { name, flag, options }) if !dry_run => return offer_draft(env, runner, &repo, packages::PackagesError::Draftable { name, flag, options }),
        change => change?,
    };
    change.here = here;
    let built = |target: &&packages::Target| target.backend == "xbps" && srcpkgs::is_source(&repo.srcpkgs_dir(), &target.spec);
    change.packages.iter().filter(built).for_each(|target| println!("build {}", target.spec));
    print_change(&change, "install", "record {} in maw.nix");
    if dry_run {
        println!("dry run, nothing changed");
        return Ok(());
    }

    packages::install(env, runner, &repo, &change)?;
    let path_var = std::env::var("PATH").unwrap_or_default();
    packages::off_path(env, runner, &change, &path_var).iter().for_each(|dir| {
        let shown = env.pretty(dir).replacen('~', "$HOME", 1);
        eprintln!("{} {} isn't on PATH; add `export PATH=\"{shown}:$PATH\"` to your shell profile", prefix(Tone::Warn, "warning:"), env.pretty(dir));
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
    match (rows.is_empty(), package) {
        (true, Some(package)) => anyhow::bail!("{package} isn't installed or declared"),
        (true, None) => {
            println!("nothing installed or declared");
            return Ok(());
        }
        _ => {}
    }

    rows.iter().for_each(|row| {
        let backend = if row.target.backend == "xbps" { String::new() } else { format!(" ({})", row.target.backend) };
        let note = match (&row.version, row.declared) {
            (None, _) => "  (missing)",
            (Some(_), false) => "  (undeclared)",
            _ => "",
        };
        let note = if colored(&std::io::stdout()) { painted(style::note_tone(note), note) } else { note.to_string() };
        println!("{} {}{backend}{note}", row.name, row.version.as_deref().unwrap_or("-"));
    });
    Ok(())
}

// repo matches, marked [*] when installed like xbps does, named the way `maw install` takes them
fn search(env: &Env, runner: &dyn Runner, term: &str) -> Result<()> {
    let found = packages::search(env, runner, term)?;
    found.unreachable.iter().for_each(|source| eprintln!("{} {source} didn't answer; skipped", prefix(Tone::Warn, "warning:")));

    // [*] installed, [-] installable, [~] draftable from the aur or nixpkgs
    let draftable = |target: &packages::Target| packages::DRAFTS.contains(&target.backend.as_str());
    found.hits.iter().for_each(|(target, pkg, installed)| {
        let mark = match (installed, draftable(target)) {
            (true, _) => "*",
            (false, true) => "~",
            (false, false) => "-",
        };
        println!("[{mark}] {target} {}  {}", pkg.version, pkg.description);
    });
    if found.hits.iter().any(|(target, ..)| draftable(target)) {
        println!("`maw install aur:<name>` or `nixpkgs:<name>` drafts a template");
    }
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

// upgrades the system through xbps, every unpinned package, then templates that follow releases and outdated source packages
fn sync(env: &Env, runner: &dyn Runner, release: bool) -> Result<()> {
    if release {
        rollback::release(env, runner)?.iter().for_each(|name| println!("release {name}"));
    }
    Xbps::new(runner, env).sync()?;
    let repo = Repo::locate(env)?;
    packages::upgrade(env, runner, &repo)?.iter().for_each(|target| println!("upgrade {target}"));

    // templates that follow a forge's releases move to the newest first, maw's own included; one that can't be checked is only a warning
    let (_, state) = edit::load(env, runner, &repo)?;
    let templates = packages::declared(&state.here(&env.host), "xbps").into_iter().map(|name| (repo.srcpkgs_dir().join(&name).join("template"), name));
    let updated: Vec<(String, String)> = templates
        .filter(|(file, _)| file.is_file())
        .filter_map(|(file, name)| match scaffold::releases::update(env, runner, &file) {
            Ok(Some((old, new))) => {
                println!("update {name} {old} -> {new}");
                Some((name, new))
            }
            Ok(None) => None,
            Err(error) => {
                eprintln!("{} {name}: {error}", prefix(Tone::Warn, "warning:"));
                None
            }
        })
        .collect();

    // source packages whose templates moved ahead are rebuilt; one that fails is a warning, and the rest still go
    let outdated = packages::outdated(env, runner, &repo, &state.here(&env.host))?;
    let failed: Vec<String> = outdated
        .into_iter()
        .filter_map(|(name, installed, template)| {
            println!("rebuild {name} {installed} -> {template}");
            let error = packages::build_source(env, runner, &repo, &name).err()?;
            eprintln!("{} {name}: {error}", prefix(Tone::Warn, "warning:"));
            Some(name)
        })
        .collect();
    packages::srcpkgs(env, runner, &repo)?.unpatch()?.iter().for_each(|name| println!("unpatch {name}"));

    // moved templates are repo changes, so they're activated and recorded like any
    if !updated.is_empty() {
        activate_after(env, runner, &repo, updated.iter().map(|(name, new)| format!("update {name} to {new}")).collect())?;
    }
    if !failed.is_empty() {
        anyhow::bail!("{} didn't build; `maw src build <name>` shows why", failed.join(", "));
    }
    Ok(())
}

// a chapter, a command's --help, or a section, paged on a terminal
fn show_help(runner: &dyn Runner, query: &str) -> Result<()> {
    let terminal = std::io::stdout().is_terminal();
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less -FRX".into());

    // color only through a pager that shows it: less given -R (on the command line or in $LESS), or none at all
    let less_colors = std::env::var("LESS").unwrap_or_default().contains(['R', 'r']) || pager.split_whitespace().any(|word| word.starts_with('-') && word.contains(['R', 'r']));
    let shows_color = !pager.split_whitespace().next().is_some_and(|program| program.ends_with("less")) || less_colors;
    let color = colored(&std::io::stdout()) && shows_color;
    let Some(text) = help_text(query, color) else {
        anyhow::bail!("no help on {query}; `maw help` lists the chapters");
    };

    // $PAGER, else less; printing directly if there's no terminal or no pager
    let mut words = pager.split_whitespace().map(String::from);
    let paged = terminal && words.next().is_some_and(|program| runner.pipe(&program, &words.collect::<Vec<_>>(), &text).is_ok());
    if !paged {
        print!("{text}");
    }
    Ok(())
}

// the chapter list for no query, else the first of: chapter, command, section
fn help_text(query: &str, color: bool) -> Option<String> {
    if query.is_empty() {
        let chapters: String = help::CHAPTERS.iter().enumerate().map(|(index, chapter)| format!("{:>3} {:<17}{}\n", index + 1, chapter.name.replace('-', " "), chapter.summary)).collect();
        return Some(format!("the maw manual:\n{chapters}\nmaw help <number | chapter | command | heading>, e.g. `maw help 2` or `maw help drift`\n"));
    }
    let render = |markdown: String| help::render(&markdown, color);
    if help::chapter(query).is_some() {
        return help::find(query).map(render);
    }

    // a command by its words (`sv enable`), unless its help only points back at a docs section of the same name
    let mut command = self::command();
    command.build();
    let found = query.split_whitespace().try_fold(&mut command, |command, word| command.find_subcommand_mut(word));
    match found {
        Some(subcommand) if subcommand.get_after_help().is_none_or(|after| after.to_string() != format!("see: maw help {query}")) => Some(subcommand.render_long_help().to_string()),
        _ => help::find(query).map(render),
    }
}

// a size the way du -h would round it, in MB or GB
fn megabytes(bytes: u64) -> String {
    match bytes as f64 / 1e6 {
        size if size >= 1000.0 => format!("{:.1} GB", size / 1000.0),
        size => format!("{size:.0} MB"),
    }
}

// runs one src subcommand
fn src(env: &Env, runner: &dyn Runner, action: SrcAction) -> Result<()> {
    let repo = Repo::locate(env)?;
    match action {
        SrcAction::New { name, from_nix, from_aur } => {
            let maintainer = maintainer(runner, &repo);

            // an upstream name left empty means the template's own name
            let named = |given: String| if given.is_empty() { name.clone() } else { given };
            let upstream = match (from_nix, from_aur) {
                (Some(attr), _) => scaffold::Upstream::Nix(named(attr)),
                (_, Some(aur)) => scaffold::Upstream::Aur(named(aur)),
                (None, None) => {
                    let template = packages::srcpkgs(env, runner, &repo)?.new_template(&name, &maintainer)?;
                    println!("create {}", relative(&repo, &template));
                    return open_in_editor(runner, &template);
                }
            };
            draft(env, runner, &repo, &name, &upstream, &maintainer)
        }
        SrcAction::Update { name } => match scaffold::update(env, runner, &repo, &name)? {
            None => {
                println!("{name} is up to date");
                Ok(())
            }
            Some((old, new)) => {
                println!("update {name} {old} -> {new}");
                let upgraded = packages::build_source(env, runner, &repo, &name)?;
                println!("{} {name}", if upgraded { "build and upgrade" } else { "build" });
                activate_after(env, runner, &repo, vec![format!("update {name} to {new}")])
            }
        },
        SrcAction::Clean => {
            let freed = packages::clean_sources(env, runner, &repo)?;
            println!("{}", if freed == 0 { "nothing to clean".to_string() } else { format!("freed {}", megabytes(freed)) });
            Ok(())
        }
        SrcAction::Build { name } => {
            let upgraded = packages::build_source(env, runner, &repo, &name)?;
            println!("{} {name}", if upgraded { "build and upgrade" } else { "build" });
            if upgraded {
                activate_after(env, runner, &repo, vec![format!("rebuild {name}")])?;
            }
            Ok(())
        }
    }
}

// the repo's git identity, for a template's maintainer line
fn maintainer(runner: &dyn Runner, repo: &Repo) -> String {
    let git = |key: &str| runner.run("git", &["-C".into(), repo.root.display().to_string(), "config".into(), key.into()]).unwrap_or_default().trim().to_string();
    format!("{} <{}>", git("user.name"), git("user.email"))
}

// for a name nothing installs: asks which upstream to draft a template from, drafts it, and says how to build it;
// with no one to ask, or no pick, the error itself says how
fn offer_draft(env: &Env, runner: &dyn Runner, repo: &Repo, error: packages::PackagesError) -> Result<()> {
    let packages::PackagesError::Draftable { name, options, .. } = &error else { return Err(error.into()) };
    let question = packages::choice_question(name, "isn't in any source maw installs from; draft a template from", options);
    let Some((target, _)) = Terminal.ask(&question).and_then(|answer| packages::pick(&answer, options).cloned()) else {
        return Err(error.into());
    };
    let name = name.clone();
    let upstream = match target.backend.as_str() {
        "aur" => scaffold::Upstream::Aur(target.spec),
        _ => scaffold::Upstream::Nix(target.spec),
    };
    draft(env, runner, repo, &name, &upstream, &maintainer(runner, repo))?;
    println!("`maw install {name}` builds and installs it");
    Ok(())
}

// drafts a template from nixpkgs or the aur, opens it, and offers to remember the dependency names the user fixed
fn draft(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str, upstream: &scaffold::Upstream, maintainer: &str) -> Result<()> {
    let (template, emitted) = scaffold::draft(env, runner, repo, name, upstream, maintainer)?;
    println!("create {}", relative(repo, &template));
    emitted.todos.iter().for_each(|todo| println!("todo {todo}: no void package"));
    open_in_editor(runner, &template)?;

    let after = std::fs::read_to_string(&template)?;
    let confirmed: Vec<(String, String)> = scaffold::learned(&emitted.text, &after, &emitted.todos)
        .into_iter()
        .filter(|(upstream, void)| Terminal.ask(&format!("record {upstream} -> {void} in depmap? [Y/n] ")).is_some_and(|answer| !answer.trim().to_lowercase().starts_with('n')))
        .collect();
    if !confirmed.is_empty() {
        scaffold::record(env, runner, repo, &confirmed)?;
        confirmed.iter().for_each(|(upstream, void)| println!("record {upstream} -> {void} in depmap.nix"));
    }
    Ok(())
}

// runs one sv subcommand
fn sv(env: &Env, runner: &dyn Runner, action: SvAction) -> Result<()> {
    let repo = Repo::locate(env)?;
    match action {
        SvAction::List => sv_list(env, runner, &repo),
        SvAction::Enable { service, here } => {
            let scope = services::enable(env, runner, &repo, &service.name, service.scope(), here)?;
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
        let note = if colored(&std::io::stdout()) { painted(style::note_tone(note), note) } else { note.to_string() };
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

    save_edited_out(env, &repo)?;
    let encrypted = rollback::rollback(env, runner, &repo, &plan)?;
    encrypted.iter().for_each(|file| println!("restore {file}; `maw secret rekey` encrypts it for every machine again"));
    activate_after(env, runner, &repo, vec![format!("rollback to generation {}", generation.number)])
}

// copies out/ files edited by hand to the backups dir, before a pull or rollback replaces out/
fn save_edited_out(env: &Env, repo: &Repo) -> Result<()> {
    build::save_edited_out(env, repo)?.iter().for_each(|(file, copy)| println!("backup {} -> {}", relative(repo, file), env.pretty(copy)));
    Ok(())
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
    // themed files are named like their out/ copies, under themed/
    let themed = build::themed_dir(env, repo);
    let relative = |path: &PathBuf| path.strip_prefix(&themed).map_or_else(|_| relative(repo, path), |file| format!("themed/{}", file.display()));
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
        Step::Unplaced { file } => println!("skip static/{}: no destination; `maw activate` in a terminal asks for one", file.display()),
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
        Step::Setting { key, value } => println!("set {key} {value}"),
        Step::SettingEdited { key } => println!("drift {key}: changed since maw set it; --force to overwrite"),
        Step::SettingReset { key } => println!("reset {key}"),
        Step::SettingsSkipped { reason } => println!("skip settings: {reason}"),
        Step::Reload { name, .. } => println!("reload {name}"),
        Step::SecretSkipped { file, reason } => println!("skip {file}: {reason}"),
        Step::SecretBackedUp { file, backup } => println!("backup {file}'s edited copy -> {}; `maw secret edit` keeps edits", env.pretty(backup)),
    });

    if dry_run && !activation.steps.is_empty() {
        println!("dry run, nothing changed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_see_pointer_resolves() {
        command().get_subcommands().filter_map(|subcommand| subcommand.get_after_help()).for_each(|after| {
            let query = after.to_string().trim_start_matches("see: maw help ").to_string();
            assert!(help::find(&query).is_some(), "{query}");
        });
    }

    #[test]
    fn help_resolves_chapters_commands_and_sections() {
        assert!(help_text("", false).unwrap().contains("  7 source packages  templates of your own"));
        assert!(help_text("formats", false).unwrap().starts_with("5. Formats"));
        assert!(help_text("2", false).unwrap().starts_with("2. Getting started"));
        assert!(help_text("add", false).unwrap().contains("Usage: maw add"));
        assert!(help_text("drift", false).unwrap().starts_with("Drift"));
        assert!(help_text("nonsense words", false).is_none());
    }
}
