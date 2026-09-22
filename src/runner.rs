use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{program}: {source}")]
    Spawn { program: String, source: std::io::Error },
    #[error("{program} failed:\n{stderr}")]
    Failed { program: String, stderr: String },
}

// every external command goes through this, so tests can swap in a fake
pub trait Runner {
    fn run(&self, program: &str, args: &[String]) -> Result<String, RunError>;

    // runs a command on the user's terminal, like an editor
    fn interactive(&self, program: &str, args: &[String]) -> Result<(), RunError>;
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
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::cell::RefCell;

    type Respond = Box<dyn Fn(&str, &[String]) -> String>;

    // records every call as one "program arg arg" line and answers with a closure
    pub struct FakeRunner {
        pub calls: RefCell<Vec<String>>,
        respond: Respond,
    }

    impl FakeRunner {
        pub fn new(respond: impl Fn(&str, &[String]) -> String + 'static) -> Self {
            FakeRunner { calls: RefCell::new(Vec::new()), respond: Box::new(respond) }
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<String, RunError> {
            self.calls.borrow_mut().push(format!("{program} {}", args.join(" ")));
            Ok((self.respond)(program, args))
        }

        // recorded like run, with the answer ignored
        fn interactive(&self, program: &str, args: &[String]) -> Result<(), RunError> {
            self.run(program, args).map(|_| ())
        }
    }
}
