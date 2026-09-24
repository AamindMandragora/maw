use crate::activate::{Activation, Ask, Step};
use crate::backend::{self, BackendError, NAMES};
use crate::env::Env;
use crate::repo::Repo;
use crate::runner::{RunError, Runner};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Component, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum GenerationsError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: bad generation entry: {source}")]
    Parse { path: PathBuf, source: serde_json::Error },
    #[error(transparent)]
    Run(#[from] RunError),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("{0} has no remote; add one with `git -C {0} remote add origin <url>`")]
    NoRemote(String),
    #[error("pull failed; if both sides have new commits, merge them with git in {0}, then run `maw activate`")]
    PullFailed(String),
}

// one activation that changed something: the commit it left, and exactly what was installed
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Generation {
    pub number: u32,
    pub time: String,
    pub commit: String,
    pub message: String,
    // backend -> pinned specs, e.g. xbps -> ["foot-1.28.0_1"]
    pub packages: BTreeMap<String, Vec<String>>,
}

fn log_file(env: &Env) -> PathBuf {
    env.state_dir.join("generations")
}

// every recorded generation, oldest first; one json object per line
pub fn load(env: &Env) -> Result<Vec<Generation>, GenerationsError> {
    let path = log_file(env);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(GenerationsError::Io { path, source }),
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|source| GenerationsError::Parse { path: path.clone(), source }))
        .collect()
}

// whether an activation did anything; drift and unplaced files are only reported
pub fn changed(activation: &Activation) -> bool {
    let acted = activation.steps.iter().any(|step| !matches!(step, Step::Replaced { .. } | Step::Edited { .. } | Step::Unplaced { .. }));
    acted || !activation.build.written.is_empty() || !activation.build.removed.is_empty() || !activation.answered.is_empty()
}

// the parts of a one-line summary: programs whose output changed, then counts and actions from the plan
pub fn summary(repo: &Repo, activation: &Activation) -> Vec<String> {
    // out/<name>/... or out/sv/<name>/... names the program or service
    let program = |path: &PathBuf| {
        let mut parts = path.strip_prefix(repo.out_dir()).ok()?.components().filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        });
        let first = parts.next()?;
        match first.as_str() {
            "sv" => parts.next(),
            name if name.starts_with('.') => None,
            _ => Some(first),
        }
    };
    let mut programs: Vec<String> = activation.build.written.iter().chain(&activation.build.removed).filter_map(program).collect();
    programs.dedup();

    let count = |matches: fn(&Step) -> bool, one: &str, many: &str| {
        let count = activation.steps.iter().filter(|step| matches(step)).count();
        (count > 0).then(|| format!("{count} {}", if count == 1 { one } else { many }))
    };
    let named = activation.steps.iter().filter_map(|step| match step {
        Step::Install { package, .. } => Some(format!("install {package}")),
        Step::Enable { name, .. } => Some(format!("enable {name}")),
        Step::Disable { name, .. } => Some(format!("disable {name}")),
        _ => None,
    });

    (!programs.is_empty())
        .then(|| programs.join(", "))
        .into_iter()
        .chain(count(|step| matches!(step, Step::Link { .. } | Step::Relink { .. }), "link", "links"))
        .chain(count(|step| matches!(step, Step::Unlink { .. }), "unlink", "unlinks"))
        .chain(count(|step| matches!(step, Step::Copy { .. }), "copy", "copies"))
        .chain(count(|step| matches!(step, Step::Delete { .. }), "delete", "deletes"))
        .chain(named)
        .collect()
}

// git args run in the repo
fn in_repo(repo: &Repo, args: &[&str]) -> Vec<String> {
    ["-C".to_string(), repo.root.display().to_string()].into_iter().chain(args.iter().map(|arg| arg.to_string())).collect()
}

fn git(runner: &dyn Runner, repo: &Repo, args: &[&str]) -> Result<String, RunError> {
    runner.run("git", &in_repo(repo, args))
}

// commits every change in the repo, if there are any; returns the commit maw is at afterwards
fn commit_all(runner: &dyn Runner, repo: &Repo, message: &str) -> Result<String, RunError> {
    if !git(runner, repo, &["status", "--porcelain"])?.trim().is_empty() {
        git(runner, repo, &["add", "-A"])?;
        git(runner, repo, &["commit", "-q", "-m", message])?;
    }
    Ok(git(runner, repo, &["rev-parse", "HEAD"]).map(|head| head.trim().to_string()).unwrap_or_default())
}

// commits everything with a message, or in git's editor without one; false if there was nothing to commit
pub fn commit(runner: &dyn Runner, repo: &Repo, message: Option<&str>) -> Result<bool, GenerationsError> {
    if git(runner, repo, &["status", "--porcelain"])?.trim().is_empty() {
        return Ok(false);
    }
    git(runner, repo, &["add", "-A"])?;
    match message {
        Some(message) => runner.interactive("git", &in_repo(repo, &["commit", "-m", message]))?,
        None => runner.interactive("git", &in_repo(repo, &["commit"]))?,
    }
    Ok(true)
}

// pushes the current branch to the first remote, setting it as upstream
pub fn push(runner: &dyn Runner, repo: &Repo) -> Result<(), GenerationsError> {
    let remotes = git(runner, repo, &["remote"])?;
    let Some(remote) = remotes.lines().next() else {
        return Err(GenerationsError::NoRemote(repo.root.display().to_string()));
    };
    Ok(runner.interactive("git", &in_repo(repo, &["push", "-u", remote, "HEAD"]))?)
}

// fast-forwards from the upstream; true if that moved the repo. Activating afterwards is up to the caller
pub fn pull(runner: &dyn Runner, repo: &Repo) -> Result<bool, GenerationsError> {
    if git(runner, repo, &["remote"])?.trim().is_empty() {
        return Err(GenerationsError::NoRemote(repo.root.display().to_string()));
    }
    let head = || git(runner, repo, &["rev-parse", "HEAD"]).unwrap_or_default();
    let before = head();
    runner.interactive("git", &in_repo(repo, &["pull", "--ff-only"])).map_err(|_| GenerationsError::PullFailed(repo.root.display().to_string()))?;
    Ok(head() != before)
}

// every backend's installed packages as exact specs, backends with none left out
fn installed(env: &Env, runner: &dyn Runner) -> Result<BTreeMap<String, Vec<String>>, BackendError> {
    let all = NAMES
        .iter()
        .filter_map(|name| backend::for_name(name, runner, env))
        .map(|backend| Ok((backend.name().to_string(), backend.list()?.iter().map(|pkg| backend.pin(pkg)).collect::<Vec<_>>())))
        .collect::<Result<Vec<_>, BackendError>>()?;
    Ok(all.into_iter().filter(|(_, specs)| !specs.is_empty()).collect())
}

// commits the repo and appends a generation, asking for the message (the generated one if there's no answer)
pub fn record(env: &Env, runner: &dyn Runner, repo: &Repo, summary: &[String], ask: &dyn Ask) -> Result<Generation, GenerationsError> {
    let number = load(env)?.last().map_or(1, |last| last.number + 1);
    let summary = if summary.is_empty() { "activate".to_string() } else { summary.join("; ") };
    let generated = format!("generation {number}: {summary}");
    let answer = ask.ask(&format!("commit message [{generated}]: ")).map(|answer| answer.trim().to_string());
    let message = answer.filter(|answer| !answer.is_empty()).unwrap_or(generated);

    let commit = commit_all(runner, repo, &message)?;
    let time = runner.run("date", &["+%Y-%m-%d %H:%M".into()])?.trim().to_string();
    let generation = Generation { number, time, commit, message, packages: installed(env, runner)? };

    // appended as one line, so the log only ever grows
    let path = log_file(env);
    let io = |source| GenerationsError::Io { path: path.clone(), source };
    fs::create_dir_all(&env.state_dir).map_err(io)?;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(&path).map_err(io)?;
    writeln!(file, "{}", serde_json::to_string(&generation).unwrap()).map_err(io)?;
    Ok(generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activate::{Options, activate};
    use crate::build::Report;
    use crate::init::Scope;
    use crate::testing::{Fixture, fixture};

    struct Answer(Option<&'static str>);

    impl Ask for Answer {
        fn ask(&self, _: &str) -> Option<String> {
            self.0.map(String::from)
        }
    }

    // a fixture whose repo is a real git repo
    fn with_git(modules: &[&str]) -> Fixture {
        let fixture = fixture(modules);
        git(&fixture.runner, &fixture.repo, &["init", "-q"]).unwrap();
        fixture
    }

    fn head(fixture: &Fixture) -> String {
        git(&fixture.runner, &fixture.repo, &["rev-parse", "HEAD"]).unwrap().trim().to_string()
    }

    fn activation(fixture: &Fixture) -> Activation {
        activate(&fixture.env, &fixture.runner, &fixture.repo, &Answer(None), Options::default()).unwrap()
    }

    #[test]
    fn record_commits_the_whole_repo_and_logs_the_generation() {
        let fixture = with_git(&["foot"]);
        let first = activation(&fixture);
        let generation = record(&fixture.env, &fixture.runner, &fixture.repo, &summary(&fixture.repo, &first), &Answer(None)).unwrap();

        assert_eq!((generation.number, generation.message.as_str()), (1, "generation 1: foot; 1 link"));
        assert_eq!(generation.commit, head(&fixture));
        assert_eq!(generation.packages["xbps"], ["bash-5.2_1"]);
        assert!(git(&fixture.runner, &fixture.repo, &["status", "--porcelain"]).unwrap().is_empty());
        assert_eq!(load(&fixture.env).unwrap(), [generation]);
    }

    #[test]
    fn a_given_message_wins_and_a_clean_repo_keeps_its_commit() {
        let fixture = with_git(&["foot"]);
        record(&fixture.env, &fixture.runner, &fixture.repo, &[], &Answer(None)).unwrap();
        let before = head(&fixture);

        let second = record(&fixture.env, &fixture.runner, &fixture.repo, &["x".into()], &Answer(Some("tweak the bar\n"))).unwrap();
        assert_eq!((second.number, second.message.as_str(), second.commit.as_str()), (2, "tweak the bar", before.as_str()));
    }

    #[test]
    fn summary_names_programs_and_actions() {
        let fixture = fixture(&[]);
        let activation = Activation {
            build: Report { written: vec![fixture.repo.out_dir().join("niri/config.kdl"), fixture.repo.out_dir().join("sv/rclone/run")], ..Report::default() },
            steps: vec![
                Step::Link { destination: "/a".into(), backup: false },
                Step::Relink { destination: "/b".into() },
                Step::Install { backend: "xbps".into(), package: "foot".into() },
                Step::Enable { scope: Scope::User, name: "rclone".into() },
            ],
            ..Activation::default()
        };
        assert_eq!(summary(&fixture.repo, &activation), ["niri, rclone", "2 links", "install foot", "enable rclone"]);
    }

    #[test]
    fn drift_alone_is_no_change() {
        let activation = Activation { steps: vec![Step::Edited { destination: "/a".into() }], ..Activation::default() };
        assert!(!changed(&activation));
    }

    #[test]
    fn commit_and_push_need_something_to_do() {
        let fixture = with_git(&[]);
        record(&fixture.env, &fixture.runner, &fixture.repo, &[], &Answer(None)).unwrap();
        assert!(!commit(&fixture.runner, &fixture.repo, Some("nothing")).unwrap());
        assert!(matches!(push(&fixture.runner, &fixture.repo), Err(GenerationsError::NoRemote(_))));
    }
}
