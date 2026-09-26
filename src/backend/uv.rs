use super::{Backend, BackendError, Pkg, spec_base, spec_program};
use crate::env::Env;
use crate::runner::{RunError, Runner};
use std::path::PathBuf;

// python programs from pypi or git, installed with uv tool into their own environments
pub struct Uv<'a> {
    runner: &'a dyn Runner,
    home: PathBuf,
}

impl<'a> Uv<'a> {
    pub fn new(runner: &'a dyn Runner, env: &Env) -> Self {
        Uv { runner, home: env.home.clone() }
    }

    fn uv(&self, args: &[&str]) -> Result<String, RunError> {
        self.runner.run("uv", &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
    }
}

// what uv tool install takes for a spec: "ruff@0.6" pins with ==, a bare name asks for the latest (clearing any pin), urls pass as they are
fn requirement(spec: &str) -> String {
    match (spec_base(spec), spec.contains("://")) {
        (_, true) => spec.to_string(),
        (base, false) if base == spec => format!("{spec}@latest"),
        (base, false) => format!("{base}=={}", &spec[base.len() + 1..]),
    }
}

// uv tool list: "ruff v0.6.9" per tool, its executables below as "- ruff"
fn parse_list(output: &str) -> Vec<Pkg> {
    output.lines().fold(Vec::new(), |mut pkgs: Vec<Pkg>, line| {
        match (line.strip_prefix("- "), pkgs.last_mut()) {
            (Some(program), Some(pkg)) => pkg.programs.get_or_insert_with(Vec::new).push(program.trim().to_string()),
            (None, _) => {
                let mut words = line.split_whitespace();
                if let (Some(name), Some(version)) = (words.next(), words.next().and_then(|word| word.strip_prefix('v'))) {
                    pkgs.push(Pkg { name: name.into(), source: name.into(), version: version.into(), manual: true, ..Pkg::default() });
                }
            }
            _ => {}
        }
        pkgs
    })
}

impl Backend for Uv<'_> {
    fn name(&self) -> &str {
        "uv"
    }

    // installed tools; none when uv isn't installed
    fn list(&self) -> Result<Vec<Pkg>, BackendError> {
        match self.uv(&["tool", "list"]) {
            Ok(output) => Ok(parse_list(&output)),
            Err(RunError::Spawn { .. }) => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }

    // pypi has no search api, so only an exact name is found
    fn search(&self, term: &str) -> Result<Vec<Pkg>, BackendError> {
        Ok(self.info(term)?.into_iter().collect())
    }

    // a project on pypi, from its json api; urls aren't checked until they're installed
    fn info(&self, spec: &str) -> Result<Option<Pkg>, BackendError> {
        if spec.contains("://") {
            return Ok(Some(Pkg { name: spec_program(spec), source: spec.into(), ..Pkg::default() }));
        }
        let url = format!("https://pypi.org/pypi/{}/json", spec_base(spec));
        let Ok(body) = self.runner.run("curl", &["-sf".into(), url]) else { return Ok(None) };
        let project: serde_json::Value = serde_json::from_str(&body).map_err(|_| BackendError::Parse { backend: "uv".into(), line: body.clone() })?;
        let field = |key: &str| project["info"][key].as_str().unwrap_or_default().to_string();
        let name = field("name");
        if name.is_empty() {
            return Ok(None);
        }
        Ok(Some(Pkg { source: name.clone(), name, version: field("version"), description: field("summary"), homepage: field("home_page"), ..Pkg::default() }))
    }

    fn install(&self, specs: &[String]) -> Result<(), BackendError> {
        specs.iter().try_for_each(|spec| Ok(self.runner.interactive("uv", &["tool".into(), "install".into(), requirement(spec)])?))
    }

    fn remove(&self, specs: &[String]) -> Result<(), BackendError> {
        let names = specs.iter().map(|spec| spec_base(spec).to_string());
        Ok(self.runner.interactive("uv", &["tool".to_string(), "uninstall".into()].into_iter().chain(names).collect::<Vec<_>>())?)
    }

    fn tool(&self) -> Option<(&'static str, &'static str)> {
        Some(("uv", "uv"))
    }

    fn pin(&self, pkg: &Pkg) -> String {
        format!("{}@{}", pkg.source, pkg.version)
    }

    // uv's bin dir, ~/.local/bin unless configured otherwise
    fn bin_dir(&self) -> Option<PathBuf> {
        let configured = self.uv(&["tool", "dir", "--bin"]).ok().map(|dir| PathBuf::from(dir.trim())).filter(|dir| !dir.as_os_str().is_empty());
        Some(configured.unwrap_or_else(|| self.home.join(".local/bin")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use std::path::Path;

    fn env() -> Env {
        Env::new(Path::new("/h"), Path::new("/"), Path::new("/s"))
    }

    #[test]
    fn list_reads_tools_and_their_programs() {
        let pkgs = parse_list("httpie v3.2.3 [required: ==3.2.3]\n- http\n- https\nruff v0.6.9\n- ruff\n");
        assert_eq!(pkgs.iter().map(|pkg| (pkg.name.as_str(), pkg.version.as_str())).collect::<Vec<_>>(), [("httpie", "3.2.3"), ("ruff", "0.6.9")]);
        assert_eq!(pkgs[0].programs.as_deref(), Some(&["http".to_string(), "https".to_string()][..]));
        assert!(parse_list("No tools installed\n").is_empty());
    }

    #[test]
    fn specs_become_uv_requirements() {
        assert_eq!(requirement("ruff"), "ruff@latest");
        assert_eq!(requirement("ruff@0.6.9"), "ruff==0.6.9");
        assert_eq!(requirement("git+https://github.com/user/tool"), "git+https://github.com/user/tool");
    }

    #[test]
    fn info_reads_pypi() {
        let runner = FakeRunner::new(|_, _| r#"{"info":{"name":"ruff","version":"0.6.9","summary":"a linter","home_page":""}}"#.into());
        let ruff = Uv::new(&runner, &env()).info("ruff").unwrap().unwrap();
        assert_eq!((ruff.version.as_str(), ruff.description.as_str()), ("0.6.9", "a linter"));
        assert_eq!(runner.calls.borrow()[0], "curl -sf https://pypi.org/pypi/ruff/json");
    }

    #[test]
    fn install_and_remove_run_uv_tool() {
        let runner = FakeRunner::new(|_, _| String::new());
        let uv = Uv::new(&runner, &env());
        uv.install(&["ruff".into(), "httpie@3.2.3".into()]).unwrap();
        uv.remove(&["ruff".into()]).unwrap();
        assert_eq!(runner.calls.borrow()[..], ["uv tool install ruff@latest", "uv tool install httpie==3.2.3", "uv tool uninstall ruff"]);
    }
}
