use std::path::{Path, PathBuf};

// every location maw reads or writes, so tests can point them at a tempdir
#[derive(Debug, Clone)]
pub struct Env {
    pub home: PathBuf,
    pub sysroot: PathBuf,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub share_dir: PathBuf,
    // whether there's a desktop session bus, which writing dconf needs
    pub session_bus: bool,
    // this machine's name, which picks its hosts/<name>.nix and per-machine lists
    pub host: String,
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
            session_bus: true,
            host: "host".into(),
        }
    }

    // where this machine's name is kept
    pub fn host_file(&self) -> PathBuf {
        self.config_dir.join("host")
    }

    // reads HOME and MAW_SYSROOT; share_dir is the checkout maw was built from while it's still there, so a
    // development build uses its own nix lib and registry, else /usr/share/maw (xbps-src deletes its build dir)
    pub fn from_process() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
        let sysroot = std::env::var_os("MAW_SYSROOT").map(PathBuf::from).unwrap_or_else(|| "/".into());
        let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let share_dir = if checkout.join("nix/lib.nix").exists() { checkout } else { PathBuf::from("/usr/share/maw") };
        let session_bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some();
        let env = Env::new(&home, &sysroot, &share_dir);
        let host = named_host(&env.host_file());
        Env { session_bus, host, ..env }
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

    pub fn backup_dir(&self) -> PathBuf {
        self.state_dir.join("backups")
    }

    // a path for display, with home shortened to ~
    pub fn pretty(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) => Path::new("~").join(rest).display().to_string(),
            Err(_) => path.display().to_string(),
        }
    }
}

// the name maw was given for this machine, else its hostname
pub fn named_host(file: &Path) -> String {
    let read = |path: &str| std::fs::read_to_string(path).ok().map(|name| name.trim().to_string()).filter(|name| !name.is_empty());
    read(&file.display().to_string()).or_else(|| read("/etc/hostname")).or_else(|| read("/proc/sys/kernel/hostname")).unwrap_or_else(|| "localhost".into())
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
