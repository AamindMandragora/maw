use crate::backend::BackendError;
use crate::backend::srcpkgs::SrcPkgs;
use crate::backend::xbps::Xbps;
use crate::build::{self, BuildError};
use crate::env::Env;
use crate::eval::{self, EvalError};
use crate::inputs::hash_bytes;
use crate::repo::Repo;
use crate::runner::{RunError, Runner};
use crate::state::{attr_name, string};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub mod aur;
pub mod nixos;
pub mod nixpkgs;
pub mod xbps_src;

#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    #[error("srcpkgs/{0}/template already exists")]
    Exists(String),
    #[error("srcpkgs/{0}/template wasn't drafted from nixpkgs or the aur")]
    NotDrafted(String),
    #[error("{0} has no source url to fetch")]
    NoSource(String),
    #[error("void already has {0}; `maw install {0}`, or patch it with srcpkgs/{0}/patches/")]
    InVoid(String),
    #[error("search.nixos.org didn't answer the way maw expects; search nixpkgs at https://search.nixos.org instead")]
    NixSearch,
    #[error("the aur has no {0}; `maw search {0}` to look for it")]
    NotInAur(String),
    #[error("{0} builds from a git checkout; draft from its release package instead, usually the name without -git")]
    Vcs(String),
    #[error("nixpkgs {attr}: unexpected metadata")]
    Meta { attr: String, source: serde_json::Error },
    #[error(transparent)]
    Eval(#[from] EvalError),
    #[error(transparent)]
    Run(#[from] RunError),
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error(transparent)]
    Build(#[from] BuildError),
    #[error("{path}")]
    Io { path: PathBuf, source: std::io::Error },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ScaffoldError + '_ {
    move |source| ScaffoldError::Io { path: path.into(), source }
}

// what nix/nixpkgs-meta.nix prints for one package
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub pname: String,
    pub version: String,
    pub description: String,
    pub homepage: String,
    pub licenses: Vec<String>,
    pub urls: Vec<String>,
    pub builder: String,
    pub native_build_inputs: Vec<String>,
    pub build_inputs: Vec<String>,
    pub sub_packages: Vec<String>,
}

// a package to build, in no distro's terms: where its source is, how it builds, and what it needs by upstream name
#[derive(Debug, Clone, PartialEq)]
pub struct SourcePkg {
    pub name: String,
    pub version: String,
    pub description: String,
    pub homepage: String,
    pub licenses: Vec<String>,
    pub distfile: String,
    pub build: String,
    pub host_deps: Vec<String>,
    pub deps: Vec<String>,
    // for go: the module path, and packages to build other than the root
    pub go_import_path: Option<String>,
    pub go_packages: Vec<String>,
    // where it came from, e.g. "nixpkgs 'lazygit' at 4975466d3"
    pub origin: String,
    // what its dependency names are, for TODO lines: "nix" or "aur"
    pub upstream: String,
    // deps mixes libraries and runtime programs (arch's depends): only -devel matches are build deps, the rest runtime
    pub mixed_deps: bool,
    // things the draft couldn't carry over, each a TODO line
    pub notes: Vec<String>,
    // text for the end of the template, like upstream build steps kept as comments
    pub extra: String,
}

impl SourcePkg {
    // a source package from nixpkgs metadata; the first url is the distfile
    pub fn from_meta(meta: &Meta, name: &str, origin: &str) -> Result<Self, ScaffoldError> {
        let distfile = meta.urls.first().cloned().ok_or_else(|| ScaffoldError::NoSource(name.into()))?;
        let go_import_path = (meta.builder == "go").then(|| go_import_path(&meta.homepage, &distfile)).flatten();
        let go_packages = match &go_import_path {
            Some(root) => meta.sub_packages.iter().filter(|sub| *sub != ".").map(|sub| format!("{root}/{}", sub.trim_start_matches("./"))).collect(),
            None => Vec::new(),
        };
        Ok(SourcePkg {
            name: name.into(),
            version: meta.version.clone(),
            description: meta.description.clone(),
            homepage: meta.homepage.clone(),
            licenses: meta.licenses.clone(),
            distfile,
            build: meta.builder.clone(),
            host_deps: meta.native_build_inputs.clone(),
            deps: meta.build_inputs.clone(),
            go_import_path,
            go_packages,
            origin: origin.into(),
            upstream: "nix".into(),
            mixed_deps: false,
            notes: Vec::new(),
            extra: String::new(),
        })
    }
}

// a go module path from a forge url: https://github.com/a/b/... -> github.com/a/b
pub(crate) fn go_import_path(homepage: &str, distfile: &str) -> Option<String> {
    [homepage, distfile].iter().find_map(|url| {
        let rest = url.strip_prefix("https://")?;
        let parts: Vec<&str> = rest.split('/').take(3).collect();
        let forge = ["github.com", "gitlab.com", "codeberg.org", "git.sr.ht"].contains(&parts[0]);
        (forge && parts.len() == 3).then(|| parts.join("/").trim_end_matches(".git").to_string())
    })
}

// what a template writer made, and the upstream dependencies it couldn't map
#[derive(Debug, PartialEq)]
pub struct Emitted {
    pub text: String,
    pub todos: Vec<String>,
}

// a distro's template writer
pub trait SourceEmitter {
    fn emit(&self, pkg: &SourcePkg, deps: &Deps, maintainer: &str, checksum: &str) -> Emitted;
}

// how one upstream dependency comes out: a package, dropped, or unknown
#[derive(Debug, PartialEq)]
pub enum Resolved {
    Package(String),
    Drop,
    Unknown,
}

// upstream dependency names to distro packages: the depmap first, then names the distro has, matched regardless of case
pub struct Deps {
    map: BTreeMap<String, Option<String>>,
    known: HashMap<String, String>,
}

impl Deps {
    // known is every package name the distro has, as it spells them
    pub fn new(map: BTreeMap<String, Option<String>>, known: HashSet<String>) -> Self {
        Deps { map, known: known.into_iter().map(|name| (name.to_lowercase(), name)).collect() }
    }

    // whether the distro has a package by exactly this name
    pub fn has(&self, name: &str) -> bool {
        self.known.get(&name.to_lowercase()).is_some_and(|known| known == name)
    }

    // development libraries prefer the -devel package, the way void's makedepends are written; arch's python-x is void's python3-x
    pub fn resolve(&self, name: &str, devel: bool) -> Resolved {
        if let Some(mapped) = self.map.get(name) {
            return mapped.clone().map_or(Resolved::Drop, Resolved::Package);
        }
        if name.ends_with("-hook") || name.ends_with("-hook.sh") {
            return Resolved::Drop;
        }
        let python = name.strip_prefix("python-").map(|module| format!("python3-{module}"));
        let candidates = [devel.then(|| format!("{name}-devel")), Some(name.to_string()), python];
        let found = candidates.into_iter().flatten().find_map(|candidate| self.known.get(&candidate.to_lowercase()));
        found.map_or(Resolved::Unknown, |package| Resolved::Package(package.clone()))
    }
}

// the shipped depmap with the dotfiles' own on top; null entries drop a dependency
pub fn load_deps(env: &Env, runner: &dyn Runner, repo: &Repo) -> Result<Deps, ScaffoldError> {
    let map = [env.share_dir.join("depmap.nix"), user_depmap(repo)]
        .iter()
        .filter(|file| file.exists())
        .map(|file| {
            let key = hash_bytes(&fs::read(file).map_err(io(file))?);
            let name = if file.starts_with(&env.share_dir) { "depmap-shipped" } else { "depmap-user" };
            Ok(eval::eval_file(runner, env, file, name, &key)?)
        })
        .collect::<Result<Vec<Value>, ScaffoldError>>()?
        .into_iter()
        .flat_map(|value| value.as_object().cloned().unwrap_or_default())
        .map(|(name, mapped)| (name, mapped.as_str().map(String::from)))
        .collect();
    Ok(Deps::new(map, Xbps::new(runner, env).repo_names()?))
}

fn user_depmap(repo: &Repo) -> PathBuf {
    repo.root.join("depmap.nix")
}

// adds mappings to the dotfiles' depmap.nix, written in maw's fixed shape
pub fn record(env: &Env, runner: &dyn Runner, repo: &Repo, learned: &[(String, String)]) -> Result<(), ScaffoldError> {
    let file = user_depmap(repo);
    let mut map: BTreeMap<String, String> = match file.exists() {
        true => {
            let key = hash_bytes(&fs::read(&file).map_err(io(&file))?);
            let value = eval::eval_file(runner, env, &file, "depmap-user", &key)?;
            value.as_object().into_iter().flatten().filter_map(|(name, mapped)| Some((name.clone(), mapped.as_str()?.to_string()))).collect()
        }
        false => BTreeMap::new(),
    };
    map.extend(learned.iter().cloned());
    let lines: String = map.iter().map(|(name, mapped)| format!("  {} = {};\n", attr_name(name), string(mapped))).collect();
    let text = format!("# upstream dependency (nixpkgs or arch) -> void package, learned by `maw src new`; maw writes this file\n{{\n{lines}}}\n");
    fs::write(&file, text).map_err(io(&file))
}

// sha256 of a distfile, downloaded into the cache
pub fn checksum(env: &Env, runner: &dyn Runner, url: &str) -> Result<String, ScaffoldError> {
    let dir = env.cache_dir().join("distfiles");
    fs::create_dir_all(&dir).map_err(io(&dir))?;
    let file = dir.join(url.rsplit('/').next().unwrap_or("distfile"));
    runner.run("curl", &["-fsSL".into(), "-o".into(), file.display().to_string(), url.into()])?;
    let sum = runner.run("sha256sum", &[file.display().to_string()])?;
    Ok(sum.split_whitespace().next().unwrap_or_default().to_string())
}

// the words of a template's dependency fields
fn dep_words(text: &str) -> Vec<String> {
    let fields = ["hostmakedepends=", "makedepends=", "depends="];
    text.lines()
        .filter_map(|line| fields.iter().find_map(|field| line.strip_prefix(field)))
        .flat_map(|value| value.trim_matches('"').split_whitespace().map(String::from).collect::<Vec<_>>())
        .collect()
}

// mappings to learn from an edit: TODOs the user removed, paired in order with the dependencies they added; only when the counts match
pub fn learned(before: &str, after: &str, todos: &[String]) -> Vec<(String, String)> {
    let removed: Vec<&String> = todos.iter().filter(|todo| !after.contains(&format!(" had '{todo}'"))).collect();
    let old = dep_words(before);
    let added: Vec<String> = dep_words(after).into_iter().filter(|word| !old.contains(word)).collect();
    if removed.len() != added.len() {
        return Vec::new();
    }
    removed.into_iter().cloned().zip(added).collect()
}

// where a drafted template came from
#[derive(Debug, PartialEq)]
pub enum Upstream {
    Nix(String),
    Aur(String),
}

// the header line recording where a drafted template came from: "# scaffolded by maw from nixpkgs 'ripgrep' at abc"
fn origin(template: &str) -> Option<Upstream> {
    let line = template.lines().find_map(|line| line.strip_prefix("# scaffolded by maw from "))?;
    let name = line.split('\'').nth(1)?.to_string();
    match line.split_whitespace().next()? {
        "nixpkgs" => Some(Upstream::Nix(name)),
        "aur" => Some(Upstream::Aur(name)),
        _ => None,
    }
}

// the source package an upstream describes now; nixpkgs is pulled first when asked
fn upstream_pkg(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str, upstream: &Upstream, pull: bool) -> Result<SourcePkg, ScaffoldError> {
    match upstream {
        Upstream::Nix(attr) => {
            let nixpkgs = nixpkgs::Nixpkgs::new(runner, env, &build::settings(env, runner, repo)?.nixpkgs(env));
            if pull {
                nixpkgs.pull()?;
            }
            nixpkgs.source_pkg(name, attr)
        }
        Upstream::Aur(aur_name) => {
            let info = aur::info(runner, aur_name)?;
            let text = aur::pkgbuild(runner, info["PackageBase"].as_str().unwrap_or(aur_name))?;
            aur::source_pkg(&info, &text, name)
        }
    }
}

// the distfile's checksum, or none when the url still has an expansion to fix by hand
fn distfile_checksum(env: &Env, runner: &dyn Runner, pkg: &SourcePkg) -> Result<String, ScaffoldError> {
    match pkg.distfile.contains('$') {
        true => Ok(String::new()),
        false => checksum(env, runner, &pkg.distfile.replace("${version}", &pkg.version)),
    }
}

// writes srcpkgs/<name>/template drafted from nixpkgs or the aur; returns the file and what the emitter made
pub fn draft(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str, upstream: &Upstream, maintainer: &str) -> Result<(PathBuf, Emitted), ScaffoldError> {
    let file = repo.srcpkgs_dir().join(name).join("template");
    if file.exists() {
        return Err(ScaffoldError::Exists(name.into()));
    }
    // void-packages' own list when maw has cloned it; the repos otherwise, which also hold local builds
    let deps = load_deps(env, runner, repo)?;
    let src = SrcPkgs::new(runner, env, &repo.srcpkgs_dir(), &build::settings(env, runner, repo)?.void_packages(env));
    if src.in_void(name).unwrap_or_else(|| deps.has(name)) {
        return Err(ScaffoldError::InVoid(name.into()));
    }
    let pkg = upstream_pkg(env, runner, repo, name, upstream, false)?;
    let checksum = distfile_checksum(env, runner, &pkg)?;
    let emitted = xbps_src::XbpsSrc.emit(&pkg, &deps, maintainer, &checksum);

    fs::create_dir_all(file.parent().unwrap()).map_err(io(&file))?;
    fs::write(&file, &emitted.text).map_err(io(&file))?;
    Ok((file, emitted))
}

// moves a drafted template to its upstream's current version; Some((old, new)) if it moved
pub fn update(env: &Env, runner: &dyn Runner, repo: &Repo, name: &str) -> Result<Option<(String, String)>, ScaffoldError> {
    let file = repo.srcpkgs_dir().join(name).join("template");
    let text = fs::read_to_string(&file).map_err(io(&file))?;
    let upstream = origin(&text).ok_or_else(|| ScaffoldError::NotDrafted(name.into()))?;

    let pkg = upstream_pkg(env, runner, repo, name, &upstream, true)?;
    let old = text.lines().find_map(|line| line.strip_prefix("version=")).unwrap_or_default().trim_matches('"').to_string();
    if old == pkg.version {
        return Ok(None);
    }

    let checksum = distfile_checksum(env, runner, &pkg)?;
    fs::write(&file, xbps_src::bump(&text, &pkg, &checksum)).map_err(io(&file))?;
    Ok(Some((old, pkg.version)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deps() -> Deps {
        let map = BTreeMap::from([("pkg-config-wrapper".into(), Some("pkg-config".into())), ("go".into(), None)]);
        Deps::new(map, ["pkg-config", "pcre2", "pcre2-devel", "scdoc", "tllist", "libXdmcp-devel", "python3-requests"].map(String::from).into())
    }

    #[test]
    fn deps_resolve_through_map_devel_and_name() {
        let deps = deps();
        assert_eq!(deps.resolve("pkg-config-wrapper", false), Resolved::Package("pkg-config".into()));
        assert_eq!(deps.resolve("go", false), Resolved::Drop);
        assert_eq!(deps.resolve("cargo-build-hook.sh", false), Resolved::Drop);
        assert_eq!(deps.resolve("pcre2", true), Resolved::Package("pcre2-devel".into()));
        assert_eq!(deps.resolve("scdoc", false), Resolved::Package("scdoc".into()));
        assert_eq!(deps.resolve("nothing", true), Resolved::Unknown);
        assert_eq!(deps.resolve("libxdmcp", true), Resolved::Package("libXdmcp-devel".into()));
        assert_eq!(deps.resolve("python-requests", false), Resolved::Package("python3-requests".into()));
    }

    #[test]
    fn go_import_paths_come_from_forge_urls() {
        assert_eq!(go_import_path("https://github.com/jesseduffield/lazygit", "").as_deref(), Some("github.com/jesseduffield/lazygit"));
        assert_eq!(go_import_path("https://lazygit.dev", "https://codeberg.org/a/b/archive/v1.tar.gz").as_deref(), Some("codeberg.org/a/b"));
        assert_eq!(go_import_path("https://example.org", "https://example.org/x.tar.gz"), None);
    }

    #[test]
    fn replaced_todos_pair_with_added_deps() {
        let before = "makedepends=\"a-devel\"\n# TODO: nix had 'libfoo'\n# TODO: nix had 'bar'\n";
        let after = "makedepends=\"a-devel foo-devel\"\n# TODO: nix had 'bar'\n";
        assert_eq!(learned(before, after, &["libfoo".into(), "bar".into()]), [("libfoo".to_string(), "foo-devel".to_string())]);
        assert!(learned(before, "makedepends=\"a-devel x y\"\n", &["libfoo".into(), "bar".into(), "baz".into()]).is_empty());
    }

    #[test]
    fn drafted_templates_name_their_upstream() {
        assert_eq!(origin("# Template file for 'rg'\n# scaffolded by maw from nixpkgs 'ripgrep' at abc\n"), Some(Upstream::Nix("ripgrep".into())));
        assert_eq!(origin("# scaffolded by maw from aur 'yay-bin' at 13.0.1-1\n"), Some(Upstream::Aur("yay-bin".into())));
        assert_eq!(origin("pkgname=x\n"), None);
    }
}
