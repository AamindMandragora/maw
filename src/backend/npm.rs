use super::{Backend, BackendError, Pkg, spec_base};
use crate::env::Env;
use crate::runner::{RunError, Runner};
use serde_json::Value;
use std::path::PathBuf;

// javascript programs from the npm registry, installed globally under ~/.local so their commands land in ~/.local/bin
pub struct Npm<'a> {
    runner: &'a dyn Runner,
    prefix: PathBuf,
}

impl<'a> Npm<'a> {
    pub fn new(runner: &'a dyn Runner, env: &Env) -> Self {
        Npm { runner, prefix: env.home.join(".local") }
    }

    // npm with the global prefix maw uses appended
    fn npm(&self, args: &[&str]) -> Result<String, RunError> {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).chain(["--prefix".into(), self.prefix.display().to_string()]).collect();
        self.runner.run("npm", &args)
    }

    fn parse_error(&self, body: &str) -> BackendError {
        BackendError::Parse { backend: "npm".into(), line: body.into() }
    }
}

// a package's commands from its bin field: a map of names, or one path named after the package
fn programs(name: &str, bin: &Value) -> Vec<String> {
    match bin {
        Value::Object(commands) => commands.keys().cloned().collect(),
        Value::String(_) => vec![name.rsplit('/').next().unwrap_or(name).to_string()],
        _ => Vec::new(),
    }
}

impl Backend for Npm<'_> {
    fn name(&self) -> &str {
        "npm"
    }

    // packages installed under the prefix; none before the first install or without npm
    fn list(&self) -> Result<Vec<Pkg>, BackendError> {
        if !self.prefix.join("lib/node_modules").is_dir() {
            return Ok(Vec::new());
        }
        let body = match self.npm(&["ls", "-g", "--depth=0", "--json"]) {
            Err(RunError::Spawn { .. }) => return Ok(Vec::new()),
            body => body?,
        };
        let tree: Value = serde_json::from_str(&body).map_err(|_| self.parse_error(&body))?;
        let dependencies = tree["dependencies"].as_object().cloned().unwrap_or_default();
        let pkgs = dependencies.into_iter().map(|(name, dependency)| {
            let version = dependency["version"].as_str().unwrap_or_default().to_string();
            Pkg { source: name.clone(), name, version, manual: true, ..Pkg::default() }
        });
        Ok(pkgs.collect())
    }

    fn search(&self, term: &str) -> Result<Vec<Pkg>, BackendError> {
        let body = self.npm(&["search", "--json", term])?;
        let results: Vec<Value> = serde_json::from_str(&body).map_err(|_| self.parse_error(&body))?;
        let field = |result: &Value, key: &str| result[key].as_str().unwrap_or_default().to_string();
        Ok(results.iter().map(|result| Pkg { name: field(result, "name"), source: field(result, "name"), version: field(result, "version"), description: field(result, "description"), ..Pkg::default() }).collect())
    }

    // a package from the registry with the commands it installs; one without any is a library
    fn info(&self, spec: &str) -> Result<Option<Pkg>, BackendError> {
        let body = match self.npm(&["view", spec, "--json"]) {
            Ok(body) => body,
            Err(RunError::Failed { .. }) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let package: Value = serde_json::from_str(&body).map_err(|_| self.parse_error(&body))?;
        let field = |key: &str| package[key].as_str().unwrap_or_default().to_string();
        let name = spec_base(spec).to_string();
        let programs = Some(programs(&name, &package["bin"]));
        Ok(Some(Pkg { source: name.clone(), name, version: field("version"), description: field("description"), homepage: field("homepage"), programs, ..Pkg::default() }))
    }

    // npm installs the latest for a bare name, so this also upgrades
    fn install(&self, specs: &[String]) -> Result<(), BackendError> {
        let args: Vec<String> = ["install", "-g"].into_iter().map(String::from).chain(specs.iter().cloned()).chain(["--prefix".into(), self.prefix.display().to_string()]).collect();
        Ok(self.runner.interactive("npm", &args)?)
    }

    fn remove(&self, specs: &[String]) -> Result<(), BackendError> {
        let names = specs.iter().map(|spec| spec_base(spec).to_string());
        let args: Vec<String> = ["uninstall", "-g"].into_iter().map(String::from).chain(names).chain(["--prefix".into(), self.prefix.display().to_string()]).collect();
        Ok(self.runner.interactive("npm", &args)?)
    }

    fn tool(&self) -> Option<(&'static str, &'static str)> {
        Some(("npm", "nodejs"))
    }

    fn pin(&self, pkg: &Pkg) -> String {
        format!("{}@{}", pkg.source, pkg.version)
    }

    fn bin_dir(&self) -> Option<PathBuf> {
        Some(self.prefix.join("bin"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use std::path::Path;

    const VIEW: &str = r#"{"version":"1.6.0","bin":{"cowsay":"cli.js","cowthink":"cli.js"},"description":"a talking cow","homepage":"https://x"}"#;

    fn fake(_: &str, args: &[String]) -> String {
        match args[0].as_str() {
            "ls" => r#"{"name":"lib","dependencies":{"cowsay":{"version":"1.5.0"},"@scope/tool":{"version":"2.0.0"}}}"#.into(),
            "view" if args[1] == "lodash" => r#"{"version":"4.18.1"}"#.into(),
            "view" => VIEW.into(),
            _ => String::new(),
        }
    }

    // a home with a real prefix, so list has somewhere to look
    fn setup() -> (tempfile::TempDir, FakeRunner, Env) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".local/lib/node_modules")).unwrap();
        let env = Env::new(dir.path(), Path::new("/"), Path::new("/s"));
        (dir, FakeRunner::new(fake), env)
    }

    #[test]
    fn list_reads_the_global_tree() {
        let (_dir, runner, env) = setup();
        let pkgs = Npm::new(&runner, &env).list().unwrap();
        assert_eq!(pkgs.iter().map(|pkg| (pkg.source.as_str(), pkg.version.as_str())).collect::<Vec<_>>(), [("@scope/tool", "2.0.0"), ("cowsay", "1.5.0")]);
        assert_eq!(Npm::new(&runner, &env).pin(&pkgs[0]), "@scope/tool@2.0.0");
    }

    #[test]
    fn nothing_is_installed_before_the_prefix_exists() {
        let runner = FakeRunner::new(fake);
        let env = Env::new(Path::new("/nonexistent"), Path::new("/"), Path::new("/s"));
        assert!(Npm::new(&runner, &env).list().unwrap().is_empty());
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn info_knows_programs_from_libraries() {
        let (_dir, runner, env) = setup();
        let npm = Npm::new(&runner, &env);
        assert_eq!(npm.info("cowsay").unwrap().unwrap().programs, Some(vec!["cowsay".to_string(), "cowthink".to_string()]));
        assert_eq!(npm.info("lodash").unwrap().unwrap().programs, Some(Vec::new()));
        assert_eq!(programs("@scope/tool", &Value::String("cli.js".into())), ["tool"]);
    }

    #[test]
    fn install_goes_under_the_local_prefix() {
        let (dir, runner, env) = setup();
        Npm::new(&runner, &env).install(&["cowsay@1.5.0".into()]).unwrap();
        assert_eq!(runner.calls.borrow()[0], format!("npm install -g cowsay@1.5.0 --prefix {}", dir.path().join(".local").display()));
    }
}
