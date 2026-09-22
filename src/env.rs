use std::path::{Path, PathBuf};

// every location maw reads or writes, so tests can point them at a tempdir
#[derive(Debug, Clone)]
pub struct Env {
    pub home: PathBuf,
    pub sysroot: PathBuf,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub share_dir: PathBuf,
}

impl Env {
    // builds paths under a home and system root, with maw's data in share_dir
    pub fn new(home: &Path, sysroot: &Path, share_dir: &Path) -> Self {
        Env {
            home: home.to_path_buf(),
            sysroot: sysroot.to_path_buf(),
            config_dir: home.join(".config/maw"),
            state_dir: home.join(".local/state/maw"),
            share_dir: share_dir.to_path_buf(),
        }
    }

    // reads HOME and MAW_SYSROOT; share_dir is /usr/share/maw when installed, else this checkout
    pub fn from_process() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
        let sysroot = std::env::var_os("MAW_SYSROOT").map(PathBuf::from).unwrap_or_else(|| "/".into());
        let installed = PathBuf::from("/usr/share/maw");
        let share_dir = if installed.exists() { installed } else { PathBuf::from(env!("CARGO_MANIFEST_DIR")) };
        Env::new(&home, &sysroot, &share_dir)
    }

    pub fn nix_dir(&self) -> PathBuf {
        self.share_dir.join("nix")
    }

    pub fn registry_file(&self) -> PathBuf {
        self.share_dir.join("registry.nix")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.state_dir.join("cache")
    }

    // a path for display, with home shortened to ~
    pub fn pretty(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) => Path::new("~").join(rest).display().to_string(),
            Err(_) => path.display().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_shortens_home() {
        let env = Env::new(Path::new("/h"), Path::new("/"), Path::new("/s"));
        assert_eq!(env.pretty(Path::new("/h/dots/out")), "~/dots/out");
        assert_eq!(env.pretty(Path::new("/etc/sv")), "/etc/sv");
    }
}
