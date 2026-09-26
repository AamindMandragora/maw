use crate::env::Env;
use crate::runner::{RunError, Runner};
use std::path::PathBuf;

pub mod cargo;
pub mod flatpak;
pub mod go;
pub mod srcpkgs;
pub mod xbps;

// every backend, in the order a bare `maw install foo` tries them after xbps
pub const NAMES: [&str; 4] = ["xbps", "flatpak", "cargo", "go"];

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("{backend}: unexpected output `{line}`")]
    Parse { backend: String, line: String },
    #[error("{path}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("void-packages already has {0}; pick another name, or patch void's with srcpkgs/{0}/patches/")]
    Taken(String),
    #[error("srcpkgs/{0} has patches but no template, and void-packages has no {0} to patch")]
    NotInVoid(String),
    #[error("srcpkgs/{0}/template already exists")]
    TemplateExists(String),
}

// one package as a backend reports it; source is what maw.nix declares (crate name, git url, go path)
// and manual is false for dependencies and repo results
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pkg {
    pub name: String,
    pub source: String,
    pub version: String,
    pub description: String,
    pub homepage: String,
    pub manual: bool,
    // the executables it builds, when the backend can tell; an empty list means a library
    pub programs: Option<Vec<String>>,
    // the exact build, when the version alone can't reinstall it: a flatpak commit
    pub build: String,
}

// a source of packages: installed state, the repo, and changes to either
pub trait Backend {
    fn name(&self) -> &str;

    // every installed package, with versions
    fn list(&self) -> Result<Vec<Pkg>, BackendError>;

    fn search(&self, term: &str) -> Result<Vec<Pkg>, BackendError>;

    // one package from the repo, or None if the repo doesn't have it
    fn info(&self, name: &str) -> Result<Option<Pkg>, BackendError>;

    fn install(&self, names: &[String]) -> Result<(), BackendError>;

    fn remove(&self, names: &[String]) -> Result<(), BackendError>;

    // the exact spec that reinstalls this version
    fn pin(&self, pkg: &Pkg) -> String;

    // where the backend puts executables, if it isn't somewhere the system already searches
    fn bin_dir(&self) -> Option<PathBuf> {
        None
    }
}

// the backend that owns the base system
pub trait SystemBackend: Backend {
    // refreshes the repo index and upgrades everything
    fn sync(&self) -> Result<(), BackendError>;

    // the cached package file of an exact version, if the local cache kept it
    fn cached(&self, pkgver: &str) -> Option<PathBuf>;

    // installs exact versions, downgrading if needed; cached ones come from the cache
    fn install_versions(&self, pkgvers: &[String]) -> Result<(), BackendError>;

    // holds packages so upgrades skip them, or releases them
    fn hold(&self, names: &[String], hold: bool) -> Result<(), BackendError>;
}

// the backend a maw.nix packages key names
pub fn for_name<'a>(name: &str, runner: &'a dyn Runner, env: &Env) -> Option<Box<dyn Backend + 'a>> {
    match name {
        "xbps" => Some(Box::new(xbps::Xbps::new(runner, env))),
        "cargo" => Some(Box::new(cargo::Cargo::new(runner, env))),
        "go" => Some(Box::new(go::Go::new(runner, env))),
        "flatpak" => Some(Box::new(flatpak::Flatpak::new(runner, env))),
        _ => None,
    }
}

// a spec without its version: "bat@0.24" -> "bat"; urls like git@host:path keep their @
pub fn spec_base(spec: &str) -> &str {
    match spec.rsplit_once('@') {
        Some((base, version)) if !base.is_empty() && !version.contains(['/', ':']) => base,
        _ => spec,
    }
}

// the program a spec installs, for finding its registry entry: the last path segment, minus .git
pub fn spec_program(spec: &str) -> String {
    let base = spec_base(spec).trim_end_matches('/');
    let last = base.rsplit(['/', ':']).next().unwrap_or(base);
    last.trim_end_matches(".git").to_lowercase()
}

// "foot-1.28.0_1" -> ("foot", "1.28.0_1")
pub fn split_pkgver(pkgver: &str) -> Option<(String, String)> {
    let (name, version) = pkgver.rsplit_once('-')?;
    Some((name.into(), version.into()))
}

// whether version a is newer than b, comparing runs of digits as numbers: "0.10.0_1" is newer than "0.9.0_2"
pub fn newer(a: &str, b: &str) -> bool {
    // "1.10.0_2" -> [1, 10, 0, 2], with any letters kept as their own parts
    let parts = |version: &str| -> Vec<(u64, String)> {
        let pieces = version.split(|char: char| !char.is_ascii_alphanumeric()).filter(|piece| !piece.is_empty());
        pieces.map(|piece| piece.parse::<u64>().map_or((0, piece.to_string()), |number| (number, String::new()))).collect()
    };
    parts(a) > parts(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_compares_numbers_as_numbers() {
        assert!(newer("0.10.0_1", "0.9.0_2"));
        assert!(newer("0.99.0_2", "0.99.0_1"));
        assert!(!newer("0.99.0_1", "0.99.0_1"));
        assert!(!newer("1.0_1", "1.0.1_1"));
    }

    #[test]
    fn spec_base_drops_only_versions() {
        assert_eq!(spec_base("typos-cli@1.24"), "typos-cli");
        assert_eq!(spec_base("golang.org/x/tools/gopls@v0.16.0"), "golang.org/x/tools/gopls");
        assert_eq!(spec_base("git@github.com:user/tool.git"), "git@github.com:user/tool.git");
        assert_eq!(spec_base("https://github.com/vitali87/croft.git"), "https://github.com/vitali87/croft.git");
    }

    #[test]
    fn spec_program_is_the_last_segment() {
        assert_eq!(spec_program("github.com/jesseduffield/lazygit@v0.40"), "lazygit");
        assert_eq!(spec_program("https://github.com/vitali87/croft.git"), "croft");
        assert_eq!(spec_program("Waybar"), "waybar");
    }

    #[test]
    fn pkgver_splits_at_the_last_dash() {
        assert_eq!(split_pkgver("alacritty-terminfo-0.17.0_1"), Some(("alacritty-terminfo".into(), "0.17.0_1".into())));
        assert_eq!(split_pkgver("nodash"), None);
    }
}
