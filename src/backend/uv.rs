use super::{Backend, BackendError, Pkg, spec_base, spec_program};
use crate::env::Env;
use crate::runner::{RunError, Runner};
use std::path::{Path, PathBuf};

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

// a tool as it was installed: one from git gets its url as its source, and the ref and commit from its receipt
fn with_receipt(tools: &Path, pkg: Pkg) -> Pkg {
    let receipt = std::fs::read_to_string(tools.join(&pkg.name).join("uv-receipt.toml")).unwrap_or_default();
    match receipt_git(&receipt, &pkg.name) {
        Some(git) => git_source(&git, pkg),
        None => pkg,
    }
}

// the git url in a uv-receipt.toml requirement for a tool: `{ name = "tool", git = "https://host/tool?rev=v1#<commit>" }`
fn receipt_git(receipt: &str, name: &str) -> Option<String> {
    let entries = receipt.split('{').filter(|entry| entry.trim_start().starts_with(&format!("name = \"{name}\"")));
    entries.into_iter().find_map(|entry| Some(entry.split('}').next()?.split_once("git = \"")?.1.split('"').next()?.to_string()))
}

// a receipt's git url as the spec that installs it, "git+<url>", with its ref (rev, tag, or branch) and commit kept apart
fn git_source(git: &str, pkg: Pkg) -> Pkg {
    let (rest, commit) = git.split_once('#').unwrap_or((git, ""));
    let (url, query) = rest.split_once('?').unwrap_or((rest, ""));
    let reference = query.split('&').find_map(|pair| ["rev=", "tag=", "branch="].iter().find_map(|key| pair.strip_prefix(key))).unwrap_or_default();
    let source = format!("git+{}", url.trim_start_matches("git+"));
    Pkg { source, reference: reference.into(), build: commit.into(), ..pkg }
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

    // installed tools, with git ones by url; none when uv isn't installed
    fn list(&self) -> Result<Vec<Pkg>, BackendError> {
        let output = match self.uv(&["tool", "list"]) {
            Err(RunError::Spawn { .. }) => return Ok(Vec::new()),
            output => output?,
        };
        let tools = PathBuf::from(self.uv(&["tool", "dir"]).unwrap_or_default().trim());
        Ok(parse_list(&output).into_iter().map(|pkg| with_receipt(&tools, pkg)).collect())
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

    // uninstalls by tool name, found from the spec's source for git tools
    fn remove(&self, specs: &[String]) -> Result<(), BackendError> {
        let installed = self.list()?;
        let name = |spec: &String| installed.iter().find(|pkg| pkg.source == spec_base(spec)).map_or(spec_base(spec).to_string(), |pkg| pkg.name.clone());
        let names = specs.iter().map(name);
        Ok(self.runner.interactive("uv", &["tool".to_string(), "uninstall".into()].into_iter().chain(names).collect::<Vec<_>>())?)
    }

    fn tool(&self) -> Option<(&'static str, &'static str)> {
        Some(("uv", "uv"))
    }

    // a pypi tool at its version; a git one at its commit, else its ref, else bare
    fn pin(&self, pkg: &Pkg) -> String {
        match [&pkg.build, &pkg.reference].into_iter().find(|at| !at.is_empty()) {
            _ if !pkg.source.starts_with("git+") => format!("{}@{}", pkg.source, pkg.version),
            Some(at) => format!("{}@{at}", pkg.source),
            None => pkg.source.clone(),
        }
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

    const RECEIPT: &str = "[tool]\nrequirements = [{ name = \"croft\", git = \"https://github.com/user/croft?tag=v1.0#4c638f60aaaabbbbccccddddeeeeffff00001111\" }]\nentrypoints = [\n    { name = \"croft\", install-path = \"/h/.local/bin/croft\" },\n]\n";

    // a tool dir where croft came from git and ruff from pypi
    fn tools() -> (tempfile::TempDir, FakeRunner) {
        let dir = tempfile::tempdir().unwrap();
        crate::testing::write(&dir.path().join("croft/uv-receipt.toml"), RECEIPT);
        crate::testing::write(&dir.path().join("ruff/uv-receipt.toml"), "[tool]\nrequirements = [{ name = \"ruff\" }]\n");
        let tool_dir = dir.path().display().to_string();
        let runner = FakeRunner::new(move |_, args| match args.get(1).map(String::as_str) {
            Some("list") => "croft v1.0.0\n- croft\nruff v0.6.9\n- ruff\n".into(),
            Some("dir") => format!("{tool_dir}\n"),
            _ => String::new(),
        });
        (dir, runner)
    }

    #[test]
    fn git_tools_are_listed_by_url_and_pinned_at_their_commit() {
        let (_dir, runner) = tools();
        let uv = Uv::new(&runner, &env());
        let pkgs = uv.list().unwrap();
        assert_eq!((pkgs[0].source.as_str(), pkgs[0].reference.as_str()), ("git+https://github.com/user/croft", "v1.0"));
        assert_eq!(uv.pin(&pkgs[0]), "git+https://github.com/user/croft@4c638f60aaaabbbbccccddddeeeeffff00001111");
        assert_eq!((pkgs[1].source.as_str(), uv.pin(&pkgs[1]).as_str()), ("ruff", "ruff@0.6.9"));
        assert!(crate::backend::satisfies(&uv, &pkgs, "git+https://github.com/user/croft@v1.0"));
    }

    #[test]
    fn install_and_remove_run_uv_tool() {
        let (_dir, runner) = tools();
        let uv = Uv::new(&runner, &env());
        uv.install(&["ruff".into(), "httpie@3.2.3".into()]).unwrap();
        uv.remove(&["ruff".into(), "git+https://github.com/user/croft@v1.0".into()]).unwrap();
        let changes: Vec<String> = runner.calls.borrow().iter().filter(|call| call.contains("install") || call.contains("uninstall")).cloned().collect();
        assert_eq!(changes, ["uv tool install ruff@latest", "uv tool install httpie==3.2.3", "uv tool uninstall ruff croft"]);
    }
}
