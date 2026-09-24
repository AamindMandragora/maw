use super::{Backend, BackendError, Pkg, SystemBackend, split_pkgver};
use crate::env::Env;
use crate::runner::{RunError, Runner, as_root};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

// void's package manager; a sysroot other than / is used directly instead of through sudo
pub struct Xbps<'a> {
    runner: &'a dyn Runner,
    sysroot: PathBuf,
}

impl<'a> Xbps<'a> {
    pub fn new(runner: &'a dyn Runner, env: &Env) -> Self {
        Xbps { runner, sysroot: env.sysroot.clone() }
    }

    // args that point xbps at the system root, none for the real one
    fn root_args(&self) -> Vec<String> {
        if self.sysroot == Path::new("/") { Vec::new() } else { vec!["-r".into(), self.sysroot.display().to_string()] }
    }

    fn query(&self, args: &[&str]) -> Result<String, RunError> {
        let args: Vec<String> = self.root_args().into_iter().chain(args.iter().map(|arg| arg.to_string())).collect();
        self.runner.run("xbps-query", &args)
    }

    // a root command on the terminal, pointed at the system root
    fn privileged(&self, program: &str, args: &[String]) -> Result<(), RunError> {
        let args: Vec<String> = self.root_args().into_iter().chain(args.iter().cloned()).collect();
        as_root(self.runner, &self.sysroot, program, &args)
    }

    // installs names or exact versions with a local repo added, like xbps-src's hostdir/binpkgs; force allows downgrades
    pub fn install_from(&self, repo: &Path, specs: &[String], force: bool) -> Result<(), BackendError> {
        let flags = if force { "-fy" } else { "-y" };
        let args: Vec<String> = ["-R".to_string(), repo.display().to_string(), flags.into()].into_iter().chain(specs.iter().cloned()).collect();
        Ok(self.privileged("xbps-install", &args)?)
    }

    // where xbps keeps every package it downloaded
    fn cache_dir(&self) -> PathBuf {
        self.sysroot.join("var/cache/xbps")
    }

    fn parse_error(&self, line: &str) -> BackendError {
        BackendError::Parse { backend: "xbps".into(), line: line.into() }
    }

    // "<marker> <pkgver>  <description>" lines, as -l and -Rs print them
    fn parse_listing(&self, output: &str, manual: &HashSet<String>) -> Result<Vec<Pkg>, BackendError> {
        output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let mut words = line.split_whitespace();
                let pkgver = words.nth(1).ok_or_else(|| self.parse_error(line))?;
                let (name, version) = split_pkgver(pkgver).ok_or_else(|| self.parse_error(line))?;
                let description = words.collect::<Vec<_>>().join(" ");
                Ok(Pkg { manual: manual.contains(&name), source: name.clone(), name, version, description, ..Pkg::default() })
            })
            .collect()
    }
}

// "key: value" lines of xbps-query -R, continuation lines skipped
fn field<'o>(output: &'o str, key: &str) -> &'o str {
    output.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(": ")).unwrap_or("")
}

impl Backend for Xbps<'_> {
    fn name(&self) -> &str {
        "xbps"
    }

    // everything installed, with the manually installed ones marked; a fresh root without a package database has nothing
    fn list(&self) -> Result<Vec<Pkg>, BackendError> {
        if !self.sysroot.join("var/db/xbps").exists() {
            return Ok(Vec::new());
        }
        let manual: HashSet<String> = self.query(&["-m"])?.lines().filter_map(|line| Some(split_pkgver(line.trim())?.0)).collect();
        self.parse_listing(&self.query(&["-l"])?, &manual)
    }

    fn search(&self, term: &str) -> Result<Vec<Pkg>, BackendError> {
        self.parse_listing(&self.query(&["-Rs", term])?, &HashSet::new())
    }

    // a failed query means the repo has no such package
    fn info(&self, name: &str) -> Result<Option<Pkg>, BackendError> {
        let output = match self.query(&["-R", name]) {
            Ok(output) => output,
            Err(RunError::Failed { .. }) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let (name, version) = split_pkgver(field(&output, "pkgver")).ok_or_else(|| self.parse_error(&output))?;
        let (description, homepage) = (field(&output, "short_desc").into(), field(&output, "homepage").into());
        Ok(Some(Pkg { source: name.clone(), name, version, description, homepage, ..Pkg::default() }))
    }

    fn install(&self, names: &[String]) -> Result<(), BackendError> {
        let args: Vec<String> = ["-y".to_string()].into_iter().chain(names.iter().cloned()).collect();
        Ok(self.privileged("xbps-install", &args)?)
    }

    // -R also drops dependencies nothing else needs
    fn remove(&self, names: &[String]) -> Result<(), BackendError> {
        let args: Vec<String> = ["-Ry".to_string()].into_iter().chain(names.iter().cloned()).collect();
        Ok(self.privileged("xbps-remove", &args)?)
    }

    fn pin(&self, pkg: &Pkg) -> String {
        format!("{}-{}", pkg.name, pkg.version)
    }
}

impl SystemBackend for Xbps<'_> {
    fn sync(&self) -> Result<(), BackendError> {
        Ok(self.privileged("xbps-install", &["-Suy".into()])?)
    }

    // <pkgver>.<arch>.xbps in the cache
    fn cached(&self, pkgver: &str) -> Option<PathBuf> {
        let prefix = format!("{pkgver}.");
        fs::read_dir(self.cache_dir())
            .ok()?
            .filter_map(|entry| Some(entry.ok()?.path()))
            .find(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(&prefix)) && path.extension().is_some_and(|ext| ext == "xbps"))
    }

    // the cache isn't a repo until indexed, so only the files needed are added to its index first; -f allows downgrades
    fn install_versions(&self, pkgvers: &[String]) -> Result<(), BackendError> {
        let files: Vec<String> = pkgvers.iter().filter_map(|pkgver| self.cached(pkgver)).map(|file| file.display().to_string()).collect();
        if !files.is_empty() {
            as_root(self.runner, &self.sysroot, "xbps-rindex", &["-a".to_string()].into_iter().chain(files).collect::<Vec<_>>())?;
        }
        self.install_from(&self.cache_dir(), pkgvers, true)
    }

    fn hold(&self, names: &[String], hold: bool) -> Result<(), BackendError> {
        let mode = if hold { "hold" } else { "unhold" };
        let args: Vec<String> = ["-m".to_string(), mode.into()].into_iter().chain(names.iter().cloned()).collect();
        Ok(self.privileged("xbps-pkgdb", &args)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;

    fn env(sysroot: &str) -> Env {
        Env::new(Path::new("/h"), Path::new(sysroot), Path::new("/s"))
    }

    // answers -m, -l, -Rs, and -R like a system with foot and its terminfo installed
    fn fake(_: &str, args: &[String]) -> String {
        match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            [.., "-m"] => "foot-1.28.0_1\n".into(),
            [.., "-l"] => "ii foot-1.28.0_1          Fast terminal\nii foot-terminfo-1.28.0_1 Terminfo for foot\n".into(),
            [.., "-Rs", _] => "[*] foot-1.28.0_1  Fast terminal\n[-] footclient-0.1_1  Client\n".into(),
            [.., "-R", _] => "homepage: https://codeberg.org/dnkl/foot\npkgver: foot-1.28.0_1\nshort_desc: Fast terminal\nprovides:\n\tcmd:foot\n".into(),
            _ => String::new(),
        }
    }

    #[test]
    fn empty_root_has_nothing_installed() {
        let runner = FakeRunner::new(fake);
        assert!(Xbps::new(&runner, &env("/nonexistent/root")).list().unwrap().is_empty());
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn list_marks_manual_packages() {
        let runner = FakeRunner::new(fake);
        let installed = Xbps::new(&runner, &env("/")).list().unwrap();
        assert_eq!(installed.len(), 2);
        assert!(installed[0].manual && !installed[1].manual);
        assert_eq!((installed[1].name.as_str(), installed[1].description.as_str()), ("foot-terminfo", "Terminfo for foot"));
    }

    #[test]
    fn info_reads_fields() {
        let runner = FakeRunner::new(fake);
        let foot = Xbps::new(&runner, &env("/")).info("foot").unwrap().unwrap();
        assert_eq!((foot.version.as_str(), foot.homepage.as_str()), ("1.28.0_1", "https://codeberg.org/dnkl/foot"));
    }

    #[test]
    fn search_parses_both_markers() {
        let runner = FakeRunner::new(fake);
        let names: Vec<String> = Xbps::new(&runner, &env("/")).search("foot").unwrap().into_iter().map(|pkg| pkg.name).collect();
        assert_eq!(names, ["foot", "footclient"]);
    }

    #[test]
    fn versions_come_from_the_indexed_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("var/cache/xbps");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("foot-1.20_1.x86_64.xbps"), "").unwrap();
        fs::write(cache.join("foot-1.20_1.x86_64.xbps.sig2"), "").unwrap();

        let runner = FakeRunner::new(fake);
        let xbps = Xbps::new(&runner, &Env::new(Path::new("/h"), dir.path(), Path::new("/s")));
        assert_eq!(xbps.cached("foot-1.20_1"), Some(cache.join("foot-1.20_1.x86_64.xbps")));
        assert_eq!(xbps.cached("foot-1.19_1"), None);

        xbps.install_versions(&["foot-1.20_1".into(), "bash-5.1_1".into()]).unwrap();
        xbps.hold(&["foot".into()], true).unwrap();
        let root = dir.path().display();
        assert_eq!(*runner.calls.borrow(), [
            format!("xbps-rindex -a {}", cache.join("foot-1.20_1.x86_64.xbps").display()),
            format!("xbps-install -r {root} -R {} -fy foot-1.20_1 bash-5.1_1", cache.display()),
            format!("xbps-pkgdb -r {root} -m hold foot"),
        ]);
    }

    #[test]
    fn real_root_goes_through_sudo_and_scratch_root_does_not() {
        let runner = FakeRunner::new(fake);
        Xbps::new(&runner, &env("/")).install(&["foot".into()]).unwrap();
        Xbps::new(&runner, &env("/tmp/root")).remove(&["foot".into()]).unwrap();
        assert_eq!(*runner.calls.borrow(), ["sudo xbps-install -y foot", "xbps-remove -r /tmp/root -Ry foot"]);
    }
}
