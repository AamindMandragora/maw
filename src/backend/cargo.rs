use super::{Backend, BackendError, Pkg, spec_base, spec_program};
use crate::env::Env;
use crate::runner::{RunError, Runner};
use std::path::PathBuf;

// crates from crates.io or git, built with cargo install into ~/.cargo/bin
pub struct Cargo<'a> {
    runner: &'a dyn Runner,
    bin: PathBuf,
}

impl<'a> Cargo<'a> {
    pub fn new(runner: &'a dyn Runner, env: &Env) -> Self {
        Cargo { runner, bin: env.home.join(".cargo/bin") }
    }

    fn cargo(&self, args: &[&str]) -> Result<String, BackendError> {
        Ok(self.runner.run("cargo", &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())?)
    }

    // the executables a crates.io version builds, from its api; None when the api can't be reached
    fn programs(&self, name: &str, version: &str) -> Option<Vec<String>> {
        let url = format!("https://crates.io/api/v1/crates/{name}/{version}");
        let args = ["-sf", "-A", "maw (void linux system manager)", &url].map(String::from);
        let response: serde_json::Value = serde_json::from_str(&self.runner.run("curl", &args).ok()?).ok()?;
        let names = response["version"]["bin_names"].as_array()?;
        Some(names.iter().filter_map(|name| Some(name.as_str()?.to_string())).collect())
    }
}

// urls cargo installs with --git rather than from crates.io
pub fn is_git(spec: &str) -> bool {
    ["https://", "http://", "git@", "git://", "ssh://"].iter().any(|prefix| spec.starts_with(prefix)) || spec.ends_with(".git")
}

// "croft-software v0.1.941 (https://github.com/vitali87/croft.git?tag=v0.1.941#4c638f60):" -> name, version, source,
// and for git crates the ref (tag, branch, or rev) and commit
fn parse_installed(line: &str) -> Option<Pkg> {
    let (name, rest) = line.trim_end_matches(':').split_once(' ')?;
    let (version, origin) = rest.split_once(" (").unwrap_or((rest, ""));
    let origin = origin.trim_end_matches(')');

    // git crates are declared by url without the ref or commit; crates.io ones by name
    let (url, commit) = origin.split_once('#').unwrap_or((origin, ""));
    let (url, query) = url.split_once('?').unwrap_or((url, ""));
    let reference = query.split('&').find_map(|pair| ["tag=", "branch=", "rev="].iter().find_map(|key| pair.strip_prefix(key))).unwrap_or_default();
    let source = if url.is_empty() { name } else { url };
    Some(Pkg { name: name.into(), source: source.into(), version: version.trim_start_matches('v').into(), build: commit.into(), reference: reference.into(), manual: true, ..Pkg::default() })
}

// the cargo install args for a spec: a crate, or --git with a ref after @ as --rev (a commit: 7 to 40 hex digits),
// --tag (starting with a digit, or v and a digit), or --branch (anything else)
fn install_args(spec: &str) -> Vec<String> {
    if !is_git(spec) {
        return vec![spec.to_string()];
    }
    let url = spec_base(spec);
    let git = vec!["--git".to_string(), url.to_string()];
    let Some(reference) = spec[url.len()..].strip_prefix('@') else { return git };
    let is_commit = (7..=40).contains(&reference.len()) && reference.chars().all(|char| char.is_ascii_hexdigit());
    let is_tag = reference.trim_start_matches('v').starts_with(|char: char| char.is_ascii_digit());
    let flag = if is_commit { "--rev" } else if is_tag { "--tag" } else { "--branch" };
    git.into_iter().chain([flag.to_string(), reference.to_string()]).collect()
}

// `name = "0.26.1"    # description` lines of cargo search
fn parse_search(line: &str) -> Option<Pkg> {
    let (name, rest) = line.split_once(" = \"")?;
    let (version, description) = rest.split_once('"')?;
    let description = description.trim().trim_start_matches('#').trim();
    Some(Pkg { name: name.into(), source: name.into(), version: version.into(), description: description.into(), ..Pkg::default() })
}

impl Backend for Cargo<'_> {
    fn name(&self) -> &str {
        "cargo"
    }

    // top-level lines of cargo install --list; the indented ones are binaries; no cargo means no crates
    fn list(&self) -> Result<Vec<Pkg>, BackendError> {
        let output = match self.cargo(&["install", "--list"]) {
            Err(BackendError::Run(RunError::Spawn { .. })) => return Ok(Vec::new()),
            output => output?,
        };
        Ok(output.lines().filter(|line| !line.starts_with(char::is_whitespace)).filter_map(parse_installed).collect())
    }

    fn search(&self, term: &str) -> Result<Vec<Pkg>, BackendError> {
        Ok(self.cargo(&["search", "--limit", "20", term])?.lines().filter_map(parse_search).collect())
    }

    // an exact crates.io match, with the programs it builds; git urls aren't checked until they're installed
    fn info(&self, spec: &str) -> Result<Option<Pkg>, BackendError> {
        if is_git(spec) {
            return Ok(Some(Pkg { name: spec_program(spec), source: spec_base(spec).into(), ..Pkg::default() }));
        }
        let name = spec_base(spec);
        let found = self.cargo(&["search", "--limit", "10", name])?.lines().filter_map(parse_search).find(|pkg| pkg.name == name);
        Ok(found.map(|pkg| Pkg { programs: self.programs(&pkg.name, &pkg.version), ..pkg }))
    }

    // one cargo install per spec, locked to each crate's own lockfile
    fn install(&self, specs: &[String]) -> Result<(), BackendError> {
        specs.iter().try_for_each(|spec| {
            let args: Vec<String> = ["install".to_string(), "--locked".into()].into_iter().chain(install_args(spec)).collect();
            Ok(self.runner.interactive("cargo", &args)?)
        })
    }

    // uninstalls by crate name, found from the spec's source
    fn remove(&self, specs: &[String]) -> Result<(), BackendError> {
        let installed = self.list()?;
        let names = specs.iter().filter_map(|spec| installed.iter().find(|pkg| pkg.source == spec_base(spec) || &pkg.name == spec));
        names.into_iter().try_for_each(|pkg| Ok(self.runner.interactive("cargo", &["uninstall".into(), pkg.name.clone()])?))
    }

    fn tool(&self) -> Option<(&'static str, &'static str)> {
        Some(("cargo", "cargo"))
    }

    // a crate at its version, a git crate at its commit
    fn pin(&self, pkg: &Pkg) -> String {
        match (is_git(&pkg.source), pkg.build.is_empty()) {
            (false, _) => format!("{}@{}", pkg.name, pkg.version),
            (true, true) => pkg.source.clone(),
            (true, false) => format!("{}@{}", pkg.source, pkg.build),
        }
    }

    fn bin_dir(&self) -> Option<PathBuf> {
        Some(self.bin.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use std::path::Path;

    const LIST: &str = "croft-software v0.1.941 (https://github.com/vitali87/croft.git?tag=v0.1.941#4c638f60):\n    croft\nbat v0.26.1:\n    bat\n";
    const SEARCH: &str = "bat = \"0.26.1\"        # A cat(1) clone with wings.\nbat-cli = \"0.26.7\"    # Toolkit\n... and 1073 crates more (use --limit N to see more)\n";

    fn fake(program: &str, args: &[String]) -> String {
        if program == "curl" {
            return r#"{"version":{"bin_names":["bat"]}}"#.into();
        }
        match args.first().map(String::as_str) {
            Some("install") if args.get(1).map(String::as_str) == Some("--list") => LIST.into(),
            Some("search") => SEARCH.into(),
            _ => String::new(),
        }
    }

    fn cargo(runner: &FakeRunner) -> Cargo<'_> {
        Cargo::new(runner, &Env::new(Path::new("/h"), Path::new("/"), Path::new("/s")))
    }

    #[test]
    fn list_reads_crates_io_and_git_sources() {
        let runner = FakeRunner::new(fake);
        let installed = cargo(&runner).list().unwrap();
        assert_eq!((installed[0].name.as_str(), installed[0].source.as_str()), ("croft-software", "https://github.com/vitali87/croft.git"));
        assert_eq!((installed[1].source.as_str(), installed[1].version.as_str()), ("bat", "0.26.1"));
    }

    #[test]
    fn info_needs_an_exact_name() {
        let runner = FakeRunner::new(fake);
        assert_eq!(cargo(&runner).info("bat@0.24").unwrap().unwrap().description, "A cat(1) clone with wings.");
        assert!(cargo(&runner).info("ba").unwrap().is_none());
    }

    #[test]
    fn info_asks_crates_io_for_the_programs() {
        let runner = FakeRunner::new(fake);
        assert_eq!(cargo(&runner).info("bat").unwrap().unwrap().programs, Some(vec!["bat".to_string()]));
        assert!(runner.calls.borrow().iter().any(|call| call.ends_with("https://crates.io/api/v1/crates/bat/0.26.1")));
    }

    #[test]
    fn unreachable_api_means_unknown_programs() {
        let runner = FakeRunner::fallible(|program, args| match program {
            "curl" => Err(crate::runner::RunError::Failed { program: "curl".into(), stderr: String::new() }),
            _ => Ok(fake(program, args)),
        });
        assert_eq!(cargo(&runner).info("bat").unwrap().unwrap().programs, None);
    }

    #[test]
    fn install_locks_and_uses_git_for_urls() {
        let runner = FakeRunner::new(fake);
        cargo(&runner).install(&["bat@0.24".into(), "https://github.com/vitali87/croft.git".into()]).unwrap();
        assert_eq!(*runner.calls.borrow(), ["cargo install --locked bat@0.24", "cargo install --locked --git https://github.com/vitali87/croft.git"]);
    }

    #[test]
    fn remove_finds_git_crates_by_url() {
        let runner = FakeRunner::new(fake);
        cargo(&runner).remove(&["https://github.com/vitali87/croft.git".into()]).unwrap();
        assert_eq!(runner.calls.borrow().last().unwrap(), "cargo uninstall croft-software");
    }

    #[test]
    fn pins_are_exact_versions_or_urls() {
        let runner = FakeRunner::new(fake);
        let installed = cargo(&runner).list().unwrap();
        assert_eq!(cargo(&runner).pin(&installed[1]), "bat@0.26.1");
        assert_eq!(cargo(&runner).pin(&installed[0]), "https://github.com/vitali87/croft.git@4c638f60");
        assert_eq!(installed[0].reference, "v0.1.941");
    }

    #[test]
    fn git_refs_become_rev_tag_or_branch() {
        let runner = FakeRunner::new(fake);
        let url = "https://github.com/vitali87/croft.git";
        let specs = [format!("{url}@4c638f60"), format!("{url}@v0.2.0"), format!("{url}@main"), "git@github.com:user/tool.git@1.0".to_string()];
        cargo(&runner).install(&specs).unwrap();
        assert_eq!(*runner.calls.borrow(), [
            format!("cargo install --locked --git {url} --rev 4c638f60"),
            format!("cargo install --locked --git {url} --tag v0.2.0"),
            format!("cargo install --locked --git {url} --branch main"),
            "cargo install --locked --git git@github.com:user/tool.git --tag 1.0".to_string(),
        ]);
    }
}
