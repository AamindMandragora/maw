use super::BackendError;
use super::xbps::Xbps;
use crate::env::Env;
use crate::runner::Runner;
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

const VOID_PACKAGES: &str = "https://github.com/void-linux/void-packages.git";

// packages built from the dotfiles' srcpkgs/<name>/template with xbps-src, in a void-packages clone maw owns
pub struct SrcPkgs<'a> {
    runner: &'a dyn Runner,
    env: Env,
    templates: PathBuf,
    clone: PathBuf,
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> BackendError + '_ {
    move |source| BackendError::Io { path: path.into(), source }
}

impl<'a> SrcPkgs<'a> {
    pub fn new(runner: &'a dyn Runner, env: &Env, templates: &Path, clone: &Path) -> Self {
        SrcPkgs { runner, env: env.clone(), templates: templates.into(), clone: clone.into() }
    }

    // whether the dotfiles have a template for this name
    pub fn has(&self, name: &str) -> bool {
        self.templates.join(name).join("template").is_file()
    }

    // where xbps-src puts what it builds, a local repo xbps can install from
    pub fn binpkgs(&self) -> PathBuf {
        self.clone.join("hostdir/binpkgs")
    }

    // a template's version_revision, like xbps prints it: "1.2.0_1"
    pub fn version(&self, name: &str) -> Option<String> {
        let text = fs::read_to_string(self.templates.join(name).join("template")).ok()?;
        let field = |key: &str| {
            let line = text.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))?;
            Some(line.trim().trim_matches(|char| char == '"' || char == '\'').to_string())
        };
        Some(format!("{}_{}", field("version")?, field("revision")?))
    }

    // a built package file of an exact version in binpkgs, if one is there
    pub fn built(&self, pkgver: &str) -> Option<PathBuf> {
        let prefix = format!("{pkgver}.");
        fs::read_dir(self.binpkgs())
            .ok()?
            .filter_map(|entry| Some(entry.ok()?.path()))
            .find(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(&prefix)) && path.extension().is_some_and(|ext| ext == "xbps"))
    }

    // clones void-packages shallowly and bootstraps its build root, the first time only
    fn ensure_clone(&self) -> Result<(), BackendError> {
        if self.clone.join("xbps-src").exists() {
            return Ok(());
        }
        fs::create_dir_all(self.clone.parent().unwrap()).map_err(io(&self.clone))?;
        let clone = self.clone.display().to_string();
        self.runner.interactive("git", &["clone".into(), "--depth".into(), "1".into(), VOID_PACKAGES.into(), clone])?;
        Ok(self.runner.interactive(&self.clone.join("xbps-src").display().to_string(), &["binary-bootstrap".into()])?)
    }

    // links the template into the clone's srcpkgs, unless void-packages has its own package by that name
    fn link(&self, name: &str) -> Result<(), BackendError> {
        let (link, template) = (self.clone.join("srcpkgs").join(name), self.templates.join(name));
        match fs::read_link(&link) {
            Ok(target) if target == template => return Ok(()),
            Ok(_) => fs::remove_file(&link).map_err(io(&link))?,
            Err(_) if fs::symlink_metadata(&link).is_ok() => return Err(BackendError::Taken(name.into())),
            Err(_) => {}
        }
        fs::create_dir_all(link.parent().unwrap()).map_err(io(&link))?;
        symlink(&template, &link).map_err(io(&link))?;
        self.exclude(name)
    }

    // keeps a linked template out of the clone's git status
    fn exclude(&self, name: &str) -> Result<(), BackendError> {
        let exclude = self.clone.join(".git/info/exclude");
        let line = format!("/srcpkgs/{name}");
        if fs::read_to_string(&exclude).unwrap_or_default().lines().any(|existing| existing == line) {
            return Ok(());
        }
        fs::create_dir_all(exclude.parent().unwrap()).map_err(io(&exclude))?;
        let mut file = fs::OpenOptions::new().create(true).append(true).open(&exclude).map_err(io(&exclude))?;
        writeln!(file, "{line}").map_err(io(&exclude))
    }

    // builds each template with xbps-src into binpkgs
    pub fn build(&self, names: &[String]) -> Result<(), BackendError> {
        self.ensure_clone()?;
        let xbps_src = self.clone.join("xbps-src").display().to_string();
        names.iter().try_for_each(|name| {
            self.link(name)?;
            Ok(self.runner.interactive(&xbps_src, &["pkg".into(), name.clone()])?)
        })
    }

    // builds, then installs from binpkgs; force reinstalls an installed package at the new build
    pub fn install(&self, names: &[String], force: bool) -> Result<(), BackendError> {
        self.build(names)?;
        Xbps::new(self.runner, &self.env).install_from(&self.binpkgs(), names, force)
    }

    // whether the clone has its own package by this name, not a link to one of ours
    fn is_official(&self, name: &str) -> bool {
        let entry = self.clone.join("srcpkgs").join(name);
        fs::symlink_metadata(&entry).is_ok() && fs::read_link(&entry).map_or(true, |target| target != self.templates.join(name))
    }

    // writes a blank srcpkgs/<name>/template, refusing names void-packages already uses
    pub fn new_template(&self, name: &str, maintainer: &str) -> Result<PathBuf, BackendError> {
        let file = self.templates.join(name).join("template");
        if file.exists() {
            return Err(BackendError::TemplateExists(name.into()));
        }
        if self.is_official(name) {
            return Err(BackendError::Taken(name.into()));
        }
        let text = format!(
            "# Template file for '{name}'\npkgname={name}\nversion=0.1.0\nrevision=1\nbuild_style=\nshort_desc=\"\"\nmaintainer=\"{maintainer}\"\nlicense=\"\"\nhomepage=\"\"\ndistfiles=\"\"\nchecksum=\n"
        );
        fs::create_dir_all(file.parent().unwrap()).map_err(io(&file))?;
        fs::write(&file, text).map_err(io(&file))?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;

    struct Setup {
        dir: tempfile::TempDir,
        runner: FakeRunner,
    }

    impl Setup {
        fn new() -> Self {
            Setup { dir: tempfile::tempdir().unwrap(), runner: FakeRunner::new(|_, _| String::new()) }
        }

        fn src(&self) -> SrcPkgs<'_> {
            let env = Env::new(&self.dir.path().join("home"), &self.dir.path().join("root"), Path::new("/s"));
            SrcPkgs::new(&self.runner, &env, &self.dir.path().join("dots/srcpkgs"), &self.dir.path().join("clone"))
        }

        // a clone that already has xbps-src and an official package called taken
        fn cloned(&self) {
            let clone = self.dir.path().join("clone");
            fs::create_dir_all(clone.join("srcpkgs/taken")).unwrap();
            fs::write(clone.join("xbps-src"), "").unwrap();
        }
    }

    #[test]
    fn version_joins_version_and_revision() {
        let setup = Setup::new();
        setup.src().new_template("hello", "me <me@x>").unwrap();
        assert_eq!(setup.src().version("hello").as_deref(), Some("0.1.0_1"));
        assert!(setup.src().has("hello") && !setup.src().has("other"));
    }

    #[test]
    fn first_build_clones_and_bootstraps() {
        let setup = Setup::new();
        setup.src().new_template("hello", "me").unwrap();
        setup.src().build(&["hello".into()]).unwrap();

        let clone = setup.dir.path().join("clone");
        let calls = setup.runner.calls.borrow();
        assert_eq!(calls[0], format!("git clone --depth 1 {VOID_PACKAGES} {}", clone.display()));
        assert_eq!(calls[1], format!("{} binary-bootstrap", clone.join("xbps-src").display()));
        assert_eq!(calls[2], format!("{} pkg hello", clone.join("xbps-src").display()));
    }

    #[test]
    fn templates_are_linked_and_excluded_from_git() {
        let setup = Setup::new();
        setup.cloned();
        setup.src().new_template("hello", "me").unwrap();
        setup.src().build(&["hello".into()]).unwrap();
        setup.src().build(&["hello".into()]).unwrap();

        let clone = setup.dir.path().join("clone");
        assert_eq!(fs::read_link(clone.join("srcpkgs/hello")).unwrap(), setup.dir.path().join("dots/srcpkgs/hello"));
        assert_eq!(fs::read_to_string(clone.join(".git/info/exclude")).unwrap(), "/srcpkgs/hello\n");
        assert!(!setup.runner.calls.borrow().iter().any(|call| call.starts_with("git clone")));
    }

    #[test]
    fn official_packages_are_never_shadowed() {
        let setup = Setup::new();
        setup.cloned();
        assert!(matches!(setup.src().new_template("taken", "me"), Err(BackendError::Taken(_))));

        // a template written before the clone existed is still refused at build time
        let template = setup.dir.path().join("dots/srcpkgs/taken/template");
        fs::create_dir_all(template.parent().unwrap()).unwrap();
        fs::write(&template, "pkgname=taken\n").unwrap();
        assert!(matches!(setup.src().build(&["taken".into()]), Err(BackendError::Taken(_))));
        assert!(matches!(setup.src().new_template("taken", "me"), Err(BackendError::TemplateExists(_))));
    }

    #[test]
    fn install_builds_then_installs_from_binpkgs() {
        let setup = Setup::new();
        setup.cloned();
        setup.src().new_template("hello", "me").unwrap();
        setup.src().install(&["hello".into()], true).unwrap();
        let binpkgs = setup.dir.path().join("clone/hostdir/binpkgs");
        let root = setup.dir.path().join("root");
        assert_eq!(setup.runner.calls.borrow().last().unwrap(), &format!("xbps-install -r {} -R {} -fy hello", root.display(), binpkgs.display()));
    }
}
