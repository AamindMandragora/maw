use std::io::Write;
use std::path::Path;
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{program}")]
    Spawn { program: String, source: std::io::Error },
    #[error("{program} failed:\n{stderr}")]
    Failed { program: String, stderr: String },
}

// every external command goes through this, so tests can swap in a fake
pub trait Runner {
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

impl Runner for SystemRunner {
    // runs the command and returns its stdout, or stderr as the error
    fn run(&self, program: &str, args: &[String]) -> Result<String, RunError> {
        let output = Command::new(program).args(args).output().map_err(|source| RunError::Spawn {
            program: program.into(),
            source,
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(RunError::Failed { program: program.into(), stderr });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn interactive(&self, program: &str, args: &[String]) -> Result<(), RunError> {
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
pub mod fake {
    use super::*;
    use std::cell::RefCell;

    type Respond = Box<dyn Fn(&str, &[String]) -> Result<String, RunError>>;

    // records every call as one "program arg arg" line and answers with a closure
    pub struct FakeRunner {
        pub calls: RefCell<Vec<String>>,
        respond: Respond,
    }

    impl FakeRunner {
        pub fn new(respond: impl Fn(&str, &[String]) -> String + 'static) -> Self {
            FakeRunner::fallible(move |program, args| Ok(respond(program, args)))
        }

        // a fake whose answers can be failures, like a query for a missing package
        pub fn fallible(respond: impl Fn(&str, &[String]) -> Result<String, RunError> + 'static) -> Self {
            FakeRunner { calls: RefCell::new(Vec::new()), respond: Box::new(respond) }
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
