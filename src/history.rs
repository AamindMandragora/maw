use crate::env::Env;
use crate::generations::{self, Generation, GenerationsError, git, in_repo};
use crate::repo::Repo;
use crate::runner::{RunError, Runner};

// the repo's history beyond one commit at a time: squashing a range of generations into one, pulling a history another
// machine rewrote without losing this one's unpushed commits, and pushing after every commit maw makes

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error(transparent)]
    Generations(#[from] GenerationsError),
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("no generation {0}; `maw generations` lists them")]
    NoGeneration(u32),
    #[error("squash needs an earlier and a later generation, like `maw squash 5 10`")]
    Order,
    #[error("generation {0}'s commit isn't in this branch's history")]
    Gone(u32),
    #[error("the history after generation {0} has merges; squash it with git instead")]
    Merges(u32),
    #[error("{0} has changes; commit them first with `maw commit`")]
    Dirty(String),
    #[error("{0} is behind its remote; `maw pull` first, so the squash keeps the other machines' commits")]
    Behind(String),
    #[error("{0} has no remote; add one with `git -C {0} remote add origin <url>`")]
    NoRemote(String),
}

// what a squash would do: the generations it merges, how many commits they are, and how many come after
#[derive(Debug, PartialEq)]
pub struct Squash {
    pub generations: Vec<u32>,
    pub commits: usize,
    pub after: usize,
    // the last generation before the range, which the squashed commit follows; none squashes from the first commit
    base: Option<String>,
    tip: String,
}

// a squash of generations from..=to, planned without changing anything
pub fn plan_squash(env: &Env, runner: &dyn Runner, repo: &Repo, from: u32, to: u32) -> Result<Squash, HistoryError> {
    if from >= to {
        return Err(HistoryError::Order);
    }
    let all = generations::load(env)?;
    let find = |number: u32| all.iter().find(|generation| generation.number == number).ok_or(HistoryError::NoGeneration(number));
    let (first, last) = (find(from)?, find(to)?);
    let base = all.iter().rfind(|generation| generation.number < first.number && !generation.commit.is_empty()).map(|generation| generation.commit.clone());

    // the range and everything after it must be this branch's own, unmerged history
    let reaches = |commit: &str| git(runner, repo, &["merge-base", "--is-ancestor", commit, "HEAD"]).is_ok();
    if last.commit.is_empty() || !reaches(&last.commit) {
        return Err(HistoryError::Gone(to));
    }
    if base.as_deref().is_some_and(|base| !reaches(base)) {
        return Err(HistoryError::Gone(from));
    }
    let since = |end: &str| base.as_ref().map_or(end.to_string(), |base| format!("{base}..{end}"));
    let count = |range: &str, merges: bool| -> Result<usize, HistoryError> {
        let args = ["rev-list", "--count"].into_iter().chain(merges.then_some("--merges")).chain([range]).collect::<Vec<_>>();
        Ok(git(runner, repo, &args)?.trim().parse().unwrap_or_default())
    };
    if count(&since("HEAD"), true)? > 0 {
        return Err(HistoryError::Merges(from));
    }

    let generations = all.iter().map(|generation| generation.number).filter(|number| (from..=to).contains(number)).collect();
    Ok(Squash { generations, commits: count(&since(&last.commit), false)?, after: count(&format!("{}..HEAD", last.commit), false)?, base, tip: last.commit.clone() })
}

// squashes the planned range into one commit with the last generation's files, replays the commits after it, and
// rewrites the generations log to match: the range becomes one generation, numbered as its last. The history before
// is kept on a backup branch, which is returned
pub fn squash(env: &Env, runner: &dyn Runner, repo: &Repo, plan: &Squash) -> Result<String, HistoryError> {
    if !git(runner, repo, &["status", "--porcelain"])?.trim().is_empty() {
        return Err(HistoryError::Dirty(repo.root.display().to_string()));
    }
    fetch_ahead(runner, repo)?;
    let head = git(runner, repo, &["rev-parse", "HEAD"])?.trim().to_string();
    let backup = format!("maw-backup/squash-{}-{}", plan.generations[0], plan.generations[plan.generations.len() - 1]);
    git(runner, repo, &["branch", "-f", &backup, &head])?;

    // one commit with the tip's files, after the base
    let all = generations::load(env)?;
    let (from, to) = (plan.generations[0], plan.generations[plan.generations.len() - 1]);
    let messages: String = all.iter().filter(|generation| plan.generations.contains(&generation.number)).map(|generation| format!("{}\n", generation.message)).collect();
    let tree = format!("{}^{{tree}}", plan.tip);
    let subject = format!("generations {from}-{to}");
    let parent = plan.base.iter().flat_map(|base| ["-p", base.as_str()]);
    let args: Vec<&str> = ["commit-tree", tree.as_str()].into_iter().chain(parent).chain(["-m", subject.as_str(), "-m", messages.trim_end()]).collect();
    let squashed = git(runner, repo, &args)?.trim().to_string();

    // the commits after the range, replayed on it; with the same files under them they apply as they are
    let later = |from: &str| -> Result<Vec<String>, HistoryError> { Ok(git(runner, repo, &["rev-list", "--reverse", &format!("{from}..HEAD")])?.lines().map(String::from).collect()) };
    let old_later = later(&plan.tip)?;
    match old_later.is_empty() {
        true => git(runner, repo, &["reset", "-q", "--hard", &squashed])?,
        false => git(runner, repo, &["rebase", "-q", "--onto", &squashed, &plan.tip])?,
    };
    let moved: Vec<(String, String)> = old_later.into_iter().zip(later(&squashed)?).collect();

    // the log: the range as one generation at the squashed commit, later ones at their replayed commits
    let last = all.iter().find(|generation| generation.number == to).cloned().ok_or(HistoryError::NoGeneration(to))?;
    let merged = Generation { commit: squashed, message: subject, ..last };
    let rewritten = all.into_iter().filter(|generation| !plan.generations.contains(&generation.number) || generation.number == to).map(|generation| match generation.number {
        number if number == to => merged.clone(),
        number if number > to => Generation { commit: moved.iter().find(|(old, _)| *old == generation.commit).map_or(generation.commit.clone(), |(_, new)| new.clone()), ..generation },
        _ => generation,
    });
    generations::save(env, &rewritten.collect::<Vec<_>>())?;
    Ok(backup)
}

// the first remote, if there is one
fn remote(runner: &dyn Runner, repo: &Repo) -> Result<Option<String>, HistoryError> {
    Ok(git(runner, repo, &["remote"])?.lines().next().map(String::from))
}

// fetches, then refuses if the remote has commits this branch doesn't: rewriting then would drop them
fn fetch_ahead(runner: &dyn Runner, repo: &Repo) -> Result<(), HistoryError> {
    let Some(remote) = remote(runner, repo)? else { return Ok(()) };
    git(runner, repo, &["fetch", "-q", &remote])?;
    match git(runner, repo, &["rev-parse", "-q", "--verify", "@{u}"]) {
        Ok(upstream) if git(runner, repo, &["merge-base", "--is-ancestor", upstream.trim(), "HEAD"]).is_err() => Err(HistoryError::Behind(repo.root.display().to_string())),
        _ => Ok(()),
    }
}

// pushes the branch to the first remote without ever stopping to ask for credentials, so it can follow any commit;
// after a squash, force replaces the remote's history (only if it's still what was fetched). False without a remote
pub fn push(runner: &dyn Runner, repo: &Repo, force: bool) -> Result<bool, HistoryError> {
    let Some(remote) = remote(runner, repo)? else { return Ok(false) };
    let push = ["push", "-q"].into_iter().chain(force.then_some("--force-with-lease")).chain(["-u", remote.as_str(), "HEAD"]).collect::<Vec<_>>();
    let args: Vec<String> = ["GIT_TERMINAL_PROMPT=0".to_string(), "git".into()].into_iter().chain(in_repo(repo, &push)).collect();
    runner.run("env", &args)?;
    Ok(true)
}

// what a pull did: whether it moved, whether the remote's history had been rewritten, how many of this machine's
// unpushed commits went on top, and the branch the old history and those commits were kept on
#[derive(Debug, Default, PartialEq)]
pub struct Pulled {
    pub moved: bool,
    pub rewritten: bool,
    pub replayed: usize,
    // unpushed commits that conflicted with the new history, left on the backup branch
    pub stranded: usize,
    pub backup: Option<String>,
}

// brings the branch to its remote's: fast-forwarded when it only moved ahead, reset when another machine rewrote it
// (a squash). Commits only this machine has go back on top, after a backup branch keeps them and the old history
pub fn pull(runner: &dyn Runner, repo: &Repo) -> Result<Pulled, HistoryError> {
    let Some(remote) = remote(runner, repo)? else { return Err(HistoryError::NoRemote(repo.root.display().to_string())) };

    // out/ is rebuilt by the activation after, so changes in it never block a pull (a repo with nothing in out/ yet
    // has nothing to discard, which git reports as an error)
    git(runner, repo, &["checkout", "-q", "--", "out"]).ok();
    if !git(runner, repo, &["status", "--porcelain"])?.trim().is_empty() {
        return Err(HistoryError::Dirty(repo.root.display().to_string()));
    }
    let upstream = || git(runner, repo, &["rev-parse", "-q", "--verify", "@{u}"]).ok().map(|commit| commit.trim().to_string());
    let Some(old) = upstream() else {
        // a branch that never tracked its remote just fast-forwards to it
        let before = git(runner, repo, &["rev-parse", "HEAD"]).unwrap_or_default();
        runner.interactive("git", &in_repo(repo, &["pull", "--ff-only", &remote]))?;
        return Ok(Pulled { moved: git(runner, repo, &["rev-parse", "HEAD"]).unwrap_or_default() != before, ..Pulled::default() });
    };
    git(runner, repo, &["fetch", "-q", &remote])?;
    let new = upstream().unwrap_or_else(|| old.clone());
    if new == old {
        return Ok(Pulled::default());
    }

    // what only this machine has, and whether the remote's history still contains the old one
    let local: Vec<String> = git(runner, repo, &["rev-list", &format!("{old}..HEAD")])?.lines().map(String::from).collect();
    let rewritten = git(runner, repo, &["merge-base", "--is-ancestor", &old, &new]).is_err();
    if local.is_empty() && !rewritten {
        git(runner, repo, &["merge", "-q", "--ff-only", &new])?;
        return Ok(Pulled { moved: true, ..Pulled::default() });
    }
    let head = git(runner, repo, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
    let backup = format!("maw-backup/pull-{head}");
    git(runner, repo, &["branch", "-f", &backup, "HEAD"])?;

    // this machine's commits replayed on the new history; ones that conflict stay on the backup branch
    let replayed = local.is_empty() || git(runner, repo, &["rebase", "-q", "--onto", &new, &old]).is_ok();
    if !replayed {
        git(runner, repo, &["rebase", "--abort"]).ok();
        git(runner, repo, &["reset", "-q", "--hard", &new])?;
    } else if local.is_empty() {
        git(runner, repo, &["reset", "-q", "--hard", &new])?;
    }
    let (replayed, stranded) = if replayed { (local.len(), 0) } else { (0, local.len()) };
    Ok(Pulled { moved: true, rewritten, replayed, stranded, backup: Some(backup) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Fixture, fixture, write};

    // a repo whose generations 1 to 4 each changed one file, with a bare remote it has pushed to
    fn history() -> (Fixture, tempfile::TempDir) {
        let fixture = fixture(&[]);
        let remote = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| git(&fixture.runner, &fixture.repo, args).unwrap();
        fixture.runner.run("git", &["init".into(), "-q".into(), "--bare".into(), "-b".into(), "main".into(), remote.path().display().to_string()]).unwrap();
        run(&["init", "-q", "-b", "main"]);
        run(&["remote", "add", "origin", &remote.path().display().to_string()]);
        let generations: Vec<Generation> = (1..=4)
            .map(|number| {
                write(&fixture.repo.root.join("notes"), &format!("{number}\n"));
                run(&["add", "-A"]);
                run(&["commit", "-q", "-m", &format!("generation {number}")]);
                let commit = run(&["rev-parse", "HEAD"]).trim().to_string();
                Generation { number, time: "t".into(), commit, message: format!("generation {number}"), packages: Default::default() }
            })
            .collect();
        generations::save(&fixture.env, &generations).unwrap();
        push(&fixture.runner, &fixture.repo, false).unwrap();
        (fixture, remote)
    }

    fn log(fixture: &Fixture) -> Vec<String> {
        git(&fixture.runner, &fixture.repo, &["log", "--format=%s"]).unwrap().lines().map(String::from).collect()
    }

    #[test]
    fn a_closed_range_becomes_one_commit_and_later_ones_follow() {
        let (fixture, _remote) = history();
        let plan = plan_squash(&fixture.env, &fixture.runner, &fixture.repo, 2, 3).unwrap();
        assert_eq!((plan.generations.clone(), plan.commits, plan.after), (vec![2, 3], 2, 1));

        let backup = squash(&fixture.env, &fixture.runner, &fixture.repo, &plan).unwrap();
        assert_eq!(log(&fixture), ["generation 4", "generations 2-3", "generation 1"]);
        assert_eq!(std::fs::read_to_string(fixture.repo.root.join("notes")).unwrap(), "4\n");

        // the log names the merged generation and moves the later one to its replayed commit
        let after = generations::load(&fixture.env).unwrap();
        assert_eq!(after.iter().map(|generation| (generation.number, generation.message.as_str())).collect::<Vec<_>>(), [(1, "generation 1"), (3, "generations 2-3"), (4, "generation 4")]);
        assert_eq!(after[2].commit, git(&fixture.runner, &fixture.repo, &["rev-parse", "HEAD"]).unwrap().trim());
        assert!(git(&fixture.runner, &fixture.repo, &["rev-parse", "--verify", &backup]).is_ok());
    }

    #[test]
    fn squashing_from_the_first_generation_starts_the_history_there() {
        let (fixture, _remote) = history();
        let plan = plan_squash(&fixture.env, &fixture.runner, &fixture.repo, 1, 4).unwrap();
        squash(&fixture.env, &fixture.runner, &fixture.repo, &plan).unwrap();
        assert_eq!(log(&fixture), ["generations 1-4"]);
        assert!(matches!(plan_squash(&fixture.env, &fixture.runner, &fixture.repo, 3, 2), Err(HistoryError::Order)));
    }

    #[test]
    fn a_squash_refuses_changes_and_a_remote_that_moved_on() {
        let (fixture, remote) = history();
        write(&fixture.repo.root.join("notes"), "edited\n");
        let plan = plan_squash(&fixture.env, &fixture.runner, &fixture.repo, 1, 2).unwrap();
        assert!(matches!(squash(&fixture.env, &fixture.runner, &fixture.repo, &plan), Err(HistoryError::Dirty(_))));
        git(&fixture.runner, &fixture.repo, &["checkout", "-q", "--", "notes"]).unwrap();

        // another machine pushed a commit this one hasn't pulled
        let other = tempfile::tempdir().unwrap();
        let other_repo = other.path().join("dots");
        fixture.runner.run("git", &["clone".into(), "-q".into(), remote.path().display().to_string(), other_repo.display().to_string()]).unwrap();
        write(&other_repo.join("other"), "x\n");
        let in_other = |args: &[&str]| fixture.runner.run("git", &["-C".to_string(), other_repo.display().to_string()].into_iter().chain(args.iter().map(|arg| arg.to_string())).collect::<Vec<_>>()).unwrap();
        in_other(&["add", "-A"]);
        in_other(&["commit", "-q", "-m", "elsewhere"]);
        in_other(&["push", "-q"]);
        assert!(matches!(squash(&fixture.env, &fixture.runner, &fixture.repo, &plan), Err(HistoryError::Behind(_))));
    }

    #[test]
    fn pulling_a_squashed_history_puts_unpushed_commits_on_top() {
        let (fixture, remote) = history();

        // another machine with the same history and one commit of its own, not pushed
        let other = fixture_clone(&remote);
        write(&other.repo.root.join("mine"), "local\n");
        git(&other.runner, &other.repo, &["add", "-A"]).unwrap();
        git(&other.runner, &other.repo, &["commit", "-q", "-m", "unpushed"]).unwrap();

        // this machine squashes everything and force-pushes
        let plan = plan_squash(&fixture.env, &fixture.runner, &fixture.repo, 1, 4).unwrap();
        squash(&fixture.env, &fixture.runner, &fixture.repo, &plan).unwrap();
        push(&fixture.runner, &fixture.repo, true).unwrap();

        let pulled = pull(&other.runner, &other.repo).unwrap();
        assert_eq!((pulled.moved, pulled.rewritten, pulled.replayed, pulled.stranded), (true, true, 1, 0));
        assert_eq!(log(&other), ["unpushed", "generations 1-4"]);
        assert!(pulled.backup.is_some_and(|backup| git(&other.runner, &other.repo, &["rev-parse", "--verify", &backup]).is_ok()));
    }

    #[test]
    fn unpushed_commits_that_conflict_stay_on_the_backup_branch() {
        let (fixture, remote) = history();
        let other = fixture_clone(&remote);
        write(&other.repo.root.join("notes"), "theirs\n");
        git(&other.runner, &other.repo, &["commit", "-q", "-am", "unpushed"]).unwrap();

        // the squash, then a change to the same file, both pushed
        let plan = plan_squash(&fixture.env, &fixture.runner, &fixture.repo, 1, 4).unwrap();
        squash(&fixture.env, &fixture.runner, &fixture.repo, &plan).unwrap();
        write(&fixture.repo.root.join("notes"), "ours\n");
        git(&fixture.runner, &fixture.repo, &["commit", "-q", "-am", "generation 5"]).unwrap();
        push(&fixture.runner, &fixture.repo, true).unwrap();

        let pulled = pull(&other.runner, &other.repo).unwrap();
        assert_eq!((pulled.replayed, pulled.stranded), (0, 1));
        assert_eq!(log(&other), ["generation 5", "generations 1-4"]);
        let backup = pulled.backup.unwrap();
        assert_eq!(git(&other.runner, &other.repo, &["log", "-1", "--format=%s", &backup]).unwrap().trim(), "unpushed");
    }

    #[test]
    fn a_plain_pull_fast_forwards() {
        let (fixture, remote) = history();
        let other = fixture_clone(&remote);
        write(&fixture.repo.root.join("notes"), "5\n");
        git(&fixture.runner, &fixture.repo, &["commit", "-q", "-am", "generation 5"]).unwrap();
        push(&fixture.runner, &fixture.repo, false).unwrap();
        assert_eq!(pull(&other.runner, &other.repo).unwrap(), Pulled { moved: true, ..Pulled::default() });
        assert_eq!(pull(&other.runner, &other.repo).unwrap(), Pulled::default());
    }

    // a second machine's repo, cloned from the remote
    fn fixture_clone(remote: &tempfile::TempDir) -> Fixture {
        let other = crate::testing::fixture(&[]);
        std::fs::remove_dir_all(&other.repo.root).unwrap();
        other.runner.run("git", &["clone".into(), "-q".into(), remote.path().display().to_string(), other.repo.root.display().to_string()]).unwrap();
        other
    }
}
