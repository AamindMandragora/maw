use std::collections::HashMap;
use std::io::Write;
use std::sync::Mutex;
use std::path::Path;
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{program}")]
    Spawn { program: String, source: std::io::Error },
    #[error("{program} failed:\n{stderr}")]
    Failed { program: String, stderr: String },
}

// every external command goes through this, so tests can swap in a fake; shared across threads for parallel searches
pub trait Runner: Sync {
    fn run(&self, program: &str, args: &[String]) -> Result<String, RunError>;

    // runs a command on the user's terminal, like an editor
    fn interactive(&self, program: &str, args: &[String]) -> Result<(), RunError>;

    // runs a command on the user's terminal with text on its stdin, like a pager
    fn pipe(&self, program: &str, args: &[String], input: &str) -> Result<(), RunError>;
}

// runs a command as root on the terminal: through sudo on the real system, directly on a scratch root
pub fn as_root(runner: &dyn Runner, sysroot: &Path, program: &str, args: &[String]) -> Result<(), RunError> {
    if sysroot == Path::new("/") {
        let with_program: Vec<String> = std::iter::once(program.to_string()).chain(args.iter().cloned()).collect();
        runner.interactive("sudo", &with_program)
    } else {
        runner.interactive(program, args)
    }
}

pub struct SystemRunner;

// answers to read-only package queries, so one command asks each tool once however many steps want to know; anything
// that can change what's installed empties it
static QUERIES: Mutex<Option<Answers>> = Mutex::new(None);

// each query, as its program and args, with its stdout or its stderr when it failed
type Answers = HashMap<Vec<String>, Result<String, String>>;

// a query whose answer only changes when packages do: listings and lookups, never installs
fn is_query(program: &str, args: &[String]) -> bool {
    let first = args.first().map(String::as_str).unwrap_or_default();
    match program {
        "xbps-query" => true,
        "flatpak" => matches!(first, "list" | "info"),
        "cargo" => args == ["install", "--list"],
        "go" => matches!(first, "version" | "env"),
        "uv" => first == "tool" && matches!(args.get(1).map(String::as_str), Some("list" | "dir")),
        "npm" => first == "ls",
        _ => false,
    }
}

// the tools queries are asked of; running one of them any other way may change its answers
const QUERIED: [&str; 7] = ["xbps-query", "xbps-install", "flatpak", "cargo", "go", "uv", "npm"];

// empties the query answers, for a long-running process like the tui after something changed outside it
pub fn forget() {
    *QUERIES.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

fn spawn(program: &str, args: &[String]) -> Result<String, RunError> {
    let output = Command::new(program).args(args).output().map_err(|source| RunError::Spawn { program: program.into(), source })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(RunError::Failed { program: program.into(), stderr });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

impl Runner for SystemRunner {
    // runs the command and returns its stdout, or stderr as the error; queries are answered once per process
    fn run(&self, program: &str, args: &[String]) -> Result<String, RunError> {
        if !is_query(program, args) {
            if QUERIED.contains(&program) {
                forget();
            }
            return spawn(program, args);
        }
        let key: Vec<String> = std::iter::once(program.to_string()).chain(args.iter().cloned()).collect();
        let known = QUERIES.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref().and_then(|queries| queries.get(&key).cloned());
        if let Some(answer) = known {
            return answer.map_err(|stderr| RunError::Failed { program: program.into(), stderr });
        }

        // a tool that can't be started isn't remembered, so installing it is noticed
        let answer = spawn(program, args);
        let remembered = match &answer {
            Ok(stdout) => Some(Ok(stdout.clone())),
            Err(RunError::Failed { stderr, .. }) => Some(Err(stderr.clone())),
            Err(RunError::Spawn { .. }) => None,
        };
        if let Some(remembered) = remembered {
            QUERIES.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get_or_insert_with(HashMap::new).insert(key, remembered);
        }
        answer
    }

    // anything on the terminal, like an install through sudo, may change what's installed
    fn interactive(&self, program: &str, args: &[String]) -> Result<(), RunError> {
        forget();
        let status = Command::new(program).args(args).status().map_err(|source| RunError::Spawn {
            program: program.into(),
            source,
        })?;

        if !status.success() {
            return Err(RunError::Failed { program: program.into(), stderr: format!("exited with {status}") });
        }
        Ok(())
    }

    // a pager that quits early just closes its stdin, which isn't an error
    fn pipe(&self, program: &str, args: &[String], input: &str) -> Result<(), RunError> {
        let spawn = |source| RunError::Spawn { program: program.into(), source };
        let mut child = Command::new(program).args(args).stdin(std::process::Stdio::piped()).spawn().map_err(spawn)?;
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        child.wait().map_err(spawn)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_listings_and_lookups_are_queries() {
        let args = |text: &str| text.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert!(is_query("xbps-query", &args("-l")) && is_query("flatpak", &args("info --show-commit x")) && is_query("uv", &args("tool dir --bin")));
        assert!(is_query("cargo", &args("install --list")) && is_query("npm", &args("ls -g")));
        assert!(!is_query("flatpak", &args("install x")) && !is_query("cargo", &args("install bat")) && !is_query("uv", &args("tool install ruff")));
        assert!(!is_query("git", &args("status")) && !is_query("nix-instantiate", &args("--eval")));
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::MutexGuard;

    type Respond = Box<dyn Fn(&str, &[String]) -> Result<String, RunError> + Send + Sync>;

    // the recorded calls, readable like a RefCell
    #[derive(Default)]
    pub struct Calls(Mutex<Vec<String>>);

    impl Calls {
        pub fn borrow(&self) -> MutexGuard<'_, Vec<String>> {
            self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        pub fn borrow_mut(&self) -> MutexGuard<'_, Vec<String>> {
            self.borrow()
        }
    }

    // records every call as one "program arg arg" line and answers with a closure
    pub struct FakeRunner {
        pub calls: Calls,
        respond: Respond,
    }

    impl FakeRunner {
        pub fn new(respond: impl Fn(&str, &[String]) -> String + Send + Sync + 'static) -> Self {
            FakeRunner::fallible(move |program, args| Ok(respond(program, args)))
        }

        // a fake whose answers can be failures, like a query for a missing package
        pub fn fallible(respond: impl Fn(&str, &[String]) -> Result<String, RunError> + Send + Sync + 'static) -> Self {
            FakeRunner { calls: Calls::default(), respond: Box::new(respond) }
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<String, RunError> {
            self.calls.borrow_mut().push(format!("{program} {}", args.join(" ")));
            (self.respond)(program, args)
        }

        // recorded like run, with the answer ignored
        fn interactive(&self, program: &str, args: &[String]) -> Result<(), RunError> {
            self.run(program, args).map(|_| ())
        }

        fn pipe(&self, program: &str, args: &[String], _: &str) -> Result<(), RunError> {
            self.run(program, args).map(|_| ())
        }
    }
}
