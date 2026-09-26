use maw::runner::{RunError, Runner, SystemRunner};

// the real runner for nix, git, and files, but nothing that reaches the live session: reloads (sh -c), dconf, sv, and
// sudo would touch the machine running the tests, so they're refused and a test that needs one fails loudly
pub struct Contained;

const LIVE: [&str; 4] = ["sh", "dconf", "sv", "sudo"];

fn refuse(program: &str) -> Result<(), RunError> {
    match LIVE.contains(&program) {
        true => Err(RunError::Failed { program: program.into(), stderr: format!("{program} would reach the live session") }),
        false => Ok(()),
    }
}

impl Runner for Contained {
    fn run(&self, program: &str, args: &[String]) -> Result<String, RunError> {
        refuse(program)?;
        SystemRunner.run(program, args)
    }

    fn interactive(&self, program: &str, args: &[String]) -> Result<(), RunError> {
        refuse(program)?;
        SystemRunner.interactive(program, args)
    }

    fn pipe(&self, program: &str, args: &[String], input: &str) -> Result<(), RunError> {
        refuse(program)?;
        SystemRunner.pipe(program, args, input)
    }
}
