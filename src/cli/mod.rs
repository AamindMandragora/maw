use crate::activate::{self, ActivateError, Activation, Ask, Options, Step};
use crate::adopt::{self, Candidate};
use crate::backend::SystemBackend;
use crate::backend::srcpkgs;
use crate::backend::xbps::Xbps;
use crate::build::{self, BuildError, Report};
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
use clap::{CommandFactory, Parser, Subcommand};
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
    #[command(about = "create a dotfiles repo, or adopt an existing one", after_help = "see: maw help setting up")]
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
        #[arg(long, help = "skip nix: activate the committed out/ from its index, e.g. on a fresh machine")]
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
    #[command(about = "record installed packages and enabled services maw.nix doesn't know about", after_help = "see: maw help adopting")]
    Adopt {
        #[arg(long, help = "print the checklist without opening it")]
        dry_run: bool,
    },
    #[command(about = "install packages, record them in maw.nix, scaffold their modules, then activate", after_help = "see: maw help installing")]
    Install {
        #[arg(required = true, help = "names or flatpak app ids, or cargo:<crate|git url>, go:<path>, uv:<pypi name|git url>, npm:<package>, flatpak:<app id>, xbps:<name>; @version pins")]
        packages: Vec<String>,
        #[arg(long, help = "print what would change without changing anything")]
        dry_run: bool,
        #[arg(long, help = "record them for this machine only")]
        here: bool,
    },
    #[command(about = "remove packages and drop them from maw.nix; their modules stay", after_help = "see: maw help removing")]
    Remove {
        #[arg(required = true, add = ArgValueCompleter::new(complete::packages))]
        packages: Vec<String>,
        #[arg(long, help = "print what would change without changing anything")]
        dry_run: bool,
    },
    #[command(about = "list packages you installed, or show one", after_help = "see: maw help looking things up")]
    Query {
        #[arg(add = ArgValueCompleter::new(complete::packages))]
        package: Option<String>,
    },
    #[command(about = "search every package source, then the aur and nixpkgs when none has it", after_help = "see: maw help looking things up")]
    Search {
        #[arg(help = "a term, or <source>:<term> to search one: xbps, flatpak, cargo, uv, npm, aur, nixpkgs")]
        term: String,
    },
    #[command(about = "a package's details, and whether maw manages it", after_help = "see: maw help looking things up")]
    Info {
        #[arg(add = ArgValueCompleter::new(complete::packages))]
        package: String,
    },
    #[command(about = "upgrade the system, then every unpinned cargo and go package", after_help = "see: maw help updating")]
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
    #[command(about = "show or set this machine's name, which picks its hosts/<name>.nix and own packages", after_help = "see: maw help more than one machine")]
    Host {
        #[arg(help = "a new name for this machine")]
        name: Option<String>,
    },
    #[command(about = "encrypted files in static/: edit one, or re-encrypt all for every machine", after_help = "see: maw help encrypted files")]
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },
    #[command(about = "set the wallpaper the theme comes from, or show it", after_help = "see: maw help wallpaper themes")]
    Wallpaper {
        #[arg(add = ArgValueCompleter::new(complete::wallpapers), help = "an image (copied into static/wallpapers/), a name already there, or random")]
        image: Option<String>,
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
    #[command(about = "packages built from your own xbps-src templates in srcpkgs/", after_help = "see: maw help source packages")]
    Src {
        #[command(subcommand)]
        action: SrcAction,
    },
    #[command(about = "read the docs: a topic, a command, or any section by its heading")]
    Help {
        #[arg(help = "usage, modules, formats, a command, or a heading like `drift`", add = ArgValueCompleter::new(complete::topics))]
        query: Vec<String>,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum SvAction {
    #[command(about = "declared and enabled services, and whether they run")]
    List,
    #[command(about = "record a service in maw.nix and link it into place")]
    Enable {
        #[command(flatten)]
        service: ServiceName,
        #[arg(long, help = "record it for this machine only")]
        here: bool,
    },
    #[command(about = "stop and unlink a service, and drop it from maw.nix")]
    Disable(ServiceName),
    #[command(about = "sv status")]
    Status(ServiceName),
    #[command(about = "sv restart")]
    Restart(ServiceName),
    #[command(about = "follow a service's log")]
    Log(ServiceName),
}

#[derive(Subcommand, Clone, Debug)]
pub enum SecretAction {
    #[command(about = "decrypt a file into your editor, then encrypt it back and activate")]
    Edit {
        #[arg(help = "the live file, or its static/<name>/<file>.age")]
        file: PathBuf,
    },
    #[command(about = "re-encrypt every file for every machine in hosts/*.pub, after adding one")]
    Rekey,
}

#[derive(Subcommand, Clone, Debug)]
pub enum SrcAction {
    #[command(about = "write a srcpkgs/<name>/template, blank or drafted from nixpkgs or the aur, and open it")]
    New {
        name: String,
        #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "ATTR", help = "draft it from a nixpkgs package; the attribute defaults to the name")]
        from_nix: Option<String>,
        #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "PKG", conflicts_with = "from_nix", help = "draft it from an aur package; the package defaults to the name")]
        from_aur: Option<String>,
    },
    #[command(about = "move a template to its upstream's current version or newest release, then rebuild")]
    Update {
        #[arg(add = ArgValueCompleter::new(complete::templates))]
        name: String,
    },
    #[command(about = "build a template with xbps-src; an installed package is upgraded to the new build")]
    Build {
        #[arg(add = ArgValueCompleter::new(complete::templates))]
        name: String,
    },
}

#[derive(clap::Args, Clone, Debug)]
pub struct ServiceName {
    #[arg(add = ArgValueCompleter::new(complete::services))]
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
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();
    let env = Env::from_process();
    match Cli::parse().command {
        Some(command) => run(&env, &SystemRunner, command),
        None => crate::tui::run(&env),
    }
}

// answers a question, or None when there's nobody to ask
pub type AskHook = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

// opens a file in the editor and waits for it to close
pub type EditHook = Box<dyn Fn(&Path) -> Result<()> + Send + Sync>;

// the whole command tree, for man pages
pub fn command() -> clap::Command {
    Cli::command()
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
            if !generations::commit(runner, &repo, message.as_deref())? {
                println!("nothing to commit");
            }
            Ok(())
        }
        Command::Push => Ok(generations::push(runner, &Repo::locate(env)?)?),
        Command::Pull => {
            let repo = Repo::locate(env)?;
            let moved = generations::pull(runner, &repo)?;
            activate_after(env, runner, &repo, if moved { vec!["pull".into()] } else { Vec::new() })
        }
        Command::Edit { name, no_activate } => edit(env, runner, &name, !no_activate),
        Command::New { name, format, no_activate } => new(env, runner, &name, format.as_deref(), !no_activate),
        Command::Add { path, name, no_activate, secret } => add(env, runner, &path, name.as_deref(), !no_activate, secret),
        Command::Diff => diff(env, runner),
        Command::Status => status(env, runner),
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
        print!("{question}");
        std::io::stdout().flush().ok()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).ok()?;
        Some(answer)
    }
}

// scaffolds a repo at a path, or clones one from a url and offers to activate it
fn init(env: &Env, runner: &dyn Runner, target: &str, clone_to: Option<&Path>) -> Result<()> {
    let is_url = target.contains("://") || target.starts_with("git@") || target.ends_with(".git");
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

// keeps this machine's name where maw and nix read it
fn set_host(env: &Env, host: &str) -> Result<()> {
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
    let env = &Env { host: name.to_string(), ..env.clone() };
    let repo = repo.for_host(name);
    let activation = activate::activate(env, runner, &repo, &Terminal, Options::default())?;
    finish(env, runner, &repo, &activation, false, false, &[])
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
    print!("{}", if std::io::stdout().is_terminal() { colorize(&text) } else { text });
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

// + lines green, - lines red, hunk headers cyan
fn colorize(diff: &str) -> String {
    diff.lines().map(|line| format!("{}\n", painted(style::diff_tone(line), line))).collect()
}

// text in a tone, or plain when there's no tone
fn painted(tone: Option<Tone>, text: &str) -> String {
    tone.map_or(text.to_string(), |tone| style::paint(tone, text, false))
}

// the `error:` or `warning:` prefix, painted when stderr is a terminal
pub fn prefix(tone: Tone, word: &str) -> String {
    if std::io::stderr().is_terminal() { style::paint(tone, word, true) } else { word.to_string() }
}

// one line per out-of-sync file, like git status --short
fn status(env: &Env, runner: &dyn Runner) -> Result<()> {
    let repo = Repo::locate(env)?;
    let color = std::io::stdout().is_terminal();
    status_lines(env, runner, &repo)?.iter().for_each(|line| println!("{}", if color { painted(style::status_tone(line), line) } else { line.clone() }));
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
    });

    // source packages behind their template, things maw.nix doesn't know about, then config for programs that aren't installed
    let outdated = report.outdated.iter().map(|(name, installed, template)| line("outdated", format!("{name} {installed} -> {template} (maw src build {name})")));
    let undeclared = report.undeclared.iter().map(|candidate| line("undeclared", candidate_label(candidate)));
    let orphans = report.orphans.iter().map(|name| {
        let module = repo.module_file(name);
        let source = if module.exists() { relative(repo, &module) } else { format!("static/{name}/") };
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
    if let Some(published) = secrets::ensure_recipient(&repo, &key)? {
        println!("record {}", relative(&repo, &published));
    }
    match action {
        SecretAction::Edit { file } => {
            // decrypted into a private scratch file, and encrypted back only if it changed
            let encrypted = secrets::find(env, &repo, &file)?;
            let temp = secrets::scratch(env, &encrypted)?;
            secrets::decrypt(runner, &repo, &key, &encrypted, &temp)?;
            let before = std::fs::read(&temp)?;
            let edited = open_in_editor(runner, &temp).map(|_| std::fs::read(&temp));
            let changed = matches!(&edited, Ok(Ok(after)) if *after != before);
            if changed {
                secrets::encrypt(runner, &repo, &temp, &encrypted)?;
                secrets::store(env, &repo, &encrypted, &temp, &mut crate::inputs::Inputs::default())?;
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
            activate_after(env, runner, &repo, vec![format!("rekey {} files", done.len())])
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

// prints the plan (asking before any crates.io fallback), then installs, records, scaffolds, and activates
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
        let note = if std::io::stdout().is_terminal() { painted(style::note_tone(note), note) } else { note.to_string() };
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

// upgrades the system through xbps, then every unpinned cargo and go package
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
    templates.filter(|(file, _)| file.is_file()).for_each(|(file, name)| match scaffold::releases::update(env, runner, &file) {
        Ok(Some((old, new))) => println!("update {name} {old} -> {new}"),
        Ok(None) => {}
        Err(error) => eprintln!("{} {name}: {error}", prefix(Tone::Warn, "warning:")),
    });

    // source packages whose templates moved ahead are rebuilt
    packages::outdated(env, runner, &repo, &state.here(&env.host))?.into_iter().try_for_each(|(name, installed, template)| {
        println!("rebuild {name} {installed} -> {template}");
        packages::build_source(env, runner, &repo, &name).map(|_| ())
    })?;
    packages::srcpkgs(env, runner, &repo)?.unpatch()?.iter().for_each(|name| println!("unpatch {name}"));
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
        let note = if std::io::stdout().is_terminal() { painted(style::note_tone(note), note) } else { note.to_string() };
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
        Step::Setting { key, value } => println!("set {key} {value}"),
        Step::SettingEdited { key } => println!("drift {key}: changed since maw set it; --force to overwrite"),
        Step::SettingReset { key } => println!("reset {key}"),
        Step::SettingsSkipped { reason } => println!("skip settings: {reason}"),
        Step::Reload { name, .. } => println!("reload {name}"),
        Step::SecretSkipped { file, reason } => println!("skip {file}: {reason}"),
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
