use super::xbps::Xbps;
use super::{Backend, BackendError, SystemBackend};
use crate::env::Env;
use crate::runner::Runner;
use std::fs;
use std::io::Write;
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

// whether srcpkgs/<name> makes a source package: a template of its own, or patches for void's template
pub fn is_source(templates: &Path, name: &str) -> bool {
    templates.join(name).join("template").is_file() || is_patches(templates, name)
}

// srcpkgs/<name> with patches and no template: void's own package, built with these patches added
pub fn is_patches(templates: &Path, name: &str) -> bool {
    !templates.join(name).join("template").is_file() && templates.join(name).join("patches").is_dir()
}

// a template's version_revision, like xbps prints it: "1.2.0_1"
fn template_version(file: &Path) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let field = |key: &str| {
        let line = text.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))?;
        Some(line.trim().trim_matches(|char| char == '"' || char == '\'').to_string())
    };
    Some(format!("{}_{}", field("version")?, field("revision")?))
}

// patch files in a patches dir, in the order xbps-src applies them: its series file, else by name
fn patch_order(dir: &Path) -> Vec<String> {
    if let Ok(series) = fs::read_to_string(dir.join("series")) {
        return series.lines().map(str::trim).filter(|line| !line.is_empty()).map(String::from).collect();
    }
    let mut names: Vec<String> = fs::read_dir(dir).into_iter().flatten().filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned())).collect();
    names.retain(|name| name != "series" && !name.ends_with(".args"));
    names.sort();
    names
}

// the packages maw built with patches and holds, kept in the state dir
fn patched_file(env: &Env) -> PathBuf {
    env.state_dir.join("patched")
}

pub fn patched(env: &Env) -> Vec<String> {
    fs::read_to_string(patched_file(env)).unwrap_or_default().lines().map(String::from).collect()
}

fn save_patched(env: &Env, names: &[String]) -> Result<(), BackendError> {
    let file = patched_file(env);
    fs::create_dir_all(file.parent().unwrap()).map_err(io(&file))?;
    fs::write(&file, names.iter().map(|name| format!("{name}\n")).collect::<String>()).map_err(io(&file))
}

impl<'a> SrcPkgs<'a> {
    pub fn new(runner: &'a dyn Runner, env: &Env, templates: &Path, clone: &Path) -> Self {
        SrcPkgs { runner, env: env.clone(), templates: templates.into(), clone: clone.into() }
    }

    // whether the dotfiles build this name from source, from a template or patches
    pub fn has(&self, name: &str) -> bool {
        is_source(&self.templates, name)
    }

    // whether the dotfiles only patch void's template for this name
    pub fn patches(&self, name: &str) -> bool {
        is_patches(&self.templates, name)
    }

    // where xbps-src puts what it builds, a local repo xbps can install from
    pub fn binpkgs(&self) -> PathBuf {
        self.clone.join("hostdir/binpkgs")
    }

    // the version a build makes: our template's, or void's for patches
    pub fn version(&self, name: &str) -> Option<String> {
        match self.patches(name) {
            true => template_version(&self.clone.join("srcpkgs").join(name).join("template")),
            false => template_version(&self.templates.join(name).join("template")),
        }
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

    // copies the template into the clone's srcpkgs, fresh each build, unless void-packages has its own package by that name;
    // a copy rather than a link, since xbps-src treats a symlinked srcpkgs entry as a subpackage
    fn place(&self, name: &str) -> Result<(), BackendError> {
        if self.patches(name) {
            return self.add_patches(name);
        }
        let copy = self.clone.join("srcpkgs").join(name);
        if self.is_official(name) {
            return Err(BackendError::Taken(name.into()));
        }

        // clear the last copy, or a link from before maw copied
        match fs::symlink_metadata(&copy) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(&copy).map_err(io(&copy))?,
            Ok(_) => fs::remove_file(&copy).map_err(io(&copy))?,
            Err(_) => {}
        }

        copy_dir(&self.templates.join(name), &copy)?;
        self.exclude(name)
    }

    // restores void's package dir, then adds our patches after void's own through a series file
    fn add_patches(&self, name: &str) -> Result<(), BackendError> {
        if !self.is_official(name) {
            return Err(BackendError::NotInVoid(name.into()));
        }
        let package = format!("srcpkgs/{name}");
        let clone = self.clone.display().to_string();
        self.runner.run("git", &["-C".into(), clone.clone(), "checkout".into(), "--".into(), package.clone()])?;
        self.runner.run("git", &["-C".into(), clone, "clean".into(), "-fdq".into(), "--".into(), package])?;

        // ours are copied in as maw-<file>, so they never collide with void's
        let (ours, theirs) = (self.templates.join(name).join("patches"), self.clone.join("srcpkgs").join(name).join("patches"));
        let official = patch_order(&theirs);
        fs::create_dir_all(&theirs).map_err(io(&theirs))?;
        fs::read_dir(&ours).map_err(io(&ours))?.try_for_each(|entry| {
            let entry = entry.map_err(io(&ours))?;
            let target = theirs.join(format!("maw-{}", entry.file_name().to_string_lossy()));
            fs::copy(entry.path(), &target).map(drop).map_err(io(&target))
        })?;

        // void's patches keep their order, and ours follow in theirs
        let series: String = official.into_iter().chain(patch_order(&ours).into_iter().map(|patch| format!("maw-{patch}"))).map(|patch| patch + "\n").collect();
        fs::write(theirs.join("series"), series).map_err(io(&theirs))
    }

    // moves the clone to void-packages' latest when the repos have a different version than its template, so patches build on what void ships
    fn refresh(&self, name: &str) -> Result<(), BackendError> {
        let shipped = Xbps::new(self.runner, &self.env).info(name).ok().flatten().map(|pkg| pkg.version);
        if shipped.is_none() || shipped == self.version(name) {
            return Ok(());
        }
        let clone = self.clone.display().to_string();
        self.runner.interactive("git", &["-C".into(), clone.clone(), "fetch".into(), "--depth".into(), "1".into(), "origin".into(), "master".into()])?;
        Ok(self.runner.run("git", &["-C".into(), clone, "reset".into(), "-q".into(), "--hard".into(), "FETCH_HEAD".into()]).map(drop)?)
    }

    // keeps a copied template out of the clone's git status
    fn exclude(&self, name: &str) -> Result<(), BackendError> {
        let exclude = self.clone.join(".git/info/exclude");
        if self.is_copied(name) {
            return Ok(());
        }
        let line = format!("/srcpkgs/{name}");
        fs::create_dir_all(exclude.parent().unwrap()).map_err(io(&exclude))?;
        let mut file = fs::OpenOptions::new().create(true).append(true).open(&exclude).map_err(io(&exclude))?;
        writeln!(file, "{line}").map_err(io(&exclude))
    }

    // builds each template with xbps-src into binpkgs
    pub fn build(&self, names: &[String]) -> Result<(), BackendError> {
        self.ensure_clone()?;
        let xbps_src = self.clone.join("xbps-src").display().to_string();
        names.iter().try_for_each(|name| {
            // a patched build has void's version, so -f rebuilds over an earlier one
            let mut args = vec!["pkg".to_string(), name.clone()];
            if self.patches(name) {
                self.refresh(name)?;
                args.insert(0, "-f".into());
            }
            self.place(name)?;
            Ok(self.runner.interactive(&xbps_src, &args)?)
        })
    }

    // builds, then installs from binpkgs; force reinstalls an installed package at the new build
    pub fn install(&self, names: &[String], force: bool) -> Result<(), BackendError> {
        self.build(names)?;
        let xbps = Xbps::new(self.runner, &self.env);
        let (patching, own): (Vec<String>, Vec<String>) = names.iter().cloned().partition(|name| self.patches(name));
        if !own.is_empty() {
            xbps.install_from(&self.binpkgs(), &own, force)?;
        }
        if patching.is_empty() {
            return Ok(());
        }

        // a patched build replaces void's at the same version, then a hold keeps upgrades from swapping void's back
        let installed: Vec<String> = xbps.list()?.into_iter().map(|pkg| pkg.name).filter(|name| patching.contains(name)).collect();
        if !installed.is_empty() {
            xbps.hold(&installed, false)?;
        }
        xbps.install_from(&self.binpkgs(), &patching, true)?;
        xbps.hold(&patching, true)?;
        let recorded: Vec<String> = patched(&self.env).into_iter().chain(patching).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        save_patched(&self.env, &recorded)
    }

    // whether an installed package came from our binpkgs rather than void's repos
    pub fn is_ours(&self, name: &str) -> bool {
        Xbps::new(self.runner, &self.env).origin(name).is_some_and(|origin| Path::new(&origin) == self.binpkgs())
    }

    // packages built with patches whose srcpkgs/ dir is gone: released and put back to void's build; returns them
    pub fn unpatch(&self) -> Result<Vec<String>, BackendError> {
        let (gone, kept): (Vec<String>, Vec<String>) = patched(&self.env).into_iter().partition(|name| !self.patches(name));
        if !gone.is_empty() {
            let xbps = Xbps::new(self.runner, &self.env);
            let installed: Vec<String> = xbps.list()?.into_iter().map(|pkg| pkg.name).filter(|name| gone.contains(name)).collect();
            if !installed.is_empty() {
                xbps.hold(&installed, false)?;
                xbps.reinstall(&installed)?;
            }
            save_patched(&self.env, &kept)?;
        }
        Ok(gone)
    }

    // whether this name is in the exclude list, which marks the clone's copies of our templates
    fn is_copied(&self, name: &str) -> bool {
        let line = format!("/srcpkgs/{name}");
        fs::read_to_string(self.clone.join(".git/info/exclude")).unwrap_or_default().lines().any(|existing| existing == line)
    }

    // whether the clone has its own package by this name, not a copy of ours
    fn is_official(&self, name: &str) -> bool {
        fs::symlink_metadata(self.clone.join("srcpkgs").join(name)).is_ok() && !self.is_copied(name)
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

// copies a directory tree, files keeping their permissions
fn copy_dir(from: &Path, to: &Path) -> Result<(), BackendError> {
    fs::create_dir_all(to).map_err(io(to))?;

    // each entry, recursing into subdirectories like files/ and patches/
    fs::read_dir(from).map_err(io(from))?.try_for_each(|entry| {
        let entry = entry.map_err(io(from))?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)
        } else {
            fs::copy(entry.path(), &target).map(drop).map_err(io(&target))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;
    use crate::testing::write;

    struct Setup {
        dir: tempfile::TempDir,
        runner: FakeRunner,
    }

    impl Setup {
        fn new() -> Self {
            Self::answering(|_, _| String::new())
        }

        fn answering(respond: impl Fn(&str, &[String]) -> String + 'static) -> Self {
            Setup { dir: tempfile::tempdir().unwrap(), runner: FakeRunner::new(respond) }
        }

        fn src(&self) -> SrcPkgs<'_> {
            let env = Env::new(&self.dir.path().join("home"), &self.dir.path().join("root"), Path::new("/s"));
            SrcPkgs::new(&self.runner, &env, &self.dir.path().join("dots/srcpkgs"), &self.dir.path().join("clone"))
        }

        // a clone that already has xbps-src and an official package called taken, at 1.0_1 with two patches
        fn cloned(&self) {
            let clone = self.dir.path().join("clone");
            write(&clone.join("srcpkgs/taken/template"), "pkgname=taken\nversion=1.0\nrevision=1\n");
            write(&clone.join("srcpkgs/taken/patches/b-musl.patch"), "");
            write(&clone.join("srcpkgs/taken/patches/a-fix.patch"), "");
            write(&clone.join("srcpkgs/taken/patches/a-fix.patch.args"), "-Np0");
            fs::write(clone.join("xbps-src"), "").unwrap();
        }

        // our patches for taken
        fn patch_taken(&self) {
            write(&self.dir.path().join("dots/srcpkgs/taken/patches/gl.patch"), "+gl\n");
        }

        fn calls(&self) -> Vec<String> {
            self.runner.calls.borrow().clone()
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
    fn templates_are_copied_fresh_and_excluded_from_git() {
        let setup = Setup::new();
        setup.cloned();
        let template = setup.src().new_template("hello", "me").unwrap();
        write(&template.with_file_name("patches/fix.patch"), "--- a\n");
        setup.src().build(&["hello".into()]).unwrap();
        fs::write(&template, "pkgname=hello\nversion=0.2.0\n").unwrap();
        setup.src().build(&["hello".into()]).unwrap();

        let copy = setup.dir.path().join("clone/srcpkgs/hello");
        assert!(!fs::symlink_metadata(&copy).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(copy.join("template")).unwrap(), "pkgname=hello\nversion=0.2.0\n");
        assert!(copy.join("patches/fix.patch").is_file());
        assert_eq!(fs::read_to_string(setup.dir.path().join("clone/.git/info/exclude")).unwrap(), "/srcpkgs/hello\n");
        assert!(!setup.runner.calls.borrow().iter().any(|call| call.starts_with("git clone")));
    }

    #[test]
    fn old_links_are_replaced_with_copies() {
        let setup = Setup::new();
        setup.cloned();
        let template = setup.src().new_template("hello", "me").unwrap();
        let link = setup.dir.path().join("clone/srcpkgs/hello");
        std::os::unix::fs::symlink(template.parent().unwrap(), &link).unwrap();
        write(&setup.dir.path().join("clone/.git/info/exclude"), "/srcpkgs/hello\n");

        setup.src().build(&["hello".into()]).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().is_dir());
        assert!(template.is_file());
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
    fn patches_follow_voids_own_on_voids_template() {
        let setup = Setup::new();
        setup.cloned();
        setup.patch_taken();
        let src = setup.src();
        assert!(src.has("taken") && src.patches("taken"));
        assert_eq!(src.version("taken").as_deref(), Some("1.0_1"));
        src.build(&["taken".into()]).unwrap();

        let (clone, patches) = (setup.dir.path().join("clone"), setup.dir.path().join("clone/srcpkgs/taken/patches"));
        assert_eq!(fs::read_to_string(patches.join("maw-gl.patch")).unwrap(), "+gl\n");
        assert_eq!(fs::read_to_string(patches.join("series")).unwrap(), "a-fix.patch\nb-musl.patch\nmaw-gl.patch\n");

        // void's dir is restored before patching, and a patched build is forced over the last one
        let calls = setup.calls();
        let restore = format!("git -C {} checkout -- srcpkgs/taken", clone.display());
        assert!(calls.iter().position(|call| *call == restore) < calls.iter().position(|call| call.contains("xbps-src")));
        assert_eq!(calls.last().unwrap(), &format!("{} -f pkg taken", clone.join("xbps-src").display()));
    }

    #[test]
    fn patches_need_a_package_to_patch() {
        let setup = Setup::new();
        setup.cloned();
        write(&setup.dir.path().join("dots/srcpkgs/nothing/patches/x.patch"), "");
        assert!(matches!(setup.src().build(&["nothing".into()]), Err(BackendError::NotInVoid(_))));
    }

    #[test]
    fn a_newer_void_release_moves_the_clone_first() {
        let setup = Setup::answering(|program, args| match (program, args.last().map(String::as_str)) {
            ("xbps-query", Some("taken")) if args.contains(&"-R".to_string()) => "pkgver: taken-1.1_1\n".into(),
            _ => String::new(),
        });
        setup.cloned();
        setup.patch_taken();
        setup.src().build(&["taken".into()]).unwrap();
        let clone = setup.dir.path().join("clone").display().to_string();
        assert!(setup.calls().contains(&format!("git -C {clone} fetch --depth 1 origin master")));
        assert!(setup.calls().contains(&format!("git -C {clone} reset -q --hard FETCH_HEAD")));
    }

    #[test]
    fn patched_installs_replace_voids_build_and_stay_held() {
        let setup = Setup::answering(|program, args| match (program, args.last().map(String::as_str)) {
            ("xbps-query", Some("-l")) => "ii taken-1.0_1  void's build\n".into(),
            _ => String::new(),
        });
        fs::create_dir_all(setup.dir.path().join("root/var/db/xbps")).unwrap();
        setup.cloned();
        setup.patch_taken();
        setup.src().install(&["taken".into()], false).unwrap();

        let (root, binpkgs) = (setup.dir.path().join("root"), setup.dir.path().join("clone/hostdir/binpkgs"));
        let root = root.display();
        let installs: Vec<String> = setup.calls().into_iter().filter(|call| call.starts_with("xbps-install") || call.starts_with("xbps-pkgdb")).collect();
        assert_eq!(installs, [
            format!("xbps-pkgdb -r {root} -m unhold taken"),
            format!("xbps-install -r {root} -R {} -fy taken", binpkgs.display()),
            format!("xbps-pkgdb -r {root} -m hold taken"),
        ]);
        assert_eq!(patched(&setup.src().env), ["taken"]);
    }

    #[test]
    fn dropped_patches_put_voids_build_back() {
        let setup = Setup::answering(|program, args| match (program, args.last().map(String::as_str)) {
            ("xbps-query", Some("-l")) => "ii taken-1.0_1  our build\n".into(),
            _ => String::new(),
        });
        fs::create_dir_all(setup.dir.path().join("root/var/db/xbps")).unwrap();
        let env = setup.src().env;
        save_patched(&env, &["taken".into()]).unwrap();

        assert_eq!(setup.src().unpatch().unwrap(), ["taken"]);
        let root = setup.dir.path().join("root").display().to_string();
        let calls = setup.calls();
        assert!(calls.contains(&format!("xbps-pkgdb -r {root} -m unhold taken")) && calls.contains(&format!("xbps-install -r {root} -fy taken")));
        assert!(patched(&env).is_empty());
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
