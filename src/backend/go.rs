use super::{Backend, BackendError, Pkg, spec_base, spec_program};
use crate::env::Env;
use crate::runner::{RunError, Runner};
use std::fs;
use std::path::PathBuf;

// go programs, built with go install into GOBIN (or GOPATH/bin)
pub struct Go<'a> {
    runner: &'a dyn Runner,
    home: PathBuf,
}

impl<'a> Go<'a> {
    pub fn new(runner: &'a dyn Runner, env: &Env) -> Self {
        Go { runner, home: env.home.clone() }
    }

    // GOBIN if set, else GOPATH/bin, else go's default ~/go/bin
    fn bin(&self) -> Result<PathBuf, BackendError> {
        let output = self.runner.run("go", &["env".into(), "GOBIN".into(), "GOPATH".into()])?;
        let mut lines = output.lines().map(str::trim);
        let (gobin, gopath) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""));
        Ok(match (gobin, gopath) {
            ("", "") => self.home.join("go/bin"),
            ("", gopath) => PathBuf::from(gopath).join("bin"),
            (gobin, _) => PathBuf::from(gobin),
        })
    }
}

// blocks of `go version -m <dir>`: "<file>: go1.x", then tab-indented path and mod lines
fn parse_versions(output: &str) -> Vec<Pkg> {
    output.lines().fold(Vec::new(), |mut pkgs: Vec<Pkg>, line| {
        let fields: Vec<&str> = line.trim().split('\t').collect();
        match (line.starts_with(char::is_whitespace), fields.as_slice(), pkgs.last_mut()) {
            (false, ..) => {
                let file = line.rsplit_once(": ").map_or(line, |(file, _)| file);
                let name = file.rsplit('/').next().unwrap_or(file).to_string();
                pkgs.push(Pkg { name, manual: true, ..Pkg::default() });
            }
            (true, ["path", path], Some(pkg)) => pkg.source = path.to_string(),
            (true, ["mod", _, version, ..], Some(pkg)) => pkg.version = version.trim_start_matches('v').to_string(),
            _ => {}
        }
        pkgs
    })
}

impl Backend for Go<'_> {
    fn name(&self) -> &str {
        "go"
    }

    // every go-built binary in the bin dir, with the package path it came from
    fn list(&self) -> Result<Vec<Pkg>, BackendError> {
        let bin = match self.bin() {
            Err(BackendError::Run(RunError::Spawn { .. })) => return Ok(Vec::new()),
            bin => bin?,
        };
        if !bin.is_dir() {
            return Ok(Vec::new());
        }
        let output = self.runner.run("go", &["version".into(), "-m".into(), bin.display().to_string()])?;
        Ok(parse_versions(&output))
    }

    // go has no package index to search
    fn search(&self, _: &str) -> Result<Vec<Pkg>, BackendError> {
        Ok(Vec::new())
    }

    // go paths are checked by go install itself
    fn info(&self, spec: &str) -> Result<Option<Pkg>, BackendError> {
        Ok(Some(Pkg { name: spec_program(spec), source: spec_base(spec).into(), version: "latest".into(), ..Pkg::default() }))
    }

    // an unversioned spec means the latest release
    fn install(&self, specs: &[String]) -> Result<(), BackendError> {
        specs.iter().try_for_each(|spec| {
            let target = if spec_base(spec) == spec { format!("{spec}@latest") } else { spec.clone() };
            Ok(self.runner.interactive("go", &["install".into(), target])?)
        })
    }

    // go has no uninstall; the binary is deleted from the bin dir
    fn remove(&self, specs: &[String]) -> Result<(), BackendError> {
        let bin = self.bin()?;
        let installed = self.list()?;
        let files = specs.iter().filter_map(|spec| installed.iter().find(|pkg| pkg.source == spec_base(spec) || &pkg.name == spec));
        files.into_iter().try_for_each(|pkg| {
            let path = bin.join(&pkg.name);
            fs::remove_file(&path).map_err(|source| BackendError::Io { path, source })
        })
    }

    fn pin(&self, pkg: &Pkg) -> String {
        format!("{}@v{}", pkg.source, pkg.version)
    }

    fn bin_dir(&self) -> Option<PathBuf> {
        self.bin().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use std::path::Path;

    const VERSIONS: &str = "/g/bin/gopls: go1.26.5\n\tpath\tgolang.org/x/tools/gopls\n\tmod\tgolang.org/x/tools/gopls\tv0.23.0\th1:x=\n\tdep\tgithub.com/a/b\tv1.0.0\th1:y=\n/g/bin/lazygit: go1.26.5\n\tpath\tgithub.com/jesseduffield/lazygit\n\tmod\tgithub.com/jesseduffield/lazygit\tv0.40.2\th1:z=\n";

    // a gopath under a tempdir with a real bin dir, so list and remove have something to find
    fn setup() -> (tempfile::TempDir, FakeRunner, Env) {
        let dir = tempfile::tempdir().unwrap();
        let gopath = dir.path().join("g");
        fs::create_dir_all(gopath.join("bin")).unwrap();
        fs::write(gopath.join("bin/lazygit"), "").unwrap();
        let answer = gopath.display().to_string();
        let runner = FakeRunner::new(move |_, args| match args[0].as_str() {
            "env" => format!("\n{answer}\n"),
            "version" => VERSIONS.into(),
            _ => String::new(),
        });
        let env = Env::new(Path::new("/h"), Path::new("/"), Path::new("/s"));
        (dir, runner, env)
    }

    #[test]
    fn versions_parse_into_packages() {
        let pkgs = parse_versions(VERSIONS);
        assert_eq!(pkgs.len(), 2);
        assert_eq!((pkgs[0].name.as_str(), pkgs[0].source.as_str(), pkgs[0].version.as_str()), ("gopls", "golang.org/x/tools/gopls", "0.23.0"));
    }

    #[test]
    fn bin_falls_back_from_gobin_to_gopath() {
        let (dir, runner, env) = setup();
        assert_eq!(Go::new(&runner, &env).bin().unwrap(), dir.path().join("g/bin"));
    }

    #[test]
    fn install_defaults_to_latest() {
        let (_dir, runner, env) = setup();
        Go::new(&runner, &env).install(&["github.com/jesseduffield/lazygit".into(), "golang.org/x/tools/gopls@v0.16.0".into()]).unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(calls[..], ["go install github.com/jesseduffield/lazygit@latest", "go install golang.org/x/tools/gopls@v0.16.0"]);
    }

    #[test]
    fn remove_deletes_the_binary() {
        let (dir, runner, env) = setup();
        Go::new(&runner, &env).remove(&["github.com/jesseduffield/lazygit".into()]).unwrap();
        assert!(!dir.path().join("g/bin/lazygit").exists());
    }
}
