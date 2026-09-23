use crate::env::Env;
use crate::runner::{RunError, Runner};

pub mod xbps;

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("{backend}: unexpected output `{line}`")]
    Parse { backend: String, line: String },
}

// one package as a backend reports it; manual is false for dependencies and repo results
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pkg {
    pub name: String,
    pub version: String,
    pub description: String,
    pub homepage: String,
    pub manual: bool,
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
}

// the backend that owns the base system
pub trait SystemBackend: Backend {
    // refreshes the repo index and upgrades everything
    fn sync(&self) -> Result<(), BackendError>;
}

// the backend a maw.nix packages key names
pub fn for_name<'a>(name: &str, runner: &'a dyn Runner, env: &Env) -> Option<Box<dyn Backend + 'a>> {
    match name {
        "xbps" => Some(Box::new(xbps::Xbps::new(runner, env))),
        _ => None,
    }
}

// "foot-1.28.0_1" -> ("foot", "1.28.0_1")
pub fn split_pkgver(pkgver: &str) -> Option<(String, String)> {
    let (name, version) = pkgver.rsplit_once('-')?;
    Some((name.into(), version.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkgver_splits_at_the_last_dash() {
        assert_eq!(split_pkgver("alacritty-terminfo-0.17.0_1"), Some(("alacritty-terminfo".into(), "0.17.0_1".into())));
        assert_eq!(split_pkgver("nodash"), None);
    }
}
